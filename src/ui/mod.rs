//! Native Ratatui harness UI.
//!
//! The screen keeps the transcript central, makes the composer persistent,
//! and moves secondary work into small overlays. Long-running requests and
//! tools are cancellable, messages sent while busy are queued, and every
//! picker shares the same visible-window calculation.

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write, stdout},
    path::{Path, PathBuf},
    process::{Command as OsCommand, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyModifiers, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    backend::{Backend, BackendEvent, Connection},
    command::{self, Mode, ParsedCommand},
    config::{Config, ConfigPaths},
    custom::{self, CustomCommand},
    export,
    model::{ChatResult, ChatState, Message, Usage},
    provider::{self, Preset},
    sandbox::Sandbox,
    security::safe_path,
    session,
    tool::{self, ToolContext, ToolResult},
};

mod approve;
mod commands;
mod complete;
mod events;
mod ghost;
mod input;
mod render;
mod sidebar;
mod tabs;
mod transcript;

pub(crate) use approve::*;
pub(crate) use commands::*;
pub(crate) use events::*;
pub(crate) use input::*;
pub(crate) use transcript::*;

pub use crate::config::THEMES;

pub async fn run(
    backend: Backend,
    state: ChatState,
    paths: ConfigPaths,
    config: Config,
) -> Result<()> {
    let mouse = config.mouse;
    let mut terminal = setup_terminal(mouse)?;
    let mut app = UiApp::new(backend, state, paths, config);
    let result = app.event_loop(&mut terminal).await;
    restore_terminal(&mut terminal)?;
    // The alternate screen is gone here, so the autosave note is visible.
    if let Some(notice) = app.exit_notice.take() {
        eprintln!("{notice}");
    }
    result
}

fn setup_terminal(mouse: bool) -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut output = stdout();
    execute!(output, EnterAlternateScreen).context("entering alternate screen")?;
    if mouse {
        execute!(output, EnableMouseCapture).context("enabling mouse capture")?;
    }
    let backend = CrosstermBackend::new(output);
    Terminal::new(backend).context("creating terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("disabling terminal raw mode")?;
    // Harmless when mouse capture was never enabled.
    let _ = execute!(terminal.backend_mut(), DisableMouseCapture);
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("leaving alternate screen")?;
    terminal.show_cursor().context("showing terminal cursor")
}

struct UiApp {
    pub(crate) backend: Backend,
    pub(crate) state: ChatState,
    pub(crate) paths: ConfigPaths,
    pub(crate) plugins_dir: PathBuf,
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) mode: Mode,
    pub(crate) overlay: crate::ui::events::Overlay,
    pub(crate) palette_selected: usize,
    pub(crate) palette_scroll: usize,
    pub(crate) transcript_scroll: usize,
    pub(crate) follow_transcript: bool,
    pub(crate) show_details: bool,
    pub(crate) busy: bool,
    pub(crate) streaming: String,
    pub(crate) status: String,
    pub(crate) status_tone: crate::ui::events::StatusTone,
    /// When the active request started, for the slow-start hint. Cold model
    /// loads look exactly like a hung request until the first token lands.
    pub(crate) request_started: Option<Instant>,
    pub(crate) awaiting_first_token: bool,
    pub(crate) slow_hint_shown: bool,
    pub(crate) queue: VecDeque<(String, Option<String>)>,
    /// Recently dispatched slash commands, most recent first (cap 8), so
    /// the palette can float repeated commands above fuzzy order.
    pub(crate) recent_commands: VecDeque<String>,
    /// In-flight compaction backup name: doubles as the "a compaction
    /// (not a chat) failed" flag so `ChatError` reports `/compact to
    /// retry` instead of the chat `/retry` path. Cleared in both arms.
    pub(crate) compact_backup: Option<String>,
    /// Name of the session this transcript was saved as or loaded from
    /// (fork sources and checkpoint parents). Forks leave it alone: you
    /// keep working here, the copy points back at you.
    pub(crate) current_session: Option<String>,
    /// Session-backed tabs (Warp-style): each tab is a saved session the
    /// bar can switch to; switching autosaves the live session first.
    pub(crate) tabs: Vec<tabs::Tab>,
    pub(crate) active_tab: usize,
    pub(crate) last_tab_rect: Rect,
    /// Next lazy transcript message number (`m<N>` IDs are assigned on
    /// first render so undo/redo/compact never shift section identity).
    pub(crate) next_msg_id: u64,
    /// Per-section expand overrides keyed by message ID; entries exist
    /// only where the user diverged from the global defaults.
    pub(crate) section_state: HashMap<String, bool>,
    /// Message IDs of the sections in gutter order, rebuilt every draw
    /// so `/expand n` resolves against what is currently visible. The
    /// bool is the section's global default (tool output follows
    /// `show_details`, thinking follows `thinking_default_expanded`).
    pub(crate) section_order: Vec<(String, bool)>,
    /// Per-block output filter keyed by message id (spec 0017). Entries exist
    /// only where the user has applied /filter. Pruned with sections on
    /// transcript load/save.
    pub(crate) block_filters: HashMap<String, BlockFilter>,
    /// File context resolved from `@refs`, pushed as a system message next
    /// to the user message once the backend answers.
    pub(crate) pending_context: Option<String>,
    /// Undone exchanges, newest last; any new user prompt clears the stack.
    pub(crate) redo_stack: Vec<crate::ui::events::UndoEntry>,
    /// Set by steering: the in-flight request is cancelled and its prompt
    /// must not be restored into the composer or offered via `/retry`.
    pub(crate) drop_next_restore: bool,
    pub(crate) active_user: Option<String>,
    pub(crate) last_failed_prompt: Option<String>,
    pub(crate) tool_round: usize,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) pending_connection: Option<Connection>,
    pub(crate) sandbox: Sandbox,
    /// Approval policy from config; card `a` verdicts extend it per run.
    pub(crate) policy: crate::approve::Policy,
    /// Tool round paused on approval cards; cleared by cancel.
    pub(crate) pending_tools: Option<PendingTools>,
    /// Shared with tool workers: `todo_write` replaces the list here,
    /// the results handler syncs it into session state.
    pub(crate) shared_todos: Arc<Mutex<Vec<crate::model::TodoItem>>>,
    /// Ghost text: visible suggestion, debounce bookkeeping, and the
    /// shell-history frequency store behind the cascade.
    pub(crate) ghost_text: Option<String>,
    pub(crate) ghost_seen_input: String,
    pub(crate) ghost_dismissed: Option<String>,
    pub(crate) ghost_changed_at: Instant,
    pub(crate) ghost_debounce: Duration,
    pub(crate) completion_on: bool,
    /// Model ghost behind the local cascade: idle-only, debounced,
    /// one flight per input generation. Toggle with `/completion`.
    pub(crate) ai_suggest_on: bool,
    pub(crate) ai_ghost_seq: u64,
    pub(crate) ai_ghost_pending: Option<String>,
    pub(crate) shell_history: crate::suggest::ShellHistory,
    pub(crate) history_max: usize,
    /// ↑/↓ history walk: `Some(depth)` while a past user message is
    /// previewed (1 = most recent), plus the stashed live draft the walk
    /// restored or replaced. Spec 0018.
    pub(crate) hist_depth: Option<usize>,
    pub(crate) draft_stash: String,
    /// PATH executables for the command-name ghost layer, refreshed on a
    /// TTL rather than per keystroke.
    pub(crate) bin_cache: Vec<String>,
    pub(crate) bin_cache_at: Option<Instant>,
    /// Filesystem-derived value candidates (git refs, npm scripts, make
    /// targets, ssh hosts) behind the context-aware ghost layer.
    pub(crate) ctx_cache: crate::suggest::ContextCache,
    /// A failed shell line's proposed fix, offered until the next edit.
    /// `→` applies it into an empty composer; any edit drops it.
    pub(crate) pending_correction: Option<crate::suggest::Correction>,
    pub(crate) last_response: String,
    pub(crate) tx: mpsc::UnboundedSender<crate::ui::events::UiEvent>,
    pub(crate) rx: mpsc::UnboundedReceiver<crate::ui::events::UiEvent>,
    pub(crate) quit: bool,
    pub(crate) exit_notice: Option<String>,
    /// Accumulated session token usage for the footer telemetry.
    pub(crate) session_in: u64,
    pub(crate) session_out: u64,
    pub(crate) git_branch: Option<String>,
    pub(crate) skills_available: usize,
    pub(crate) attention_bell: bool,
    pub(crate) mouse_enabled: bool,
    /// `@file` completion state: selected index plus an input-keyed cache so
    /// the workspace walk only reruns when the composer text changes.
    pub(crate) at_selected: usize,
    pub(crate) at_cache_key: String,
    pub(crate) at_cache_items: Vec<String>,
    /// Markdown-backed custom commands (`/name` from `commands/*.md`),
    /// reloaded on `/config reload`, `/commands reload`, and workspace
    /// switches so new files never need a restart.
    pub(crate) custom_commands: Vec<CustomCommand>,
    /// First-argument value completion (`/theme <Tab>`): same input-keyed
    /// cache discipline as the `@` menu.
    pub(crate) arg_selected: usize,
    pub(crate) arg_cache_key: String,
    pub(crate) arg_cache_items: Vec<String>,
    /// Shell-line Tab menu (`!git check<Tab>`): unified history, spec,
    /// and file candidates with the same input-keyed cache discipline.
    pub(crate) sh_selected: usize,
    pub(crate) sh_cache_key: String,
    pub(crate) sh_cache_items: Vec<crate::suggest::ShellCandidate>,
    /// Tab opens the shell menu only when ambiguous (2+ rows); a
    /// single row applies directly. Sticky across edits until Esc,
    /// accept, or submit — the ghost stands down while it shows rows.
    pub(crate) sh_menu_invoked: bool,
    pub(crate) last_sh_rect: Option<Rect>,
    pub(crate) last_sh_count: usize,
    /// Model ids from the last `/models` refresh, backing `/model <Tab>`.
    pub(crate) known_models: Vec<String>,
    pub(crate) editor_requested: bool,
    /// Last drawn viewport heights/rects for viewport-aware paging and
    /// mouse hit-testing. Updated in `draw`; read by input handlers.
    pub(crate) transcript_height: u16,
    pub(crate) last_transcript_rect: Rect,
    pub(crate) last_composer_rect: Rect,
    pub(crate) last_palette_rect: Option<Rect>,
    pub(crate) last_palette_count: usize,
    /// Per rendered transcript line, the collapsible section id when the
    /// line is a section header. Lets a mouse click toggle a section.
    pub(crate) transcript_header_rows: Vec<Option<String>>,
    /// Scroll offset at render time, so a click row maps back to a line.
    pub(crate) last_transcript_scroll: usize,
    /// Left session pane: visibility, keyboard focus, selection, filter,
    /// cached saved sessions, recent workspaces, and the last drawn
    /// rect plus scroll for click mapping.
    pub(crate) sidebar_visible: bool,
    pub(crate) sidebar_focus: bool,
    pub(crate) sidebar_selected: usize,
    pub(crate) sidebar_scroll: usize,
    pub(crate) sidebar_filter: String,
    pub(crate) sidebar_sessions: Vec<crate::session::SessionInfo>,
    pub(crate) recent_workspaces: Vec<String>,
    pub(crate) last_sidebar_rect: Rect,
}

