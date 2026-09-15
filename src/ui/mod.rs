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

mod commands;
mod complete;
mod input;
mod render;
mod transcript;

pub(crate) use commands::*;
pub(crate) use input::*;
pub(crate) use transcript::*;

#[derive(Debug)]
enum UiEvent {
    Backend(BackendEvent),
    ChatDone(ChatResult),
    ChatError(String),
    ToolsDone(Vec<ToolResult>),
    Compacted {
        summary: String,
        recent: Vec<Message>,
    },
    /// A `/sh` draft round-trip finished: prefill the composer with the
    /// proposed `!command` (`Ok`) or report why drafting failed (`Err`).
    ShellDraft(Result<String, String>),
    ModelsLoaded {
        backend: Backend,
        models: Vec<ModelInfo>,
    },
    Notice(String),
}

/// One entry of a provider model list. `status` carries the backend's load
/// state (`loaded`, `unloaded`, …) when the provider reports one; most
/// OpenAI-compatible endpoints omit it entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelInfo {
    id: String,
    status: Option<String>,
}

impl ModelInfo {
    pub(crate) fn display(&self, active: &str) -> String {
        let marker = if self.id == active { "●" } else { " " };
        let mut text = if self.id == active {
            format!("{marker} {} (active)", self.id)
        } else {
            format!("{marker} {}", self.id)
        };
        if let Some(status) = &self.status {
            text.push_str(&format!(" · {status}"));
        }
        text
    }
}

#[derive(Debug)]
enum Overlay {
    None,
    Providers {
        selected: usize,
        scroll: usize,
    },
    Models {
        items: Vec<ModelInfo>,
        selected: usize,
        scroll: usize,
        active: String,
    },
    ApiKey {
        provider: String,
    },
    CustomUrl {
        provider: String,
    },
    Theme {
        selected: usize,
        original: String,
    },
    Settings {
        selected: usize,
    },
}

/// One undoable exchange: everything from a user message onward. The prompt
/// itself is `messages[0]`, so `/undo` can restore it into the composer.
#[derive(Debug, Clone)]
struct UndoEntry {
    messages: Vec<Message>,
}

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

/// Severity of the footer status line. The line is a single superseding
/// slot (a new note always replaces the old one); the tone only colors
/// it so failures stop looking like idle notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum StatusTone {
    #[default]
    Muted,
    Success,
    Error,
}

impl StatusTone {
    pub(crate) fn color(self) -> Color {
        match self {
            StatusTone::Muted => Color::White,
            StatusTone::Success => Color::Green,
            StatusTone::Error => Color::Red,
        }
    }
}