impl UiApp {
    pub(crate) fn new(
        backend: Backend,
        state: ChatState,
        paths: ConfigPaths,
        config: Config,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let plugins_dir = config.plugins_dir.clone();
        let sandbox = Sandbox::detect(
            &config.sandbox_backend,
            config.docker_image.clone(),
            config.timeout_seconds,
        );
        // Surface weak isolation up front: the rlimit/none fallback still
        // applies timeouts, output bounds, a sanitized env, and workspace
        // confinement, but it is not a namespace/container boundary.
        let mut status = match sandbox.selected_name() {
            "rlimit" | "none" => format!(
                "Ready · sandbox '{}' (limited) — /state for details",
                sandbox.selected_name()
            ),
            _ => "Ready".into(),
        };
        let invalid_bindings = config
            .keybindings
            .iter()
            .filter(|(action, spec)| is_known_key_action(action) && !valid_ctrl_spec(spec))
            .count();
        if invalid_bindings > 0 {
            status.push_str(&format!(
                " · {invalid_bindings} invalid keybinding(s) ignored (want ctrl+<letter>)"
            ));
        }
        // Hoisted: the struct literal below moves `state`, so ambient
        // workspace facts are read first.
        let git_branch = git_branch_for(&state.workspace);
        let skills_available = count_skills(&config.skills_dir);
        let custom_commands = custom::load_commands(
            &commands_dir(&paths),
            &project_commands_dir(&state.workspace),
        );
        if !custom_commands.is_empty() {
            status.push_str(&format!(
                " · {} custom command{} (/commands)",
                custom_commands.len(),
                if custom_commands.len() == 1 { "" } else { "s" }
            ));
        }
        let mut policy = crate::approve::Policy::from_config(&config).unwrap_or_else(|error| {
            status.push_str(&format!(
                " · approval policy invalid ({error:#}); tools locked down"
            ));
            crate::approve::Policy::locked_down()
        });
        // `a` verdicts from a previous run never carry over; the base
        // policy is always the config file.
        policy.session_allow.clear();
        // Shell history loads before the literal moves `paths`.
        let history_max = config.completion_history_max.max(1) as usize;
        let shell_history = crate::suggest::ShellHistory::load(
            &paths.config_dir.join("shell_history.json"),
            history_max,
        );
        let mut app = Self {
            backend,
            state,
            paths,
            plugins_dir,
            input: String::new(),
            cursor: 0,
            mode: Mode::Build,
            overlay: crate::ui::events::Overlay::None,
            palette_selected: 0,
            palette_scroll: 0,
            transcript_scroll: 0,
            follow_transcript: true,
            show_details: false,
            busy: false,
            streaming: String::new(),
            status,
            status_tone: crate::ui::events::StatusTone::Muted,
            request_started: None,
            awaiting_first_token: false,
            slow_hint_shown: false,
            queue: VecDeque::new(),
            recent_commands: VecDeque::new(),
            compact_backup: None,
            current_session: None,
            tabs: vec![tabs::Tab {
                session: None,
                title: "session".into(),
            }],
            active_tab: 0,
            last_tab_rect: Rect::default(),
            next_msg_id: 0,
            section_state: HashMap::new(),
            section_order: Vec::new(),
            block_filters: HashMap::new(),
            pending_context: None,
            redo_stack: Vec::new(),
            drop_next_restore: false,
            active_user: None,
            last_failed_prompt: None,
            tool_round: 0,
            cancellation: None,
            pending_connection: None,
            sandbox,
            policy,
            pending_tools: None,
            shared_todos: Arc::new(Mutex::new(Vec::new())),
            ghost_text: None,
            ghost_seen_input: String::new(),
            ghost_dismissed: None,
            ghost_changed_at: Instant::now(),
            ghost_debounce: Duration::from_millis(config.completion_debounce_ms),
            completion_on: config.completion_enabled,
            ai_suggest_on: config.ai_suggest,
            ai_ghost_seq: 0,
            ai_ghost_pending: None,
            shell_history,
            history_max,
            hist_depth: None,
            draft_stash: String::new(),
            bin_cache: Vec::new(),
            bin_cache_at: None,
            ctx_cache: crate::suggest::ContextCache::default(),
            pending_correction: None,
            last_response: String::new(),
            tx,
            rx,
            quit: false,
            exit_notice: None,
            session_in: 0,
            session_out: 0,
            git_branch,
            skills_available,
            attention_bell: config.attention_bell,
            mouse_enabled: config.mouse,
            at_selected: 0,
            at_cache_key: String::new(),
            at_cache_items: Vec::new(),
            custom_commands,
            arg_selected: 0,
            arg_cache_key: String::new(),
            arg_cache_items: Vec::new(),
            sh_selected: 0,
            sh_cache_key: String::new(),
            sh_cache_items: Vec::new(),
            sh_menu_invoked: false,
            last_sh_rect: None,
            last_sh_count: 0,
            known_models: Vec::new(),
            editor_requested: false,
            transcript_height: 20,
            last_transcript_rect: Rect::default(),
            last_composer_rect: Rect::default(),
            last_palette_rect: None,
            last_palette_count: 0,
            transcript_header_rows: Vec::new(),
            last_transcript_scroll: 0,
            sidebar_visible: false,
            sidebar_focus: false,
            sidebar_selected: 0,
            sidebar_scroll: 0,
            sidebar_filter: String::new(),
            sidebar_sessions: Vec::new(),
            recent_workspaces: Vec::new(),
            last_sidebar_rect: Rect::default(),
        };
        app.sync_mode_from_state();
        app.load_recent_workspaces();
        app.load_tabs();
        app
    }

    pub(crate) async fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        // Async input via EventStream + a redraw tick: no blocking poll on a
        // Tokio worker, no yield_now spin. Streaming tokens redraw on the
        // next 45ms tick at the latest.
        let mut reader = EventStream::new();
        let mut tick = tokio::time::interval(Duration::from_millis(45));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            self.process_events();
            self.maybe_note_slow_start();
            self.tick_ghost();
            terminal.draw(|frame| self.draw(frame))?;
            if self.editor_requested {
                self.editor_requested = false;
                if let Err(error) = self.run_editor(terminal).await {
                    self.set_error(format!("Editor failed: {error:#}"));
                }
            }
            if self.quit {
                if let Some(cancellation) = &self.cancellation {
                    cancellation.cancel();
                }
                if self.state.history.is_empty() {
                    self.exit_notice = None;
                } else {
                    match session::save(&self.paths, "__autosave__", &self.state) {
                        Ok(_) => {
                            self.exit_notice = Some(format!(
                                "Autosaved session '__autosave__' ({} messages)",
                                self.state.history.len()
                            ));
                        }
                        Err(error) => {
                            self.exit_notice =
                                Some(format!("warning: could not save autosession: {error}"));
                        }
                    }
                }
                break;
            }
            tokio::select! {
                biased;
                maybe = reader.next() => {
                    match maybe {
                        Some(Ok(Event::Key(key))) => self.handle_key(key).await?,
                        Some(Ok(Event::Mouse(mouse))) => self.handle_mouse(mouse),
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            self.set_error(format!("Input error: {error}"));
                        }
                        None => {
                            self.quit = true;
                        }
                    }
                }
                _ = tick.tick() => {}
            }
        }
        Ok(())
    }

    pub(crate) fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                crate::ui::events::UiEvent::Backend(BackendEvent::Token(token)) => {
                    self.streaming.push_str(&token);
                    self.awaiting_first_token = false;
                    self.set_status("Generating…".into());
                    self.follow_transcript = true;
                }
                crate::ui::events::UiEvent::Backend(BackendEvent::Status(status)) => {
                    self.set_status(status)
                }
                crate::ui::events::UiEvent::ChatDone(result) => self.chat_done(result),
                crate::ui::events::UiEvent::ChatError(error) => {
                    self.streaming.clear();
                    self.awaiting_first_token = false;
                    self.busy = false;
                    self.cancellation = None;
                    self.tool_round = 0;
                    self.pending_context = None;
                    let cancelled = error.contains("cancelled");
                    let prompt = self.active_user.take();
                    if self.drop_next_restore {
                        // Steering cancelled this request on purpose: the new
                        // prompt is already queued, so restore nothing.
                        self.drop_next_restore = false;
                    } else if let Some(prompt) = prompt {
                        self.last_failed_prompt = Some(prompt.clone());
                        // Restore the failed prompt so it is not lost; /retry reuses it.
                        self.input = prompt;
                        self.cursor = self.input.len();
                    }
                    // A failed compaction retries via `/compact` (history is
                    // intact), not via the chat `/retry` path.
                    let compact_failed = self.compact_backup.take().is_some();
                    self.set_error(if compact_failed {
                        format!("Compaction failed: {error} · /compact to retry")
                    } else {
                        format!("Request failed: {error} · /retry to try again")
                    });
                    let settled = self.queue.is_empty() && !cancelled;
                    self.start_next_queued();
                    if settled {
                        self.ring_bell();
                    }
                }
                crate::ui::events::UiEvent::ToolsDone(results) => {
                    self.awaiting_first_token = false;
                    for result in &results {
                        self.state.history.push(Message::tool(
                            result.call_id.clone(),
                            format!("[{}]\n{}", result.name, result.content),
                        ));
                    }
                    // `todo_write` ran inside the workers: adopt the list
                    // so the transcript section and footer render it.
                    if let Ok(todos) = self.shared_todos.lock() {
                        self.state.todos = todos.clone();
                    }
                    let names =
                        tool_names(&results.iter().map(|r| r.name.clone()).collect::<Vec<_>>());
                    self.set_status(format!(
                        "{} result(s) [{}] · continuing…",
                        results.len(),
                        names
                    ));
                    self.start_continue();
                }
                crate::ui::events::UiEvent::Compacted { summary, recent } => {
                    self.awaiting_first_token = false;
                    if self.apply_compaction(summary, recent) {
                        self.start_next_queued();
                    }
                }
                crate::ui::events::UiEvent::ModelsLoaded { backend, models } => {
                    self.backend = backend;
                    self.pending_connection = None;
                    if models.is_empty() {
                        self.set_status("Connected · no models listed".into());
                        self.persist_connection(None);
                    } else {
                        let cold = models
                            .iter()
                            .filter(|model| {
                                model
                                    .status
                                    .as_deref()
                                    .is_some_and(|status| status != "loaded")
                            })
                            .count();
                        self.set_status(if cold > 0 {
                            format!(
                                "Connected · {} models ({} load on first use)",
                                models.len(),
                                cold
                            )
                        } else {
                            format!("Connected · {} models", models.len())
                        });
                        let active = self.state.model.clone();
                        let selected = models
                            .iter()
                            .position(|model| model.id == active)
                            .unwrap_or(0);
                        self.known_models = models.iter().map(|model| model.id.clone()).collect();
                        self.overlay = crate::ui::events::Overlay::Models {
                            items: models,
                            selected,
                            scroll: 0,
                            active,
                        };
                    }
                }
                crate::ui::events::UiEvent::Notice(notice) => self.push_system(&notice),
                crate::ui::events::UiEvent::ShellCorrection {
                    failed,
                    fixed,
                    more,
                } => {
                    // Never clobber typing: an empty composer gets the
                    // one-keystroke offer, a busy one gets a transcript
                    // note pointing at the same fixes.
                    if self.input.trim().is_empty() {
                        self.pending_correction = Some(crate::suggest::Correction {
                            failed,
                            fixed: fixed.clone(),
                            more: more.clone(),
                        });
                        self.set_status(if more.is_empty() {
                            format!("Did you mean `!{fixed}`? → applies")
                        } else {
                            format!("Did you mean `!{fixed}`? → applies · +{} more", more.len())
                        });
                    } else {
                        self.push_system(&format!("Did you mean `!{fixed}`?"));
                    }
                }
                crate::ui::events::UiEvent::LiveValues { key, values, ok } => {
                    let live = &mut self.ctx_cache.live;
                    live.inflight.remove(&key);
                    if ok
                        && matches!(
                            key.as_str(),
                            "k8s-ns" | "k8s-pods" | "docker-containers" | "docker-images"
                        )
                    {
                        match key.as_str() {
                            "k8s-ns" => live.namespaces = values,
                            "k8s-pods" => live.pods = values,
                            "docker-containers" => live.containers = values,
                            "docker-images" => live.images = values,
                            _ => unreachable!("key allowlisted above"),
                        }
                        live.at.insert(key, Instant::now());
                    } else if !ok {
                        live.failed_at.insert(key, Instant::now());
                    }
                }
                crate::ui::events::UiEvent::AiGhost {
                    seq,
                    for_input,
                    suffix,
                } => {
                    // Stale flights die quietly: a newer keystroke owns
                    // the composer now, the local cascade wins any race,
                    // and a dismissal sticks.
                    let fresh = seq == self.ai_ghost_seq && for_input == self.input;
                    if seq == self.ai_ghost_seq {
                        self.ai_ghost_pending = None;
                    }
                    if fresh
                        && self.ghost_text.is_none()
                        && self.ghost_dismissed.as_deref() != Some(for_input.as_str())
                        && let Some(suffix) = suffix
                        && !suffix.is_empty()
                    {
                        self.ghost_text = Some(suffix);
                    }
                }
                crate::ui::events::UiEvent::ShellDraft(outcome) => match outcome {
                    Ok(command) if self.input.is_empty() => {
                        self.input = command;
                        self.cursor = self.input.len();
                        self.status = "Review — Enter runs · Esc clears".into();
                    }
                    Ok(command) => {
                        self.push_system(&format!("Composer busy — run it when ready:\n{command}"));
                        self.set_ok("Saved to session".into());
                    }
                    Err(error) => self.set_error(error),
                },
            }
        }
    }

    pub(crate) fn chat_done(&mut self, result: ChatResult) {
        self.streaming.clear();
        self.awaiting_first_token = false;
        self.state.last_usage = result.usage.clone();
        if let Some(tokens) = result.usage.prompt_tokens {
            self.session_in += tokens;
        }
        if let Some(tokens) = result.usage.completion_tokens {
            self.session_out += tokens;
        }
        if let Some(prompt) = self.active_user.take() {
            self.state.history.push(Message::user(prompt));
        }
        if let Some(context) = self.pending_context.take() {
            self.state
                .history
                .push(Message::system(format!("Attached context:\n{context}")));
        }
        self.state.history.push(Message::assistant_with_tools(
            result.content.clone(),
            result.tool_calls.clone(),
        ));
        self.last_response = result.content.clone();
        self.follow_transcript = true;

        if !result.tool_calls.is_empty() && self.tool_round < MAX_TOOL_ROUNDS {
            self.tool_round += 1;
            let names: Vec<String> = result
                .tool_calls
                .iter()
                .map(|c| c.function.name.clone())
                .collect();
            self.set_status(format!(
                "Running {} tool call(s) [{}]…",
                result.tool_calls.len(),
                tool_names(&names)
            ));
            let Some(cancellation) = self.cancellation.clone() else {
                self.busy = false;
                self.set_error("Tool execution aborted: missing cancellation token".into());
                self.start_next_queued();
                return;
            };
            let context = ToolContext {
                workspace: self.state.workspace.clone(),
                plugins_dir: self.plugins_dir.clone(),
                sandbox: self.sandbox.clone(),
                cancellation,
                allow_network: self.state.permission_posture != "restricted"
                    && self.state.permission_posture != "off",
                allow_code: self.state.permission_posture != "off",
                mode: self.state.mode.clone(),
                policy: self.policy.clone(),
                todos: self.shared_todos.clone(),
            };
            // Policy pre-check: auto-run, pre-deny, or pause on approval
            // cards. Enforcement re-runs inside execute(), so a card
            // approval cannot be bypassed by a later code path.
            self.precheck_tool_calls(result.tool_calls, context);
        } else {
            if !result.tool_calls.is_empty() {
                // Tool-round limit reached with pending calls: do not claim success.
                let names: Vec<String> = result
                    .tool_calls
                    .iter()
                    .map(|c| c.function.name.clone())
                    .collect();
                self.busy = false;
                self.cancellation = None;
                self.set_status(format!(
                    "Tool-round limit ({MAX_TOOL_ROUNDS}) reached; {} call(s) [{}] not executed",
                    result.tool_calls.len(),
                    tool_names(&names)
                ));
                self.push_system(&format!(
                    "Tool-round limit ({MAX_TOOL_ROUNDS}) reached. The model requested further tool calls that were not executed. Refine the prompt or continue manually."
                ));
                let settled = self.queue.is_empty();
                self.start_next_queued();
                if settled {
                    self.ring_bell();
                }
            } else if self.state.auto_compact
                && self.state.history.len() >= 4
                && self.state.token_usage().percent() >= 80.0
            {
                self.start_compaction(true);
            } else {
                self.busy = false;
                self.cancellation = None;
                self.set_ok(format!("Done in {:.2}s", result.wall_seconds));
                let settled = self.queue.is_empty();
                self.start_next_queued();
                if settled {
                    self.ring_bell();
                }
            }
        }
    }

    /// A request with no first token after a while is usually a cold model
    /// load, not a hang. Say so once per request instead of leaving the
    /// stale "Sending…" status up for a minute.
    pub(crate) fn maybe_note_slow_start(&mut self) {
        const SLOW_START_SECONDS: u64 = 15;
        if slow_start_due(
            self.busy,
            self.awaiting_first_token,
            self.slow_hint_shown,
            self.request_started,
            Instant::now(),
        ) {
            self.slow_hint_shown = true;
            self.set_status(format!(
                "Still waiting for the first token (>{SLOW_START_SECONDS}s) — the server may be loading the model; Esc cancels"
            ));
        }
    }

    pub(crate) fn start_prompt(&mut self, prompt: String, context: Option<String>) {
        if self.busy {
            self.queue.push_back((prompt, context));
            self.set_status(format!("Queued ({} waiting)", self.queue.len()));
            return;
        }
        self.busy = true;
        self.active_user = Some(prompt.clone());
        self.last_failed_prompt = None;
        self.pending_context = context;
        self.request_started = Some(Instant::now());
        self.awaiting_first_token = true;
        self.slow_hint_shown = false;
        self.state.last_usage = Usage::default();
        self.tool_round = 0;
        self.streaming.clear();
        self.set_status(format!("Sending in {} mode…", self.mode.as_str()));
        let cancellation = CancellationToken::new();
        self.cancellation = Some(cancellation.clone());
        let backend = self.backend.clone();
        let state = self.state.clone();
        let tools =
            tool::definitions_for_mode(tool::definitions_from(&self.plugins_dir), &self.state.mode);
        let sender = self.tx.clone();
        let (backend_sender, mut backend_events) = mpsc::unbounded_channel();
        let relay_sender = sender.clone();
        tokio::spawn(async move {
            while let Some(event) = backend_events.recv().await {
                let _ = relay_sender.send(crate::ui::events::UiEvent::Backend(event));
            }
        });
        tokio::spawn(async move {
            match backend
                .stream_chat(&state, &prompt, &tools, backend_sender, cancellation)
                .await
            {
                Ok(result) => {
                    let _ = sender.send(crate::ui::events::UiEvent::ChatDone(result));
                }
                Err(error) => {
                    let _ = sender.send(crate::ui::events::UiEvent::ChatError(error.to_string()));
                }
            }
        });
    }

    pub(crate) fn start_continue(&mut self) {
        let Some(cancellation) = self.cancellation.clone() else {
            self.busy = false;
            self.set_error("Nothing to continue".into());
            self.start_next_queued();
            return;
        };
        let backend = self.backend.clone();
        let state = self.state.clone();
        let tools =
            tool::definitions_for_mode(tool::definitions_from(&self.plugins_dir), &self.state.mode);
        let sender = self.tx.clone();
        let (backend_sender, mut backend_events) = mpsc::unbounded_channel();
        let relay_sender = sender.clone();
        tokio::spawn(async move {
            while let Some(event) = backend_events.recv().await {
                let _ = relay_sender.send(crate::ui::events::UiEvent::Backend(event));
            }
        });
        tokio::spawn(async move {
            match backend
                .stream_continue(&state, &tools, backend_sender, cancellation)
                .await
            {
                Ok(result) => {
                    let _ = sender.send(crate::ui::events::UiEvent::ChatDone(result));
                }
                Err(error) => {
                    let _ = sender.send(crate::ui::events::UiEvent::ChatError(error.to_string()));
                }
            }
        });
    }

    pub(crate) fn start_next_queued(&mut self) {
        if let Some((prompt, context)) = self.queue.pop_front() {
            self.start_prompt(prompt, context);
        }
    }

    pub(crate) fn ring_bell(&self) {
        if self.attention_bell {
            print!("\x07");
            let _ = io::stdout().flush();
        }
    }

    pub(crate) fn refresh_git_branch(&mut self) {
        self.git_branch = git_branch_for(&self.state.workspace);
    }

    pub(crate) fn refresh_skills(&mut self) {
        self.skills_available = count_skills(&self.state.skills_dir);
    }

    /// Reload Markdown-backed custom commands from the global and project
    /// directories. Built-in collisions are dropped here (built-ins win) and
    /// counted so `/commands` can report the shadowing instead of hiding it.
    pub(crate) fn refresh_custom_commands(&mut self) -> usize {
        let loaded = custom::load_commands(
            &commands_dir(&self.paths),
            &project_commands_dir(&self.state.workspace),
        );
        let shadowed = loaded
            .iter()
            .filter(|command| command::command(&format!("/{}", command.name)).is_some())
            .count();
        self.custom_commands = loaded;
        self.arg_cache_key.clear();
        shadowed
    }

    pub(crate) fn cancel_work(&mut self, status: &str) {
        // A card-paused round runs nothing: no event will settle it, so
        // cancel settles it here instead of stranding busy.
        if self.pending_tools.take().is_some() {
            self.overlay = crate::ui::events::Overlay::None;
            self.busy = false;
            self.cancellation = None;
            self.tool_round = 0;
            self.set_status(status.into());
            self.start_next_queued();
            return;
        }
        if let Some(token) = &self.cancellation {
            token.cancel();
            self.set_status(status.into());
        } else if !self.busy {
            self.set_status(status.into());
        }
    }
}

const MAX_TOOL_ROUNDS: usize = 8;

trait Expanduser {
    fn expanduser(self) -> Self;
}