struct UiApp {
    pub(crate) backend: Backend,
    pub(crate) state: ChatState,
    pub(crate) paths: ConfigPaths,
    pub(crate) plugins_dir: PathBuf,
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) mode: Mode,
    pub(crate) overlay: Overlay,
    pub(crate) palette_selected: usize,
    pub(crate) palette_scroll: usize,
    pub(crate) transcript_scroll: usize,
    pub(crate) follow_transcript: bool,
    pub(crate) show_details: bool,
    pub(crate) busy: bool,
    pub(crate) streaming: String,
    pub(crate) status: String,
    pub(crate) status_tone: StatusTone,
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
    /// File context resolved from `@refs`, pushed as a system message next
    /// to the user message once the backend answers.
    pub(crate) pending_context: Option<String>,
    /// Undone exchanges, newest last; any new user prompt clears the stack.
    pub(crate) redo_stack: Vec<UndoEntry>,
    /// Set by steering: the in-flight request is cancelled and its prompt
    /// must not be restored into the composer or offered via `/retry`.
    pub(crate) drop_next_restore: bool,
    pub(crate) active_user: Option<String>,
    pub(crate) last_failed_prompt: Option<String>,
    pub(crate) tool_round: usize,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) pending_connection: Option<Connection>,
    pub(crate) sandbox: Sandbox,
    pub(crate) last_response: String,
    pub(crate) tx: mpsc::UnboundedSender<UiEvent>,
    pub(crate) rx: mpsc::UnboundedReceiver<UiEvent>,
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
    /// Model ids from the last `/models` refresh, backing `/model <Tab>`.
    pub(crate) known_models: Vec<String>,
    pub(crate) editor_requested: bool,
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
                "Ready · sandbox '{}' fallback — install nsjail/bwrap/docker for stronger isolation (/state)",
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
        Self {
            backend,
            state,
            paths,
            plugins_dir,
            input: String::new(),
            cursor: 0,
            mode: Mode::Build,
            overlay: Overlay::None,
            palette_selected: 0,
            palette_scroll: 0,
            transcript_scroll: 0,
            follow_transcript: true,
            show_details: false,
            busy: false,
            streaming: String::new(),
            status,
            status_tone: StatusTone::Muted,
            request_started: None,
            awaiting_first_token: false,
            slow_hint_shown: false,
            queue: VecDeque::new(),
            recent_commands: VecDeque::new(),
            compact_backup: None,
            current_session: None,
            next_msg_id: 0,
            section_state: HashMap::new(),
            section_order: Vec::new(),
            pending_context: None,
            redo_stack: Vec::new(),
            drop_next_restore: false,
            active_user: None,
            last_failed_prompt: None,
            tool_round: 0,
            cancellation: None,
            pending_connection: None,
            sandbox,
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
            known_models: Vec::new(),
            editor_requested: false,
        }
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
                            self.set_error(format!("input error: {error}"));
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
                UiEvent::Backend(BackendEvent::Token(token)) => {
                    self.streaming.push_str(&token);
                    self.awaiting_first_token = false;
                    self.set_status("Generating…".into());
                    self.follow_transcript = true;
                }
                UiEvent::Backend(BackendEvent::Status(status)) => self.set_status(status),
                UiEvent::ChatDone(result) => self.chat_done(result),
                UiEvent::ChatError(error) => {
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
                UiEvent::ToolsDone(results) => {
                    self.awaiting_first_token = false;
                    for result in &results {
                        self.state.history.push(Message::tool(
                            result.call_id.clone(),
                            format!("[{}]\n{}", result.name, result.content),
                        ));
                    }
                    let names =
                        tool_names(&results.iter().map(|r| r.name.clone()).collect::<Vec<_>>());
                    self.set_status(format!(
                        "{} tool result(s) [{}] received; continuing…",
                        results.len(),
                        names
                    ));
                    self.start_continue();
                }
                UiEvent::Compacted { summary, recent } => {
                    self.awaiting_first_token = false;
                    if self.apply_compaction(summary, recent) {
                        self.start_next_queued();
                    }
                }
                UiEvent::ModelsLoaded { backend, models } => {
                    self.backend = backend;
                    self.pending_connection = None;
                    if models.is_empty() {
                        self.set_status("Connected; provider returned no model list".into());
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
                                "Connected; choose a model ({} available, {} unloaded — first use loads them)",
                                models.len(),
                                cold
                            )
                        } else {
                            format!("Connected; choose a model ({} available)", models.len())
                        });
                        let active = self.state.model.clone();
                        let selected = models
                            .iter()
                            .position(|model| model.id == active)
                            .unwrap_or(0);
                        self.known_models = models.iter().map(|model| model.id.clone()).collect();
                        self.overlay = Overlay::Models {
                            items: models,
                            selected,
                            scroll: 0,
                            active,
                        };
                    }
                }
                UiEvent::Notice(notice) => self.push_system(&notice),
                UiEvent::ShellDraft(outcome) => match outcome {
                    Ok(command) if self.input.is_empty() => {
                        self.input = format!("!{command}");
                        self.cursor = self.input.len();
                        self.status =
                            "Review the proposed command · Enter to run · Esc clears".into();
                    }
                    Ok(command) => {
                        self.push_system(&format!(
                            "Proposed shell command (composer was busy, run it with !):\n{command}"
                        ));
                        self.set_ok("Draft landed as a transcript note".into());
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
            };
            let calls = result.tool_calls;
            let sender = self.tx.clone();
            tokio::spawn(async move {
                match tool::execute_calls(&calls, &context, None).await {
                    Ok(results) => {
                        let _ = sender.send(UiEvent::ToolsDone(results));
                    }
                    Err(error) => {
                        let _ =
                            sender.send(UiEvent::ChatError(format!("tool execution: {error:#}")));
                    }
                }
            });
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
            self.set_status(format!("Queued prompt ({} waiting)", self.queue.len()));
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
        let tools = tool::definitions_from(&self.plugins_dir);
        let sender = self.tx.clone();
        let (backend_sender, mut backend_events) = mpsc::unbounded_channel();
        let relay_sender = sender.clone();
        tokio::spawn(async move {
            while let Some(event) = backend_events.recv().await {
                let _ = relay_sender.send(UiEvent::Backend(event));
            }
        });
        tokio::spawn(async move {
            match backend
                .stream_chat(&state, &prompt, &tools, backend_sender, cancellation)
                .await
            {
                Ok(result) => {
                    let _ = sender.send(UiEvent::ChatDone(result));
                }
                Err(error) => {
                    let _ = sender.send(UiEvent::ChatError(error.to_string()));
                }
            }
        });
    }

    pub(crate) fn start_continue(&mut self) {
        let Some(cancellation) = self.cancellation.clone() else {
            self.busy = false;
            self.set_error("Done; continuation unavailable (no active request)".into());
            self.start_next_queued();
            return;
        };
        let backend = self.backend.clone();
        let state = self.state.clone();
        let tools = tool::definitions_from(&self.plugins_dir);
        let sender = self.tx.clone();
        let (backend_sender, mut backend_events) = mpsc::unbounded_channel();
        let relay_sender = sender.clone();
        tokio::spawn(async move {
            while let Some(event) = backend_events.recv().await {
                let _ = relay_sender.send(UiEvent::Backend(event));
            }
        });
        tokio::spawn(async move {
            match backend
                .stream_continue(&state, &tools, backend_sender, cancellation)
                .await
            {
                Ok(result) => {
                    let _ = sender.send(UiEvent::ChatDone(result));
                }
                Err(error) => {
                    let _ = sender.send(UiEvent::ChatError(error.to_string()));
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
    /// skills live in temp dirs, and config discovery only reads env.
    fn test_app() -> (UiApp, tempfile::TempDir, tempfile::TempDir) {
        let workspace = tempfile::TempDir::new().expect("workspace");
        let skills = tempfile::TempDir::new().expect("skills");
        let config = Config {
            skills_dir: skills.path().to_path_buf(),
            ..Config::default()
        };
        let paths = ConfigPaths::discover();
        let state = ChatState::from_config(&config, workspace.path().to_path_buf());
        let connection = provider::resolve_connection(None, None, Some("http://127.0.0.1:9"));
        let backend = Backend::new(connection, 5).expect("backend");
        (UiApp::new(backend, state, paths, config), workspace, skills)
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

    #[test]
    fn status_error_renders_red() {
        let (mut app, _workspace, _skills) = test_app();
        app.set_error("Error demo".to_string());
        assert_eq!(app.status_tone, StatusTone::Error);
        app.set_ok("Ok demo".to_string());
        assert_eq!(app.status_tone, StatusTone::Success);
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

    #[test]
    fn hash_prefix_prefills_shell_for_ls() {
        let (mut app, _workspace, _skills) = test_app();
        app.submit_classified("ls -la".to_string());
        assert_eq!(app.input, "!ls -la");
        assert!(
            app.status.contains("Looks like shell"),
            "status: {}",
            app.status
        );
        app.submit_classified("tests".to_string());
        assert_eq!(app.input, "tests");
        assert!(app.status.contains("Ambiguous"), "status: {}", app.status);
        app.submit_classified(String::new());
        assert!(app.input.is_empty());
    }

    #[tokio::test]
    async fn hash_prefix_submits_question_to_agent() {
        let (mut app, _workspace, _skills) = test_app();
        app.input = "#what is this?".to_string();
        app.submit_classified("what is this?".to_string());
        // The prompt left the composer for the (test-backend) request path.
        assert!(app.input.is_empty());
        assert!(app.busy);
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
        for action in ["cancel", "details", "tasks", "history", "redraw"] {
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
}