impl Expanduser for PathBuf {
    fn expanduser(self) -> Self {
        if self == Path::new("~") {
            return std::env::var_os("HOME").map(PathBuf::from).unwrap_or(self);
        }
        if let Ok(stripped) = self.strip_prefix("~/")
            && let Some(home) = std::env::var_os("HOME")
        {
            return PathBuf::from(home).join(stripped);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::complete::score_file_candidate;
    use super::*;

    fn ctrl(letter: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(letter), KeyModifiers::CONTROL)
    }

    #[test]
    fn file_refs_extract_in_order_without_duplicates() {
        assert_eq!(
            extract_file_refs("see @src/main.rs and @Cargo.toml, also @src/main.rs"),
            vec!["src/main.rs".to_string(), "Cargo.toml".to_string()]
        );
    }

    #[test]
    fn file_refs_ignore_emails_and_bare_at() {
        assert!(extract_file_refs("mail user@host.com").is_empty());
        assert!(extract_file_refs("@").is_empty());
        assert!(extract_file_refs("a@b").is_empty());
    }

    #[test]
    fn file_candidates_rank_prefix_first() {
        let exact = score_file_candidate("src/main.rs", "src/main.rs").unwrap();
        let prefix = score_file_candidate("src/main.rs", "src/ma").unwrap();
        let filename = score_file_candidate("src/main.rs", "main").unwrap();
        let substring = score_file_candidate("src/main.rs", "rc/mai").unwrap();
        let fuzzy = score_file_candidate("src/main.rs", "mrs").unwrap();
        assert!(exact > prefix && prefix > filename);
        assert!(filename > substring && substring > fuzzy);
        assert!(score_file_candidate("src/main.rs", "zzz").is_none());
        assert!(score_file_candidate("anything", "").is_some());
    }

    #[test]
    fn shell_draft_cleaner_reduces_to_one_line() {
        assert_eq!(
            clean_shell_draft("```sh\nrg -n TODO src\n```\n"),
            Some("rg -n TODO src".to_string())
        );
        assert_eq!(clean_shell_draft("$ ls -la"), Some("ls -la".to_string()));
        assert_eq!(clean_shell_draft("```\n```"), None);
        assert_eq!(clean_shell_draft("   \n  "), None);
    }

    /// A live `UiApp` without I/O: the backend points at a closed
    /// loopback port (never dialed in these tests), the workspace and
    /// skills live in temp dirs, and the config dir is a process-local
    /// temp path so tests never read or write the real user config
    /// (shell history, recents, tabs).
    fn test_app() -> (UiApp, tempfile::TempDir, tempfile::TempDir) {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let skills = tempfile::TempDir::new().expect("skills");
        let config = Config {
            skills_dir: skills.path().to_path_buf(),
            ..Config::default()
        };
        let mut paths = ConfigPaths::discover();
        paths.config_dir = std::env::temp_dir().join(format!("r105-tests-{}", std::process::id()));
        paths.sessions_dir = paths.config_dir.join("sessions");
        let _ = std::fs::create_dir_all(&paths.config_dir);
        let state = ChatState::from_config(&config, workspace.path().to_path_buf());
        let connection = provider::resolve_connection(None, None, Some("http://127.0.0.1:9"));
        let backend = Backend::new(connection, 5).expect("backend");
        (UiApp::new(backend, state, paths, config), workspace, skills)
    }

    /// Card keys resolve in order: deny delivers without spawning,
    /// approve-once runs the call and merges back into call order.
    #[tokio::test]
    async fn approval_card_keys_resolve() {
        use crate::model::{FunctionCall, ToolCall};

        fn write_call(id: &str) -> ToolCall {
            ToolCall {
                id: id.to_string(),
                type_: "function".to_string(),
                function: FunctionCall {
                    name: "write_file".to_string(),
                    arguments: r#"{"path":"notes.txt","content":"hi"}"#.to_string(),
                },
            }
        }

        let (mut app, workspace, _skills) = test_app();
        let plugins_dir = app.plugins_dir.clone();
        let workspace_path = workspace.path().to_path_buf();
        let policy = app.policy.clone();
        let context = || ToolContext {
            workspace: workspace_path.clone(),
            plugins_dir: plugins_dir.clone(),
            sandbox: Sandbox::detect("none", None, 5),
            cancellation: CancellationToken::new(),
            allow_network: true,
            allow_code: true,
            mode: "build".to_string(),
            policy: policy.clone(),
            todos: Arc::new(Mutex::new(Vec::new())),
        };
        // Default config asks for writes: the call pauses on a card.
        app.precheck_tool_calls(vec![write_call("c1")], context());
        assert!(matches!(app.overlay, crate::ui::events::Overlay::Approval));
        let (name, summary, remaining) = app.approval_card().unwrap();
        assert_eq!(name, "write_file");
        assert!(summary.contains("notes.txt"), "{summary}");
        assert_eq!(remaining, 0);
        app.resolve_approval(ApprovalVerdict::Deny);
        assert!(matches!(app.overlay, crate::ui::events::Overlay::None));
        match app.rx.try_recv().expect("denial delivered") {
            crate::ui::events::UiEvent::ToolsDone(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].call_id, "c1");
                assert!(
                    results[0].content.contains("denied by user"),
                    "{}",
                    results[0].content
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
        // Approve-once runs the call for real and merges in order.
        app.precheck_tool_calls(vec![write_call("c2")], context());
        app.resolve_approval(ApprovalVerdict::Once);
        match tokio::time::timeout(std::time::Duration::from_secs(10), app.rx.recv())
            .await
            .expect("tools done arrives")
            .expect("channel open")
        {
            crate::ui::events::UiEvent::ToolsDone(results) => {
                assert_eq!(results.len(), 1);
                assert!(
                    results[0].content.contains("notes.txt"),
                    "{}",
                    results[0].content
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert!(workspace.path().join("notes.txt").exists());
    }

    fn test_custom(name: &str) -> CustomCommand {
        CustomCommand {
            name: name.into(),
            description: "Test command".into(),
            argument_hint: None,
            content: "Do $1".into(),
            source: "user".into(),
            path: PathBuf::from("/tmp/test.md"),
        }
    }

    /// Draw one frame headlessly and return each row with trailing
    /// whitespace trimmed. Tests mutate `UiApp` state directly and assert
    /// on the joined rows; cell styles stay reachable through a retained
    /// terminal when a color assertion is needed (see `render_terminal`).
    fn render_lines(app: &mut UiApp, width: u16, height: u16) -> Vec<String> {
        render_terminal(app, width, height)
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Same as [`render_lines`] but keeps the terminal so tests can
    /// inspect cell styles (status tone, selection highlight).
    fn render_terminal(
        app: &mut UiApp,
        width: u16,
        height: u16,
    ) -> ratatui::Terminal<ratatui::backend::TestBackend> {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.draw(frame)).expect("draw");
        terminal
    }

    #[test]
    fn harness_renders_status_and_composer_text() {
        let (mut app, _workspace, _skills) = test_app();
        app.status = "Hello status".into();
        app.input = "hello world".into();
        app.cursor = app.input.len();
        let joined = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            joined.contains("Hello status"),
            "status line missing:\n{joined}"
        );
        assert!(
            joined.contains("> hello world"),
            "composer text missing:\n{joined}"
        );
    }

    /// ↑/↓ walks user turns, stashes the live draft, clamps at the
    /// oldest, and restores the draft past the newest.
    #[test]
    fn history_walk_stashes_and_restores_draft() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("first"));
        app.state.history.push(Message::user("second"));
        app.input = "draft".into();
        app.cursor = app.input.len();

        app.history_previous();
        assert_eq!(app.input, "second");
        app.history_previous();
        assert_eq!(app.input, "first");
        app.history_previous();
        assert_eq!(app.input, "first", "walk clamps at the oldest turn");
        app.history_next();
        assert_eq!(app.input, "second");
        app.history_next();
        assert_eq!(app.input, "draft");
        assert_eq!(app.hist_depth, None);
        app.history_next();
        assert_eq!(app.input, "draft", "no walk: ↓ is a no-op");
    }

    /// A real edit adopts the previewed turn as the draft; Esc restores
    /// the stashed draft and ends the walk.
    #[test]
    fn history_walk_edit_adopts_and_esc_restores() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("sent"));
        app.input = "draft".into();
        app.cursor = app.input.len();

        app.history_previous();
        app.insert_text("x");
        assert_eq!(app.input, "sentx");
        assert_eq!(app.hist_depth, None);

        app.input = "draft".into();
        app.cursor = app.input.len();
        app.history_previous();
        app.end_history_walk(false);
        assert_eq!(app.input, "draft");
        assert_eq!(app.hist_depth, None);
    }

    /// Single-owner rule: palette, `@` token, and argument menus all
    /// clear and suppress the ghost.
    #[test]
    fn menus_suppress_ghost() {
        let (mut app, _workspace, _skills) = test_app();
        app.completion_on = true;
        app.ghost_debounce = Duration::ZERO;
        app.shell_history.record("git status", "/repo");
        app.bin_cache_at = Some(Instant::now());

        app.input = "!gi".into();
        app.cursor = app.input.len();
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("t status"));

        // Slash palette open (`/th`): ghost cleared, nothing resolved.
        app.input = "/th".into();
        app.cursor = app.input.len();
        app.tick_ghost();
        assert!(app.ghost_text.is_none());
        assert!(app.menu_wants_input());

        // Live `@token` suppresses.
        app.input = "see @Cargo".into();
        app.cursor = app.input.len();
        assert!(app.menu_wants_input());

        // Argument position with candidates suppresses (`/theme d`).
        app.input = "/theme d".into();
        app.cursor = app.input.len();
        assert!(app.menu_wants_input());

        // Plain `!` argument territory does not.
        app.input = "!git sta".into();
        app.cursor = app.input.len();
        assert!(!app.menu_wants_input());
    }

    /// Empty-composer hint: idle coaching is a display-only string,
    /// never part of the input; an active walk retitles the composer.
    #[test]
    fn empty_composer_shows_teaching_hint() {
        let (mut app, _workspace, _skills) = test_app();
        let joined = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            joined.contains("Ask anything · shell runs"),
            "idle hint missing:\n{joined}"
        );
        assert!(app.input.is_empty());

        app.state.history.push(Message::user("old"));
        app.history_previous();
        app.input.clear();
        app.cursor = 0;
        let walking = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            walking.contains("Esc restores"),
            "walk title missing:\n{walking}"
        );
    }

    /// Warp-style Enter: bare shell-looking lines run (with a shell
    /// title on the composer), prose still reaches the model.
    #[tokio::test]
    async fn enter_runs_bare_shell_lines() {
        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        let learned = app.shell_history.len();
        app.input = "echo hi".to_string();
        app.cursor = app.input.len();
        let rendered = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            rendered.contains("Shell · Enter runs"),
            "shell affordance missing:\n{rendered}"
        );
        app.submit().await.unwrap();
        assert!(app.input.is_empty());
        assert!(app.status.starts_with("Running:"), "status: {}", app.status);
        assert!(
            app.state.history.iter().any(|m| m.content == "echo hi"),
            "command not in transcript"
        );
        assert_eq!(
            app.shell_history.len(),
            learned + 1,
            "history teaches the ghost"
        );

        // Prose stays a prompt: the composer clears for a request.
        app.input = "explain this codebase".to_string();
        app.cursor = app.input.len();
        app.submit().await.unwrap();
        assert!(app.busy, "prose must start a request");
    }

    /// Bare `cd` retargets the workspace like a terminal; a missing
    /// directory errors without touching it.
    #[tokio::test]
    async fn enter_cd_changes_workspace() {
        let (mut app, workspace, _skills) = test_app();
        let sub = workspace.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        app.input = "cd sub".to_string();
        app.cursor = app.input.len();
        app.submit().await.unwrap();
        assert_eq!(app.state.workspace, std::fs::canonicalize(&sub).unwrap());
        assert!(
            app.status.starts_with("Workspace:"),
            "status: {}",
            app.status
        );
        app.input = "cd nope".to_string();
        app.cursor = app.input.len();
        app.submit().await.unwrap();
        assert_eq!(app.state.workspace, std::fs::canonicalize(&sub).unwrap());
        assert!(
            app.status.contains("no such directory"),
            "status: {}",
            app.status
        );
    }

    /// The composer carries a visible reversed cursor cell.
    #[test]
    fn composer_draws_cursor_cell() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "git status".to_string();
        app.cursor = 4;
        let terminal = render_terminal(&mut app, 80, 24);
        let reversed = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .count();
        assert!(reversed >= 1, "cursor cell missing");
    }

    /// `/filter` parsing: flags, `#n` form, clear, bad regex, missing
    /// context value, out-of-range blocks.
    #[test]
    fn block_filter_parses() {
        let args = |parts: &[&str]| parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_block_filter(3, &[]).unwrap(), FilterAction::List);
        match parse_block_filter(3, &args(&["#2", "error", "--context", "1"])).unwrap() {
            FilterAction::Set(index, filter) => {
                assert_eq!(index, 1);
                assert_eq!(filter.context, 1);
                assert_eq!(filter.describe(), "\"error\" · ctx 1");
            }
            other => panic!("{other:?}"),
        }
        match parse_block_filter(3, &args(&["2"])).unwrap() {
            FilterAction::Clear(index) => assert_eq!(index, 1),
            other => panic!("{other:?}"),
        }
        assert!(parse_block_filter(3, &args(&["9", "x"])).is_err());
        assert!(parse_block_filter(3, &args(&["1", "[", "--regex"])).is_err());
        assert!(parse_block_filter(3, &args(&["1", "--context"])).is_err());
        assert!(parse_block_filter(0, &args(&["1", "x"])).is_err());
    }

    /// `/filter` application: hits, context windows, invert, regex, case,
    /// and the all-hidden case.
    #[test]
    fn block_filter_keeps_hits_and_context() {
        let body = "alpha\nbeta error\ngamma\ndelta error\nepsilon";
        let base = BlockFilter {
            pattern: "error".into(),
            regex: false,
            case: false,
            invert: false,
            context: 0,
        };
        let filtered = apply_block_filter(body, &base);
        assert_eq!(filtered.shown, vec!["beta error", "delta error"]);
        assert_eq!(filtered.hidden, 3);

        let contextual = BlockFilter {
            context: 1,
            ..base.clone()
        };
        let filtered = apply_block_filter(body, &contextual);
        assert_eq!(filtered.shown.len(), 5);
        assert_eq!(filtered.hidden, 0);

        let inverted = BlockFilter {
            invert: true,
            ..base.clone()
        };
        assert_eq!(
            apply_block_filter(body, &inverted).shown,
            vec!["alpha", "gamma", "epsilon"]
        );

        let regex = BlockFilter {
            pattern: "err.r".into(),
            regex: true,
            ..base.clone()
        };
        assert_eq!(apply_block_filter(body, &regex).shown.len(), 2);

        let case = BlockFilter {
            pattern: "ERROR".into(),
            case: true,
            ..base
        };
        let all_hidden = apply_block_filter(body, &case);
        assert!(all_hidden.shown.is_empty());
        assert_eq!(all_hidden.hidden, 5);
    }

    /// `/filter` stores by message id and bare `/filter <n>` clears,
    /// while `/block n` reports the filter.
    #[test]
    fn filter_command_stores_and_clears() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("hello"));
        let id = app.section_id(0);
        app.command_filter(&["1".into(), "err".into()]);
        assert!(app.block_filters.contains_key(&id));
        app.command_filter(&["#1".into()]);
        assert!(app.block_filters.is_empty());
    }

    /// `/copy out` payload: verbatim without a filter, filtered view
    /// with one.
    #[test]
    fn copy_payload_respects_filter() {
        let body = "keep\nnoise\nkeep too";
        assert_eq!(block_copy_content(body, None), body);
        let filter = BlockFilter {
            pattern: "keep".into(),
            regex: false,
            case: false,
            invert: false,
            context: 0,
        };
        assert_eq!(block_copy_content(body, Some(&filter)), "keep\nkeep too");
    }

    /// Rerun resolves explicit blocks, defaults to the last user turn,
    /// and refuses non-user blocks and out-of-range numbers.
    #[test]
    fn rerun_resolves_user_blocks_only() {
        let history = vec![
            Message::user("first"),
            Message::assistant_with_tools("ok", vec![]),
            Message::user("!ls -la"),
        ];
        assert_eq!(rerun_target(&history, None).unwrap(), "!ls -la");
        assert_eq!(rerun_target(&history, Some(1)).unwrap(), "first");
        assert!(rerun_target(&history, Some(2)).is_err());
        assert!(rerun_target(&history, Some(9)).is_err());
        assert!(rerun_target(&[], None).is_err());
    }

    /// `/expand #n` maps a block to its section; failed tool blocks and
    /// non-section blocks refuse with a status note.
    #[test]
    fn expand_accepts_block_address() {
        let (mut app, _workspace, _skills) = test_app();
        app.state
            .history
            .push(Message::tool("call-1", "tool error: boom"));
        let id = app.section_id(0);
        app.section_order = vec![(id.clone(), false)];
        app.command_expand(&["#1".into()]);
        assert!(
            !app.section_state.contains_key(&id),
            "failed tool block must stay expanded"
        );

        app.state.history[0].content = "clean output".into();
        app.command_expand(&["#1".into()]);
        assert_eq!(app.section_state.get(&id), Some(&true));

        app.state.history.push(Message::user("hi"));
        app.command_expand(&["#2".into()]);
        assert!(
            app.status.contains("not a collapsible section"),
            "status: {}",
            app.status
        );
    }

    /// The transcript renders block addresses as `#n` so `/filter`,
    /// `/block`, and `/rerun` targets are visible.
    #[test]
    fn transcript_renders_block_numbers() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("hello"));
        app.state
            .history
            .push(Message::assistant_with_tools("hi", vec![]));
        let joined = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            joined.contains("USER #1"),
            "block gutter missing:\n{joined}"
        );
        assert!(
            joined.contains("ASSISTANT #2"),
            "block gutter missing:\n{joined}"
        );
    }

    /// The task list renders as a collapsible TASKS section and the
    /// footer counts progress; collapsing uses the same section state
    /// as `/expand`.
    #[test]
    fn todo_render_section_collapses() {
        use crate::model::{TodoItem, TodoStatus};

        let (mut app, _workspace, _skills) = test_app();
        app.state.todos = vec![
            TodoItem {
                content: "first".to_string(),
                status: TodoStatus::Completed,
            },
            TodoItem {
                content: "second".to_string(),
                status: TodoStatus::InProgress,
            },
        ];
        let joined = render_lines(&mut app, 80, 30).join("\n");
        assert!(joined.contains("TASKS"), "section missing:\n{joined}");
        assert!(joined.contains("✓ first"), "done marker missing:\n{joined}");
        assert!(
            joined.contains("▶ second"),
            "active marker missing:\n{joined}"
        );
        assert!(
            joined.contains("tasks 1/2"),
            "footer count missing:\n{joined}"
        );
        app.section_state.insert("todos".to_string(), false);
        let collapsed = render_lines(&mut app, 80, 30).join("\n");
        assert!(
            collapsed.contains("1/2 done"),
            "collapsed summary missing:\n{collapsed}"
        );
        assert!(
            !collapsed.contains("✓ first"),
            "collapsed section leaks items:\n{collapsed}"
        );
    }

    /// The tick resolves history ghosts synchronously; dismissal
    /// sticks until the next edit, acceptance chains continuations.
    #[test]
    fn ghost_tick_suggests_from_history() {
        let (mut app, workspace, _skills) = test_app();
        let cwd = workspace.path().to_string_lossy().to_string();
        app.shell_history.record("git status", &cwd);
        app.shell_history.record("git status", &cwd);
        app.input = "!git sta".to_string();
        app.ghost_debounce = Duration::ZERO;
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("tus"));
    }

    #[test]
    fn ghost_dismiss_stays_dismissed() {
        let (mut app, workspace, _skills) = test_app();
        let cwd = workspace.path().to_string_lossy().to_string();
        app.shell_history.record("git status", &cwd);
        app.input = "!git sta".to_string();
        app.ghost_debounce = Duration::ZERO;
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("tus"));
        assert!(app.dismiss_ghost());
        app.tick_ghost();
        assert!(app.ghost_text.is_none(), "dismiss must stick");
        app.input = "!git statu".to_string();
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("s"));
    }

    #[tokio::test]
    async fn right_accepts_ghost_ctrl_right_takes_word() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills) = test_app();
        // Plain → at the end takes the whole ghost.
        app.input = "!git sta".to_string();
        app.cursor = app.input.len();
        app.ghost_text = Some("tus".to_string());
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        app.handle_key(right).await.unwrap();
        assert_eq!(app.input, "!git status");
        // Ctrl+→ takes one word; the rest stays offered.
        app.input = "!git check".to_string();
        app.cursor = app.input.len();
        app.ghost_text = Some("out main".to_string());
        let ctrl_right = KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL);
        app.handle_key(ctrl_right).await.unwrap();
        assert_eq!(app.input, "!git checkout");
        // Mid-line → still moves the cursor, never eats the ghost.
        app.cursor = 2;
        app.ghost_text = Some("out".to_string());
        app.handle_key(right).await.unwrap();
        assert_eq!(app.cursor, 3);
        assert_eq!(app.ghost_text.as_deref(), Some("out"));
    }

    #[tokio::test]
    async fn correction_applies_on_right_clears_on_edit() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills) = test_app();
        // The worker's offer lands as a one-keystroke hint.
        app.tx
            .send(crate::ui::events::UiEvent::ShellCorrection {
                failed: "gti status".to_string(),
                fixed: "git status".to_string(),
                more: vec!["get status".to_string()],
            })
            .unwrap();
        app.process_events();
        assert!(app.pending_correction.is_some());
        assert!(
            app.status.contains("Did you mean"),
            "status: {}",
            app.status
        );
        // → takes it into an empty composer.
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        app.handle_key(right).await.unwrap();
        assert_eq!(app.input, "!git status");
        assert!(app.pending_correction.is_none());
        // A fresh offer dies on the next edit instead of lingering.
        app.input.clear();
        app.cursor = 0;
        app.pending_correction = Some(crate::suggest::Correction {
            failed: "gti status".to_string(),
            fixed: "git status".to_string(),
            more: Vec::new(),
        });
        app.insert_text("x");
        assert!(app.pending_correction.is_none());
    }

    #[test]
    fn ghost_tick_completes_branch_from_workspace_git() {
        let (mut app, workspace, _skills) = test_app();
        let git = workspace.path().join(".git").join("refs").join("heads");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("main"), "abc").unwrap();
        std::fs::write(
            workspace.path().join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .unwrap();
        app.state.workspace = workspace.path().to_path_buf();
        app.input = "!git checkout ma".to_string();
        app.ghost_debounce = Duration::ZERO;
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("in"));
    }

    /// No marker needed: a plain `git sta` line ghosts like `!git sta`,
    /// and Tab on a bare command word opens its subcommand menu.
    #[test]
    fn marker_free_lines_ghost_and_menu_like_bang_lines() {
        let (mut app, workspace, _skills) = test_app();
        let cwd = workspace.path().to_string_lossy().to_string();
        app.shell_history.record("git status", &cwd);
        app.ghost_debounce = Duration::ZERO;
        app.input = "git sta".to_string();
        app.cursor = app.input.len();
        app.tick_ghost();
        assert_eq!(app.ghost_text.as_deref(), Some("tus"));
        app.input = "git".to_string();
        app.cursor = app.input.len();
        assert!(app.sh_tab(), "Tab on a bare command word opens the menu");
        let items = app.sh_menu_items();
        assert!(
            items
                .iter()
                .any(|item| item.text == "git status" && item.whole_line),
            "whole-line subcommand row missing: {items:?}"
        );
        app.close_sh_menu();
        // Prose never starves the mode cycle: Tab stays Tab.
        app.input = "hello".to_string();
        app.cursor = app.input.len();
        assert!(!app.sh_tab());
    }

    /// Model ghosts land only for the live generation and never clobber
    /// a local ghost or a dismissal.
    #[test]
    fn ai_ghost_applies_only_when_fresh() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "docker ps".to_string();
        app.cursor = app.input.len();
        app.ai_ghost_seq = 2;
        let ghost = |seq, input: &str, suffix: &str| crate::ui::events::UiEvent::AiGhost {
            seq,
            for_input: input.to_string(),
            suffix: Some(suffix.to_string()),
        };
        app.tx.send(ghost(1, "docker ps", " -a")).unwrap();
        app.process_events();
        assert!(app.ghost_text.is_none(), "stale generation dropped");
        app.tx.send(ghost(2, "docker ps", " -a")).unwrap();
        app.process_events();
        assert_eq!(app.ghost_text.as_deref(), Some(" -a"));
        // A dismissal owns the input until the next edit.
        assert!(app.dismiss_ghost());
        app.tx.send(ghost(2, "docker ps", " -a")).unwrap();
        app.process_events();
        assert!(app.ghost_text.is_none());
        // A local ghost wins any race.
        app.ai_ghost_seq = 3;
        app.ghost_text = Some(" --all".to_string());
        app.tx.send(ghost(3, "docker ps", " -a")).unwrap();
        app.process_events();
        assert_eq!(app.ghost_text.as_deref(), Some(" --all"));
    }

    /// Daemon round-trips land as cache values (or backoff marks); the
    /// keystroke path only reads them.
    #[test]
    fn live_values_events_update_cache() {
        let (mut app, _workspace, _skills) = test_app();
        app.tx
            .send(crate::ui::events::UiEvent::LiveValues {
                key: "k8s-pods".to_string(),
                values: vec!["api-0".to_string()],
                ok: true,
            })
            .unwrap();
        app.process_events();
        assert_eq!(app.ctx_cache.live.pods, vec!["api-0".to_string()]);
        assert!(app.ctx_cache.live.at.contains_key("k8s-pods"));
        app.tx
            .send(crate::ui::events::UiEvent::LiveValues {
                key: "docker-images".to_string(),
                values: Vec::new(),
                ok: false,
            })
            .unwrap();
        app.process_events();
        assert!(app.ctx_cache.live.failed_at.contains_key("docker-images"));
        assert!(!app.ctx_cache.live.inflight.contains("docker-images"));
    }

    /// Sidebar tests run against temp session/config dirs: `test_app`
    /// discovers the real config paths, which tests must never write.
    fn sidebar_app() -> (
        UiApp,
        tempfile::TempDir,
        tempfile::TempDir,
        tempfile::TempDir,
    ) {
        let (mut app, workspace, skills) = test_app();
        let dirs = tempfile::TempDir::new().expect("sidebar dirs");
        app.paths.sessions_dir = dirs.path().join("sessions");
        app.paths.config_dir = dirs.path().join("config");
        app.recent_workspaces.clear();
        (app, workspace, skills, dirs)
    }

    #[tokio::test]
    async fn ctrl_b_toggles_sidebar_esc_unfocuses() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        let toggle = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        app.handle_key(toggle).await.unwrap();
        assert!(app.sidebar_visible && app.sidebar_focus);
        // Focused toggle hides outright.
        app.handle_key(toggle).await.unwrap();
        assert!(!app.sidebar_visible && !app.sidebar_focus);
        // Visible-but-idle toggle refocuses instead of hiding.
        app.handle_key(toggle).await.unwrap();
        app.unfocus_sidebar();
        assert!(app.sidebar_visible && !app.sidebar_focus);
        app.handle_key(toggle).await.unwrap();
        assert!(app.sidebar_visible && app.sidebar_focus);
        // Esc leaves the pane visible but returns keys to the composer.
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc).await.unwrap();
        assert!(app.sidebar_visible && !app.sidebar_focus);
        assert!(app.sidebar_filter.is_empty());
    }

    #[test]
    fn sidebar_rows_filter_sessions_and_workspaces() {
        let (mut app, workspace, _skills, _dirs) = sidebar_app();
        crate::session::save(&app.paths, "alpha", &app.state).unwrap();
        crate::session::save(&app.paths, "beta", &app.state).unwrap();
        app.refresh_sidebar();
        let live = workspace.path().to_string_lossy().to_string();
        let rows = app.sidebar_rows();
        // `list` order is recency-based, so only the shape is stable:
        // New first, the live workspace last, both saves between.
        assert_eq!(rows.first(), Some(&super::sidebar::SidebarRow::New));
        assert_eq!(
            rows.last(),
            Some(&super::sidebar::SidebarRow::Workspace { path: live })
        );
        let mut saved: Vec<String> = rows
            .iter()
            .filter_map(|row| match row {
                super::sidebar::SidebarRow::Session { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        saved.sort();
        assert_eq!(saved, vec!["alpha".to_string(), "beta".to_string()]);
        // A filter hides `+ New` and non-matching rows alike.
        app.sidebar_filter = "alpha".into();
        assert_eq!(
            app.sidebar_rows(),
            vec![super::sidebar::SidebarRow::Session {
                name: "alpha".into(),
                messages: 0,
            }]
        );
        app.sidebar_filter = "zzz-no-match".into();
        assert!(app.sidebar_rows().is_empty());
    }

    #[test]
    fn sidebar_new_load_delete_roundtrip() {
        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        app.state.history.push(Message::user("hello"));
        app.sidebar_new();
        assert!(app.state.history.is_empty());
        // Fresh sessions are unnamed, but the previous work autosaved.
        assert!(app.current_session.is_none());
        assert!(
            crate::session::list(&app.paths)
                .iter()
                .any(|info| info.name.starts_with("autosave-")),
            "previous transcript autosaved"
        );
        // Named save, wipe, reload through the pane.
        app.state.history.push(Message::user("work"));
        crate::session::save(&app.paths, "proj", &app.state).unwrap();
        app.state.history.clear();
        app.sidebar_load("proj");
        assert_eq!(app.state.history.len(), 1);
        assert_eq!(app.current_session.as_deref(), Some("proj"));
        // Deleting the loaded session removes the file but keeps the
        // live transcript, unnamed.
        let index = app
            .sidebar_rows()
            .iter()
            .position(|row| {
                matches!(row, super::sidebar::SidebarRow::Session { name, .. } if name == "proj")
            })
            .expect("proj row");
        app.sidebar_selected = index;
        app.sidebar_delete_selected();
        assert!(
            !crate::session::list(&app.paths)
                .iter()
                .any(|info| info.name == "proj")
        );
        assert_eq!(app.state.history.len(), 1);
        assert!(app.current_session.is_none());
    }

    /// Tabs are session-backed: new stashes the live session, switching
    /// reloads each tab's transcript, closing falls back to its neighbor.
    #[test]
    fn tabs_new_switch_close_roundtrip() {
        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        app.state.history.push(Message::user("first"));
        crate::session::save(&app.paths, "alpha", &app.state).unwrap();
        app.current_session = Some("alpha".into());
        app.tab_new();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);
        assert!(app.state.history.is_empty(), "new tab starts fresh");
        assert_eq!(app.tabs[0].session.as_deref(), Some("alpha"));
        app.state.history.push(Message::user("second"));
        crate::session::save(&app.paths, "beta", &app.state).unwrap();
        app.current_session = Some("beta".into());
        // Switching back reloads the first tab's transcript.
        app.tab_switch(0);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.current_session.as_deref(), Some("alpha"));
        assert_eq!(app.state.history.len(), 1);
        assert_eq!(app.state.history[0].content, "first");
        // Cycle forward and close the second tab.
        app.tab_next(true);
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.current_session.as_deref(), Some("beta"));
        app.tab_close();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.current_session.as_deref(), Some("alpha"));
        assert_eq!(app.state.history[0].content, "first");
        // The last tab never closes.
        app.tab_close();
        assert_eq!(app.tabs.len(), 1);
    }

    /// Tab keys: Ctrl+Shift+T/W, Ctrl+Tab, Alt+1..9; clicks resolve
    /// against the drawn bar.
    #[tokio::test]
    async fn tab_keys_and_click_map_to_tabs() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        let ctrl_shift = |code: char| {
            KeyEvent::new(
                KeyCode::Char(code),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )
        };
        app.handle_key(ctrl_shift('T')).await.unwrap();
        assert_eq!(app.tabs.len(), 2);
        let ctrl_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL);
        app.handle_key(ctrl_tab).await.unwrap();
        assert_eq!(app.active_tab, 0);
        app.handle_key(ctrl_tab).await.unwrap();
        assert_eq!(app.active_tab, 1);
        let alt_one = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
        app.handle_key(alt_one).await.unwrap();
        assert_eq!(app.active_tab, 0);
        // Bar geometry: ` r105 ` (6 cells), then ` 1 label ` chips.
        app.current_session = Some("alpha".into());
        app.last_tab_rect = Rect::new(0, 0, 80, 1);
        assert_eq!(app.tab_hit(8, 0), Some(0));
        assert_eq!(app.tab_hit(18, 0), Some(1));
        assert_eq!(app.tab_hit(2, 0), None);
        // The `+` chip opens a tab on click.
        let plus_col = (0..60)
            .find(|column| app.tab_plus_hit(*column, 0))
            .expect("plus cell");
        app.handle_mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: plus_col,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.tabs.len(), 3, "click on + opens a tab");
        app.handle_key(ctrl_shift('W')).await.unwrap();
        app.handle_key(ctrl_shift('W')).await.unwrap();
        assert_eq!(app.tabs.len(), 1);
        // Plain Ctrl+W still deletes a word.
        app.input = "one two".into();
        app.cursor = app.input.len();
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        app.handle_key(ctrl_w).await.unwrap();
        assert_eq!(app.input, "one ");
    }

    #[test]
    fn recent_workspaces_pin_live_and_persist() {
        let (mut app, workspace, _skills, _dirs) = sidebar_app();
        let other = tempfile::TempDir::new().expect("other workspace");
        app.note_workspace(other.path());
        let live = workspace.path().to_string_lossy().to_string();
        let other_path = other.path().to_string_lossy().to_string();
        // Live first, recents after, no duplicates.
        assert_eq!(
            app.sidebar_rows(),
            vec![
                super::sidebar::SidebarRow::New,
                super::sidebar::SidebarRow::Workspace { path: live },
                super::sidebar::SidebarRow::Workspace {
                    path: other_path.clone()
                },
            ]
        );
        app.note_workspace(other.path());
        assert_eq!(app.recent_workspaces.len(), 1, "re-switch dedupes");
        // A fresh load restores the persisted recents.
        app.recent_workspaces.clear();
        app.load_recent_workspaces();
        assert_eq!(app.recent_workspaces, vec![other_path]);
    }

    #[test]
    fn sidebar_draws_pane_beside_transcript() {
        let (mut app, _workspace, _skills, _dirs) = sidebar_app();
        app.toggle_sidebar();
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.draw(frame)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(text.contains("SESSIONS"), "pane header missing");
        assert!(text.contains("New session"), "new row missing");
        assert!(text.contains("WORKSPACES"), "workspace group missing");
        assert!(!app.last_sidebar_rect.is_empty(), "click rect untracked");
    }

    #[tokio::test]
    async fn ghost_tab_accepts_esc_dismisses() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills) = test_app();
        // Certain Tab (one row, no ghost): applies at once.
        app.input = "!git statu".to_string();
        app.cursor = app.input.len();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        app.handle_key(tab).await.unwrap();
        assert_eq!(app.input, "!git status");
        // A fresh ghost dismisses on Esc without touching the request.
        app.ghost_text = Some(" --help".to_string());
        app.busy = false;
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc).await.unwrap();
        assert!(app.ghost_text.is_none());
        assert!(!app.busy);
    }

    #[tokio::test]
    async fn shell_tab_opens_menu_when_ambiguous() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills) = test_app();
        app.input = "!git sta".to_string();
        app.cursor = app.input.len();
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        // `status` and `stash` both match: first Tab opens, input stays.
        app.handle_key(tab).await.unwrap();
        assert_eq!(app.input, "!git sta");
        assert!(app.sh_menu_invoked);
        assert!(app.sh_menu_open());
        // Second Tab accepts the selected row.
        app.handle_key(tab).await.unwrap();
        assert_eq!(app.input, "!git status");
        assert!(!app.sh_menu_invoked);
        // Reopen, then Esc closes without accepting.
        app.input = "!git sta".to_string();
        app.cursor = app.input.len();
        app.handle_key(tab).await.unwrap();
        assert!(app.sh_menu_invoked);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_key(esc).await.unwrap();
        assert!(!app.sh_menu_invoked);
        assert_eq!(app.input, "!git sta");
    }

    /// Shrinking widgets must not leave stale glyphs: draw with content,
    /// clear it, draw again on the same terminal, and prove the old text
    /// is gone.
    #[test]
    fn redraw_clears_shrunk_transcript() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("old leftover line"));
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.draw(frame)).expect("first draw");
        app.state.history.clear();
        terminal.draw(|frame| app.draw(frame)).expect("second draw");
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
        }
        assert!(
            !text.contains("old leftover"),
            "stale glyphs survived redraw"
        );
    }

    /// `/state` renders short labeled lines, never a raw dump.
    #[test]
    fn state_renders_labeled_lines() {
        let (mut app, _workspace, _skills) = test_app();
        app.command_state();
        let content = app
            .state
            .history
            .last()
            .expect("state note")
            .content
            .clone();
        assert!(content.starts_with("Mode: "), "{content}");
        assert!(content.contains("Approvals: "), "{content}");
        assert!(!content.contains("mode="), "{content}");
        assert!(content.lines().count() <= 6, "{content}");
    }

    #[test]
    fn status_error_renders_red() {
        let (mut app, _workspace, _skills) = test_app();
        app.set_error("Error demo".to_string());
        assert_eq!(app.status_tone, crate::ui::events::StatusTone::Error);
        app.set_ok("Ok demo".to_string());
        assert_eq!(app.status_tone, crate::ui::events::StatusTone::Success);
        app.set_error("Error demo".to_string());
        let terminal = render_terminal(&mut app, 80, 24);
        let buffer = terminal.backend().buffer();
        // Footer takes the last two rows; the status span starts at x=0
        // of the first footer row with one leading space, so x=2 is the
        // second status character.
        let cell = &buffer.content()[22 * 80 + 2];
        assert_eq!(cell.symbol(), "r");
        assert_eq!(cell.fg, Color::Red);
    }

    /// Isolated session dirs for tests that write checkpoints: the
    /// default `test_app` paths point at the real user config.
    fn test_paths(root: &std::path::Path) -> ConfigPaths {
        ConfigPaths {
            home: root.to_path_buf(),
            config_dir: root.join("config"),
            config_file: root.join("config/config.json"),
            sessions_dir: root.join("config/sessions"),
            plugins_dir: root.join("config/plugins"),
        }
    }

    fn system_notes(app: &UiApp) -> Vec<String> {
        app.state
            .history
            .iter()
            .filter(|message| message.role == "system")
            .map(|message| message.content.clone())
            .collect()
    }

    fn user_messages(app: &UiApp) -> Vec<String> {
        app.state
            .history
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.clone())
            .collect()
    }

    #[test]
    fn push_system_folds_consecutive_duplicates() {
        let (mut app, _workspace, _skills) = test_app();
        let before = system_notes(&app).len();
        app.push_system("Same note");
        app.push_system("Same note");
        app.push_system("Same note");
        let folded = system_notes(&app);
        assert_eq!(folded.len(), before + 1);
        assert_eq!(folded[before], "Same note (×3)");
        // A different note breaks the run; the earlier text returns plain.
        app.push_system("Other note");
        app.push_system("Same note");
        let after = system_notes(&app);
        assert_eq!(
            after[before..],
            vec!["Same note (×3)", "Other note", "Same note"]
        );
    }

    #[test]
    fn split_repeat_suffix_parses_counts() {
        assert_eq!(split_repeat_suffix("plain"), ("plain", 1));
        assert_eq!(split_repeat_suffix("note (×2)"), ("note", 2));
        assert_eq!(split_repeat_suffix("note (×12)"), ("note", 12));
        // Malformed tails are left alone rather than mis-folded.
        assert_eq!(split_repeat_suffix("note (×)"), ("note (×)", 1));
        assert_eq!(split_repeat_suffix("note (×x)"), ("note (×x)", 1));
    }

    #[test]
    fn expand_toggles_single_tool_section() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::user("do things"));
        app.state
            .history
            .push(Message::tool("call-1", "line1\nline2\nline3"));
        // Collapsed by default: only the first line shows, with a marker.
        let collapsed = render_lines(&mut app, 80, 24).join("\n");
        assert!(
            collapsed.contains("TOOL [1]"),
            "gutter missing:\n{collapsed}"
        );
        assert!(collapsed.contains("▸[1]"), "marker missing:\n{collapsed}");
        assert!(!collapsed.contains("line2"), "should start collapsed");
        app.command_expand(&["1".to_string()]);
        let expanded = render_lines(&mut app, 80, 24).join("\n");
        assert!(expanded.contains("line2"), "toggle did not expand");
        assert!(expanded.contains("line3"), "toggle did not expand");
        // Toggling again collapses back to the summary row.
        app.command_expand(&["1".to_string()]);
        let again = render_lines(&mut app, 80, 24).join("\n");
        assert!(!again.contains("line2"), "second toggle did not collapse");
    }

    #[test]
    fn expand_all_none_and_failed_guard() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.history.push(Message::tool("call-1", "ok1\nok1b"));
        app.state
            .history
            .push(Message::tool("call-2", "tool error: boom\ndetail"));
        let _ = render_lines(&mut app, 80, 24);
        assert_eq!(app.section_order.len(), 2);
        // The failed result reports that it stays expanded.
        app.command_expand(&["2".to_string()]);
        assert!(
            app.status.contains("stays expanded"),
            "guard missing: {}",
            app.status
        );
        let rendered = render_lines(&mut app, 80, 24).join("\n");
        assert!(rendered.contains("detail"), "failed result must expand");
        // `none` collapses the healthy section but not the failed one.
        app.command_expand(&["none".to_string()]);
        let collapsed = render_lines(&mut app, 80, 24).join("\n");
        assert!(!collapsed.contains("ok1b"), "healthy section not collapsed");
        assert!(
            collapsed.contains("detail"),
            "failed result must stay expanded"
        );
        app.command_expand(&["all".to_string()]);
        let all = render_lines(&mut app, 80, 24).join("\n");
        assert!(all.contains("ok1b"), "all did not expand");
        // Bare `/expand` toggles the most recent section (the failed one
        // refuses, so point the check at a healthy-only transcript).
        let (mut solo, _workspace, _skills) = test_app();
        solo.state
            .history
            .push(Message::tool("call-9", "solo1\nsolo2"));
        let _ = render_lines(&mut solo, 80, 24);
        solo.command_expand(&[]);
        let bare = render_lines(&mut solo, 80, 24).join("\n");
        assert!(
            bare.contains("solo2"),
            "bare expand did not toggle last section"
        );
    }

    #[test]
    fn rewind_truncates_and_pushes_redo() {
        let (mut app, _workspace, _skills) = test_app();
        let sessions = tempfile::TempDir::new().expect("sessions");
        app.paths = test_paths(sessions.path());
        app.state.history.push(Message::user("first"));
        app.state
            .history
            .push(Message::assistant_with_tools("answer one", vec![]));
        app.state.history.push(Message::user("second"));
        app.state
            .history
            .push(Message::assistant_with_tools("answer two", vec![]));
        app.command_rewind(&[]);
        let users = user_messages(&app);
        assert_eq!(users, vec!["first"]);
        assert_eq!(app.redo_stack.len(), 1);
        assert!(app.status.contains("backup checkpoint-rewind-"));
        // The boundary notice names the backup for a later restore.
        let notes = system_notes(&app);
        assert!(
            notes
                .last()
                .is_some_and(|note| note.contains("backup checkpoint-rewind-")),
            "boundary notice missing: {notes:?}"
        );
        // `/redo` re-applies the dropped turn.
        app.command_redo();
        assert_eq!(user_messages(&app), vec!["first", "second"]);
        // Two turns back drops every user message.
        app.command_rewind(&["2".to_string()]);
        assert!(user_messages(&app).is_empty());
        // Nothing left to rewind.
        app.command_rewind(&[]);
        assert_eq!(app.status, "Nothing to rewind");
    }

    #[test]
    fn rewind_writes_checkpoint_backup() {
        let (mut app, _workspace, _skills) = test_app();
        let sessions = tempfile::TempDir::new().expect("sessions");
        app.paths = test_paths(sessions.path());
        app.state.history.push(Message::user("keep me"));
        app.state
            .history
            .push(Message::assistant_with_tools("kept", vec![]));
        app.command_rewind(&[]);
        assert!(app.state.history.len() <= 1);
        let backup = session::list(&app.paths)
            .into_iter()
            .map(|item| item.name)
            .find(|name| name.starts_with("checkpoint-rewind-"))
            .expect("checkpoint backup");
        app.state.history.clear();
        let count = session::load(&app.paths, &backup, &mut app.state).unwrap();
        assert_eq!(count, 2);
        assert_eq!(app.state.history[0].content, "keep me");
    }

    #[test]
    fn compact_split_keeps_tool_pairs_intact() {
        // Naive last-third splitting (len 9, keep 3) would cut at index 6,
        // stranding tool results whose call was summarized away.
        let history = vec![
            Message::user("u1"),
            Message::assistant_with_tools("a1", vec![]),
            Message::tool("c1", "r1"),
            Message::user("u2"),
            Message::assistant_with_tools("a2", vec![]),
            Message::tool("c2", "r2"),
            Message::tool("c3", "r3"),
            Message::user("u3"),
            Message::assistant_with_tools("a3", vec![]),
        ];
        assert_eq!(history[6].role, "tool");
        let (older, recent) = split_compact(&history);
        assert_eq!(older.len() + recent.len(), 9);
        assert_ne!(
            recent.first().map(|message| message.role.as_str()),
            Some("tool"),
            "kept tail starts mid-exchange"
        );
        assert!(recent.iter().any(|message| message.content == "a2"));
    }

    #[test]
    fn compact_rejects_empty_summary() {
        let (mut app, _workspace, _skills) = test_app();
        for index in 0..5 {
            app.state.history.push(Message::user(format!("m{index}")));
        }
        let before = app.state.history.clone();
        let recent = before[3..].to_vec();
        assert!(!app.apply_compaction(String::new(), recent.clone()));
        assert_eq!(app.state.history, before);
        assert!(
            app.status.contains("empty summary"),
            "status: {}",
            app.status
        );
        assert!(app.apply_compaction("summary text".to_string(), recent));
        assert_eq!(app.state.history.len(), 3);
        assert!(app.state.history[0].content.contains("summary text"));
    }

    #[test]
    fn fork_with_turns_snaps_to_boundary() {
        use crate::model::{FunctionCall, ToolCall};

        let (mut app, _workspace, _skills) = test_app();
        let sessions = tempfile::TempDir::new().expect("sessions");
        app.paths = test_paths(sessions.path());
        let call = ToolCall {
            id: "c1".to_string(),
            type_: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        app.state.history.push(Message::user("first"));
        app.state
            .history
            .push(Message::assistant_with_tools("answer", vec![call]));
        app.state.history.push(Message::tool("c1", "result"));
        app.state.history.push(Message::user("second"));
        app.state
            .history
            .push(Message::assistant_with_tools("later", vec![]));
        app.command_session(&["fork".to_string(), "child".to_string(), "1".to_string()]);
        assert!(app.status.contains("Forked first 1 turn(s) as child"));
        let mut forked = app.state.clone();
        forked.history.clear();
        let count = session::load(&app.paths, "child", &mut forked).unwrap();
        assert_eq!(count, 3);
        assert_eq!(forked.history[2].content, "result");
        // The fork records its source (none yet: nothing saved or loaded).
        let info = session::list(&app.paths)
            .into_iter()
            .find(|item| item.name == "child")
            .expect("forked session");
        assert_eq!(info.parent, None);
    }

    /// `#` is natural-language command search: the composer clears and a
    /// command draft is requested; a bare `#` teaches the usage.
    #[tokio::test]
    async fn hash_describes_a_command_draft() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "#list large files".to_string();
        app.submit().await.unwrap();
        assert!(app.input.is_empty());
        assert!(app.status.contains("Drafting"), "status: {}", app.status);
        app.input = "#".to_string();
        app.submit().await.unwrap();
        assert!(app.input.is_empty());
        assert!(
            app.status.contains("describe what you want"),
            "status: {}",
            app.status
        );
    }

    #[test]
    fn harness_renders_palette_rows_for_slash_query() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "/the".into();
        app.cursor = app.input.len();
        let joined = render_lines(&mut app, 80, 24).join("\n");
        assert!(joined.contains("/theme"), "palette row missing:\n{joined}");
        assert!(
            joined.contains("commands ·"),
            "palette frame missing:\n{joined}"
        );
    }

    #[test]
    fn palette_recency_boosts_repeated_command() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "/".into();
        app.cursor = app.input.len();
        let before: Vec<String> = app
            .palette_items()
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert!(before.iter().any(|name| name == "/tokens"));
        app.note_recent("/tokens");
        // A typo is never recorded, so it cannot pollute the boost.
        app.note_recent("/tokns");
        let after: Vec<String> = app
            .palette_items()
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert_eq!(after[0], "/tokens");
        assert!(!after.contains(&"/tokns".to_string()));
    }

    #[test]
    fn palette_shows_live_theme_badge() {
        let (mut app, _workspace, _skills) = test_app();
        app.state.theme = "dracula".to_string();
        app.input = "/the".into();
        app.cursor = app.input.len();
        let theme = app
            .palette_items()
            .into_iter()
            .find(|item| item.name == "/theme")
            .expect("theme row");
        assert!(
            theme.description.contains("now dracula"),
            "badge missing: {}",
            theme.description
        );
        // The `/build` row carries `· active` in the default build mode.
        app.input = "/".into();
        let build = app
            .palette_items()
            .into_iter()
            .find(|item| item.name == "/build")
            .expect("build row");
        assert!(
            build.description.contains("active"),
            "mode badge missing: {}",
            build.description
        );
    }

    #[test]
    fn arg_menu_completes_static_values_and_accepts() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "/theme dr".into();
        app.cursor = app.input.len();
        assert_eq!(app.arg_menu_items(), vec!["dracula".to_string()]);
        assert!(app.accept_arg_complete());
        assert_eq!(app.input, "/theme dracula ");
    }

    #[test]
    fn arg_menu_lists_all_values_on_empty_token() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "/reasoning ".into();
        app.cursor = app.input.len();
        let items = app.arg_menu_items();
        assert_eq!(items.len(), 5);
        assert!(items.contains(&"low".to_string()));
    }

    #[test]
    fn arg_menu_completes_skill_names_in_second_position() {
        let (mut app, _workspace, skills) = test_app();
        std::fs::write(skills.path().join("review.md"), "Review $1").unwrap();
        app.input = "/skill use ".into();
        app.cursor = app.input.len();
        assert_eq!(app.arg_menu_items(), vec!["review".to_string()]);
    }

    #[test]
    fn arg_menu_stays_shut_for_plain_text() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "just typing".into();
        app.cursor = app.input.len();
        assert!(app.arg_menu_items().is_empty());
    }

    #[test]
    fn palette_lists_custom_commands_with_marker() {
        let (mut app, _workspace, _skills) = test_app();
        app.custom_commands = vec![test_custom("review")];
        app.input = "/rev".into();
        let items = app.palette_items();
        let found = items
            .iter()
            .find(|item| item.name == "/review")
            .expect("custom row");
        assert!(found.custom);
    }

    #[tokio::test]
    async fn custom_command_dispatch_expands_and_submits() {
        let (mut app, _workspace, _skills) = test_app();
        app.custom_commands = vec![test_custom("review")];
        let parsed = command::parse("/review scope").expect("parses");
        app.handle_command(parsed).await.expect("dispatches");
        assert_eq!(app.active_user.as_deref(), Some("Do scope"));
        assert_eq!(
            app.status,
            "Expanded /review (user) · Sending in build mode…"
        );
    }

    #[tokio::test]
    async fn unknown_command_suggests_closest_match() {
        let (mut app, _workspace, _skills) = test_app();
        let parsed = command::parse("/modell").expect("parses");
        app.handle_command(parsed).await.expect("dispatches");
        assert_eq!(app.status, "Unknown command /modell; did you mean /model?");
    }

    #[test]
    fn thinking_body_only_matches_whole_message_wrappers() {
        assert_eq!(
            thinking_body("<thinking>\n  weigh options\n</thinking>"),
            Some("weigh options")
        );
        assert_eq!(thinking_body("plain reply"), None);
        // Mixed content is ordinary model text, never a thinking block.
        assert_eq!(thinking_body("intro\n<thinking>\nweigh\n</thinking>"), None);
        assert_eq!(thinking_body("<thinking>unclosed"), None);
    }

    #[test]
    fn code_blocks_extract_fenced_sections() {
        let text = "intro\n```rust\nlet x = 1;\n```\nmid\n```\nplain\n```\ntail";
        assert_eq!(
            code_blocks(text),
            vec!["let x = 1;".to_string(), "plain".to_string()]
        );
        assert!(code_blocks("no fences").is_empty());
        assert!(code_blocks("```\nunclosed").is_empty());
    }

    #[test]
    fn compact_numbers_use_k_and_m_suffixes() {
        assert_eq!(compact_number(999), "999");
        assert_eq!(compact_number(1_500), "1.5k");
        assert_eq!(compact_number(2_500_000), "2.5M");
    }

    #[test]
    fn ctrl_specs_match_exact_modifiers_case_insensitively() {
        assert!(match_ctrl_spec("ctrl+x", &ctrl('x')));
        assert!(match_ctrl_spec("ctrl+x", &ctrl('X')));
        assert!(match_ctrl_spec(" ctrl+o ", &ctrl('o')));
        assert!(!match_ctrl_spec(
            "ctrl+x",
            &KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
        ));
        assert!(!match_ctrl_spec("ctrl+x", &ctrl('y')));
        assert!(!valid_ctrl_spec("alt+x"));
        assert!(!valid_ctrl_spec("ctrl+"));
        assert!(!valid_ctrl_spec("ctrl+xy"));
        assert!(!valid_ctrl_spec("ctrl+1"));
    }

    #[test]
    fn known_key_actions_cover_the_remappable_set() {
        for action in ["cancel", "details", "tasks", "history", "redraw", "sidebar"] {
            assert!(is_known_key_action(action));
        }
        assert!(!is_known_key_action("quit"));
    }

    #[test]
    fn model_list_keeps_router_load_status() {
        let value = serde_json::json!({
            "data": [
                {"id": "b-model", "status": {"value": "unloaded", "args": ["x"]}},
                {"id": "a-model", "status": {"value": "loaded"}},
                {"id": "c-model", "status": "loading"},
                {"id": "plain"},
                "legacy-string",
                {"id": "b-model", "status": {"value": "loaded"}},
            ]
        });
        let models = extract_models(&value);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a-model", "b-model", "c-model", "legacy-string", "plain"]
        );
        let status = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .and_then(|model| model.status.clone())
        };
        assert_eq!(status("a-model").as_deref(), Some("loaded"));
        // First occurrence wins on duplicates.
        assert_eq!(status("b-model").as_deref(), Some("unloaded"));
        assert_eq!(status("c-model").as_deref(), Some("loading"));
        assert_eq!(status("plain"), None);
    }

    #[test]
    fn model_display_marks_active_and_status() {
        let loaded = ModelInfo {
            id: "a".into(),
            status: Some("loaded".into()),
        };
        assert_eq!(loaded.display("a"), "● a (active) · loaded");
        assert_eq!(loaded.display("b"), "  a · loaded");
        let unknown = ModelInfo {
            id: "a".into(),
            status: None,
        };
        assert_eq!(unknown.display("b"), "  a");
    }

    #[test]
    fn slow_start_hint_fires_once_after_fifteen_silent_seconds() {
        let now = Instant::now();
        let silent_long = Some(now - Duration::from_secs(20));
        // Fires while busy, awaiting, unshown, and slow.
        assert!(slow_start_due(true, true, false, silent_long, now));
        // Not before the threshold.
        assert!(!slow_start_due(
            true,
            true,
            false,
            Some(now - Duration::from_secs(5)),
            now
        ));
        // Never twice, never idle, never after the first token, never dateless.
        assert!(!slow_start_due(true, true, true, silent_long, now));
        assert!(!slow_start_due(false, true, false, silent_long, now));
        assert!(!slow_start_due(true, false, false, silent_long, now));
        assert!(!slow_start_due(true, true, false, None, now));
    }

    #[test]
    fn word_boundaries_skip_whitespace_and_words() {
        assert_eq!(prev_word_boundary("git status", 10), 4);
        assert_eq!(prev_word_boundary("git status", 4), 0);
        assert_eq!(prev_word_boundary("git status", 3), 0);
        assert_eq!(next_word_boundary("git status", 0), 3);
        assert_eq!(next_word_boundary("git status", 3), 10);
        assert_eq!(page_step(24), 22);
        assert_eq!(page_step(2), 3);
    }

    #[test]
    fn paired_input_closes_and_steps_over() {
        let (mut app, _workspace, _skills) = test_app();
        app.insert_paired('(');
        assert_eq!(app.input, "()");
        assert_eq!(app.cursor, 1);
        app.insert_paired(')');
        assert_eq!(app.input, "()");
        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn delete_word_and_line_edits() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "git status".into();
        app.cursor = app.input.len();
        app.delete_prev_word();
        assert_eq!(app.input, "git ");
        app.delete_to_start();
        assert_eq!(app.input, "");
        app.input = "git status".into();
        app.cursor = 3;
        app.delete_to_end();
        assert_eq!(app.input, "git");
    }

    #[tokio::test]
    async fn ctrl_p_toggles_action_palette() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _workspace, _skills) = test_app();
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert_eq!(app.input, "/");
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL))
            .await
            .unwrap();
        assert!(app.input.is_empty());
    }

    #[tokio::test]
    async fn mouse_command_toggles_capture() {
        let (mut app, _workspace, _skills) = test_app();
        assert!(app.mouse_enabled);
        let parsed = command::parse("/mouse off").expect("parses");
        app.handle_command(parsed).await.expect("dispatches");
        assert!(!app.mouse_enabled);
        assert_eq!(app.status, "Mouse off");
    }

    #[tokio::test]
    async fn workflows_lists_saved_items() {
        let (mut app, _workspace, _skills) = test_app();
        app.custom_commands = vec![test_custom("review")];
        let parsed = command::parse("/workflows").expect("parses");
        app.handle_command(parsed).await.expect("dispatches");
        let content = app
            .state
            .history
            .last()
            .expect("workflows note")
            .content
            .clone();
        assert!(content.contains("Saved workflows"), "{content}");
        assert!(content.contains("/review"), "{content}");
    }
}
