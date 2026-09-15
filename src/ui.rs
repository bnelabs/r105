//! Native Ratatui harness UI.
//!
//! The screen keeps the transcript central, makes the composer persistent,
//! and moves secondary work into small overlays. Long-running requests and
//! tools are cancellable, messages sent while busy are queued, and every
//! picker shares the same visible-window calculation.

use std::{
    collections::VecDeque,
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
struct ModelInfo {
    id: String,
    status: Option<String>,
}

impl ModelInfo {
    fn display(&self, active: &str) -> String {
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
    python_approved: bool,
) -> Result<()> {
    let mouse = config.mouse;
    let mut terminal = setup_terminal(mouse)?;
    let mut app = UiApp::new(backend, state, paths, config, python_approved);
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
    backend: Backend,
    state: ChatState,
    paths: ConfigPaths,
    plugins_dir: PathBuf,
    input: String,
    cursor: usize,
    mode: Mode,
    overlay: Overlay,
    palette_selected: usize,
    palette_scroll: usize,
    transcript_scroll: usize,
    follow_transcript: bool,
    show_details: bool,
    busy: bool,
    streaming: String,
    status: String,
    /// When the active request started, for the slow-start hint. Cold model
    /// loads look exactly like a hung request until the first token lands.
    request_started: Option<Instant>,
    awaiting_first_token: bool,
    slow_hint_shown: bool,
    queue: VecDeque<(String, Option<String>)>,
    /// File context resolved from `@refs`, pushed as a system message next
    /// to the user message once the backend answers.
    pending_context: Option<String>,
    /// Undone exchanges, newest last; any new user prompt clears the stack.
    redo_stack: Vec<UndoEntry>,
    /// Set by steering: the in-flight request is cancelled and its prompt
    /// must not be restored into the composer or offered via `/retry`.
    drop_next_restore: bool,
    active_user: Option<String>,
    last_failed_prompt: Option<String>,
    tool_round: usize,
    cancellation: Option<CancellationToken>,
    pending_connection: Option<Connection>,
    sandbox: Sandbox,
    python_bridge_command: Option<String>,
    python_approved: bool,
    last_response: String,
    tx: mpsc::UnboundedSender<UiEvent>,
    rx: mpsc::UnboundedReceiver<UiEvent>,
    quit: bool,
    exit_notice: Option<String>,
    /// Accumulated session token usage for the footer telemetry.
    session_in: u64,
    session_out: u64,
    git_branch: Option<String>,
    skills_available: usize,
    attention_bell: bool,
    mouse_enabled: bool,
    /// `@file` completion state: selected index plus an input-keyed cache so
    /// the workspace walk only reruns when the composer text changes.
    at_selected: usize,
    at_cache_key: String,
    at_cache_items: Vec<String>,
    /// Markdown-backed custom commands (`/name` from `commands/*.md`),
    /// reloaded on `/config reload`, `/commands reload`, and workspace
    /// switches so new files never need a restart.
    custom_commands: Vec<CustomCommand>,
    /// First-argument value completion (`/theme <Tab>`): same input-keyed
    /// cache discipline as the `@` menu.
    arg_selected: usize,
    arg_cache_key: String,
    arg_cache_items: Vec<String>,
    /// Model ids from the last `/models` refresh, backing `/model <Tab>`.
    known_models: Vec<String>,
    editor_requested: bool,
}

impl UiApp {
    fn new(
        backend: Backend,
        state: ChatState,
        paths: ConfigPaths,
        config: Config,
        python_approved: bool,
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
            request_started: None,
            awaiting_first_token: false,
            slow_hint_shown: false,
            queue: VecDeque::new(),
            pending_context: None,
            redo_stack: Vec::new(),
            drop_next_restore: false,
            active_user: None,
            last_failed_prompt: None,
            tool_round: 0,
            cancellation: None,
            pending_connection: None,
            sandbox,
            python_bridge_command: config.python_bridge_command.clone(),
            python_approved: python_approved || config.auto_approve_execute_python,
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

    async fn event_loop(
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
                    self.status = format!("Editor failed: {error:#}");
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
                            self.status = format!("input error: {error}");
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

    fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                UiEvent::Backend(BackendEvent::Token(token)) => {
                    self.streaming.push_str(&token);
                    self.awaiting_first_token = false;
                    self.status = "Generating…".into();
                    self.follow_transcript = true;
                }
                UiEvent::Backend(BackendEvent::Status(status)) => self.status = status,
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
                    self.status = format!("Request failed: {error} · /retry to try again");
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
                    self.status = format!(
                        "{} tool result(s) [{}] received; continuing…",
                        results.len(),
                        names
                    );
                    self.start_continue();
                }
                UiEvent::Compacted { summary, recent } => {
                    self.awaiting_first_token = false;
                    self.state.history =
                        vec![Message::system(format!("Conversation summary:\n{summary}"))];
                    self.state.history.extend(recent);
                    self.state.last_usage = Usage::default();
                    self.busy = false;
                    self.cancellation = None;
                    self.follow_transcript = true;
                    self.status = "Context compacted".into();
                    self.start_next_queued();
                }
                UiEvent::ModelsLoaded { backend, models } => {
                    self.backend = backend;
                    self.pending_connection = None;
                    if models.is_empty() {
                        self.status = "Connected; provider returned no model list".into();
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
                        self.status = if cold > 0 {
                            format!(
                                "Connected; choose a model ({} available, {} unloaded — first use loads them)",
                                models.len(),
                                cold
                            )
                        } else {
                            format!("Connected; choose a model ({} available)", models.len())
                        };
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
                        self.status = "Draft landed as a transcript note".into();
                    }
                    Err(error) => self.status = error,
                },
            }
        }
    }

    fn chat_done(&mut self, result: ChatResult) {
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
            self.status = format!(
                "Running {} tool call(s) [{}]…",
                result.tool_calls.len(),
                tool_names(&names)
            );
            let Some(cancellation) = self.cancellation.clone() else {
                self.busy = false;
                self.status = "Tool execution aborted: missing cancellation token".into();
                self.start_next_queued();
                return;
            };
            let context = ToolContext {
                workspace: self.state.workspace.clone(),
                plugins_dir: self.plugins_dir.clone(),
                python_bridge_command: self.python_bridge_command.clone(),
                sandbox: self.sandbox.clone(),
                cancellation,
                allow_network: self.state.permission_posture != "restricted"
                    && self.state.permission_posture != "off",
                allow_code: self.state.permission_posture != "off",
                python_approved: self.python_approved,
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
                self.status = format!(
                    "Tool-round limit ({MAX_TOOL_ROUNDS}) reached; {} call(s) [{}] not executed",
                    result.tool_calls.len(),
                    tool_names(&names)
                );
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
                self.status = format!("Done in {:.2}s", result.wall_seconds);
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
    fn maybe_note_slow_start(&mut self) {
        const SLOW_START_SECONDS: u64 = 15;
        if slow_start_due(
            self.busy,
            self.awaiting_first_token,
            self.slow_hint_shown,
            self.request_started,
            Instant::now(),
        ) {
            self.slow_hint_shown = true;
            self.status = format!(
                "Still waiting for the first token (>{SLOW_START_SECONDS}s) — the server may be loading the model; Esc cancels"
            );
        }
    }

    fn start_prompt(&mut self, prompt: String, context: Option<String>) {
        if self.busy {
            self.queue.push_back((prompt, context));
            self.status = format!("Queued prompt ({} waiting)", self.queue.len());
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
        self.status = format!("Sending in {} mode…", self.mode.as_str());
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

    fn start_continue(&mut self) {
        let Some(cancellation) = self.cancellation.clone() else {
            self.busy = false;
            self.status = "Done; continuation unavailable (no active request)".into();
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

    fn start_next_queued(&mut self) {
        if let Some((prompt, context)) = self.queue.pop_front() {
            self.start_prompt(prompt, context);
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.busy {
                self.cancel_work("Cancelling…");
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if self.action_key("cancel", &key) {
            self.cancel_work("Cancelling current work…");
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            if matches!(self.overlay, Overlay::None) {
                self.cancel_work("Cancelled");
            } else {
                self.overlay = Overlay::None;
                self.input.clear();
                self.cursor = 0;
            }
            return Ok(());
        }
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key).await?;
            return Ok(());
        }
        if self.action_key("details", &key) {
            self.show_details = !self.show_details;
            self.status = if self.show_details {
                "Details expanded".into()
            } else {
                "Details collapsed".into()
            };
            return Ok(());
        }
        if self.action_key("tasks", &key) {
            let text = if self.busy {
                format!(
                    "active request; {} queued prompt(s); tool round {}",
                    self.queue.len(),
                    self.tool_round
                )
            } else {
                format!("idle; {} queued prompt(s)", self.queue.len())
            };
            self.push_system(&text);
            return Ok(());
        }
        if self.action_key("history", &key) {
            self.status = "History search: type /session search <term>".into();
            return Ok(());
        }
        if self.action_key("redraw", &key) {
            self.transcript_scroll = 0;
            self.follow_transcript = true;
            self.status = "Redrawn".into();
            return Ok(());
        }
        if key.code == KeyCode::Tab && self.at_menu_open() {
            self.accept_at_complete();
            return Ok(());
        }
        if key.code == KeyCode::Tab && self.arg_menu_open() {
            self.accept_arg_complete();
            return Ok(());
        }
        if key.code == KeyCode::Tab {
            self.mode = match self.mode {
                Mode::Build => Mode::Plan,
                Mode::Plan => Mode::Ask,
                Mode::Ask => Mode::Build,
            };
            self.status = format!("Mode: {}", self.mode.as_str());
            return Ok(());
        }
        if self.at_menu_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.at_cache_items.len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.at_selected = self.at_selected.saturating_sub(1);
                } else {
                    self.at_selected = (self.at_selected + 1).min(count - 1);
                }
            }
            return Ok(());
        }
        if self.arg_menu_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.arg_cache_items.len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.arg_selected = self.arg_selected.saturating_sub(1);
                } else {
                    self.arg_selected = (self.arg_selected + 1).min(count - 1);
                }
            }
            return Ok(());
        }
        if self.palette_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.palette_items().len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.palette_selected = self.palette_selected.saturating_sub(1);
                } else {
                    self.palette_selected = (self.palette_selected + 1).min(count - 1);
                }
                self.palette_scroll =
                    command::ensure_visible(self.palette_selected, self.palette_scroll, 7, count);
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.insert_text("\n");
            }
            KeyCode::Enter => self.submit().await?,
            KeyCode::Char(character) => self.insert_text(&character.to_string()),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = previous_boundary(&self.input, self.cursor),
            KeyCode::Right => self.cursor = next_boundary(&self.input, self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::PageUp => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(8);
                self.follow_transcript = false;
            }
            KeyCode::PageDown => {
                self.transcript_scroll = self.transcript_scroll.saturating_add(8);
                self.follow_transcript = false;
            }
            KeyCode::Up => self.history_previous(),
            KeyCode::Down => self.history_next(),
            _ => {}
        }
        Ok(())
    }

    async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        let current = std::mem::replace(&mut self.overlay, Overlay::None);
        match current {
            Overlay::Providers {
                mut selected,
                mut scroll,
            } => {
                let count = provider::PRESETS.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(6),
                    KeyCode::PageDown => selected = (selected + 6).min(count.saturating_sub(1)),
                    KeyCode::Enter => {
                        let preset = provider::PRESETS[selected];
                        let id = preset.id.to_string();
                        if preset.asks_for_url {
                            self.open_url_overlay(id);
                            keep = false;
                        } else if preset.api_key_required
                            && preset
                                .api_key_env
                                .and_then(|name| std::env::var(name).ok())
                                .is_none_or(|value| value.trim().is_empty())
                        {
                            self.input.clear();
                            self.cursor = 0;
                            self.overlay = Overlay::ApiKey { provider: id };
                            keep = false;
                        } else {
                            self.start_provider(id, None);
                            keep = false;
                        }
                    }
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    scroll = command::ensure_visible(selected, scroll, 8, count);
                    self.overlay = Overlay::Providers { selected, scroll };
                }
            }
            Overlay::Models {
                items,
                mut selected,
                mut scroll,
                active,
            } => {
                let count = items.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(8),
                    KeyCode::PageDown => selected = (selected + 8).min(count.saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(model) = items.get(selected) {
                            self.state.model = model.id.clone();
                            let model = model.id.clone();
                            self.persist_connection(Some(&model));
                            self.status = format!("Model selected: {model}");
                        }
                        keep = false;
                    }
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    scroll = command::ensure_visible(selected, scroll, 10, count);
                    self.overlay = Overlay::Models {
                        items,
                        selected,
                        scroll,
                        active,
                    };
                }
            }
            Overlay::ApiKey { provider } => match key.code {
                KeyCode::Enter => {
                    let value = self.input.trim().to_string();
                    self.input.clear();
                    self.cursor = 0;
                    if value.is_empty() {
                        self.status = "API key was not entered".into();
                    } else {
                        self.start_provider(provider, Some(value));
                    }
                }
                KeyCode::Char(character) => {
                    self.insert_text(&character.to_string());
                    self.overlay = Overlay::ApiKey { provider };
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.overlay = Overlay::ApiKey { provider };
                }
                _ => self.overlay = Overlay::ApiKey { provider },
            },
            Overlay::CustomUrl { provider } => match key.code {
                KeyCode::Enter => {
                    let value = if self.input.trim().is_empty() {
                        provider::preset(&provider)
                            .and_then(|preset| preset.base_url)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        self.input.trim().to_string()
                    };
                    self.input.clear();
                    self.cursor = 0;
                    if provider::valid_url(&value) {
                        self.start_provider(provider, Some(value));
                    } else {
                        self.status = "Enter an http:// or https:// URL without credentials".into();
                        self.overlay = Overlay::CustomUrl { provider };
                    }
                }
                KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input.clear();
                    self.cursor = 0;
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Char(character) => {
                    self.insert_text(&character.to_string());
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Delete => {
                    self.delete();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Left => {
                    self.cursor = previous_boundary(&self.input, self.cursor);
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Right => {
                    self.cursor = next_boundary(&self.input, self.cursor);
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Home => {
                    self.cursor = 0;
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::End => {
                    self.cursor = self.input.len();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                _ => self.overlay = Overlay::CustomUrl { provider },
            },
            Overlay::Theme {
                mut selected,
                original,
            } => {
                let count = THEMES.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.state.theme = THEMES[selected].to_string();
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(count.saturating_sub(1));
                        self.state.theme = THEMES[selected].to_string();
                    }
                    KeyCode::Enter => {
                        let theme = THEMES[selected].to_string();
                        self.state.theme = theme.clone();
                        self.persist_config(|config| config.theme = theme.clone());
                        self.status = format!("Theme: {theme}");
                        keep = false;
                    }
                    KeyCode::Esc => {
                        // Revert the live preview.
                        self.state.theme = original.clone();
                        keep = false;
                    }
                    _ => {}
                }
                if keep {
                    self.overlay = Overlay::Theme { selected, original };
                }
            }
            Overlay::Settings { mut selected } => {
                const COUNT: usize = 6;
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(COUNT - 1),
                    KeyCode::Left => self.cycle_setting(selected, -1),
                    KeyCode::Right | KeyCode::Enter => self.cycle_setting(selected, 1),
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    self.overlay = Overlay::Settings { selected };
                }
            }
            Overlay::None => {}
        }
        Ok(())
    }

    async fn submit(&mut self) -> Result<()> {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        if self.palette_active()
            && command::command(&value).is_none()
            && !self.is_custom_command(&value)
        {
            if let Some(item) = self.palette_items().get(self.palette_selected) {
                self.input = format!("{} ", item.name);
                self.cursor = self.input.len();
            }
            return Ok(());
        }
        self.input.clear();
        self.cursor = 0;
        self.at_cache_key.clear();
        self.arg_cache_key.clear();
        if let Some(shell) = value.strip_prefix('!') {
            let shell = shell.trim().to_string();
            if shell.is_empty() {
                self.status = "Usage: !<shell command>".into();
                return Ok(());
            }
            self.redo_stack.clear();
            self.state.history.push(Message::user(value));
            self.follow_transcript = true;
            self.run_shell_command(shell);
            return Ok(());
        }
        if let Some(parsed) = command::parse(&value) {
            self.handle_command(parsed).await
        } else {
            self.submit_prompt(value);
            Ok(())
        }
    }

    /// Shared prompt entry: resolves `@file` references into attached
    /// context, then steers the active request or starts a new one.
    fn submit_prompt(&mut self, value: String) {
        self.redo_stack.clear();
        let mut blocks = Vec::new();
        let mut unknown = Vec::new();
        for path in extract_file_refs(&value) {
            match self.resolve_file_ref(&path) {
                Some(block) => blocks.push(block),
                None => unknown.push(path),
            }
        }
        if !unknown.is_empty() {
            self.status = format!("Unknown @ref(s) sent literally: {}", unknown.join(", "));
        }
        let context = if blocks.is_empty() {
            None
        } else {
            Some(blocks.join("\n"))
        };
        if self.busy {
            // Steer: cancel the in-flight request and jump the queue. The
            // cancelled task errors out, drops its restore via
            // `drop_next_restore`, and the steered prompt runs next.
            if let Some(token) = &self.cancellation {
                token.cancel();
            }
            self.drop_next_restore = true;
            self.queue.push_front((value, context));
            self.status = format!("Steering… ({} queued behind)", self.queue.len() - 1);
            return;
        }
        self.start_prompt(value, context);
    }

    /// Resolve one `@path` token against the workspace. Files are inlined
    /// (bounded), directories become listings; anything else is `None` so
    /// the caller can warn and leave the token literal.
    fn resolve_file_ref(&self, path: &str) -> Option<String> {
        let resolved = safe_path(&self.state.workspace, path).ok()?;
        if resolved.is_file() {
            let content = std::fs::read_to_string(&resolved).ok()?;
            const LIMIT: usize = 12_000;
            let over = content.chars().count() > LIMIT;
            let body: String = content.chars().take(LIMIT).collect();
            Some(format!(
                "<file path=\"{path}\">\n{body}{}\n</file>",
                if over { "\n⋯ truncated" } else { "" }
            ))
        } else if resolved.is_dir() {
            let mut entries: Vec<String> = std::fs::read_dir(&resolved)
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    entry.file_name().into_string().ok().map(|name| {
                        format!("{}{}", name, if entry.path().is_dir() { "/" } else { "" })
                    })
                })
                .take(40)
                .collect();
            entries.sort();
            Some(format!(
                "<directory path=\"{path}\">\n{}\n</directory>",
                entries.join("\n")
            ))
        } else {
            None
        }
    }

    /// Run a `!` shell line inside the sandbox boundary (timeouts,
    /// cancellation, workspace confinement) and attach its output to the
    /// conversation so the next turn can use it.
    fn run_shell_command(&mut self, command: String) {
        if self.state.permission_posture == "off" {
            self.status = "Shell (!) is disabled by permission posture 'off'".into();
            return;
        }
        let workspace = self.state.workspace.clone();
        let sandbox = self.sandbox.clone();
        let allow_network =
            self.state.permission_posture != "restricted" && self.state.permission_posture != "off";
        let cancellation = self.cancellation.clone().unwrap_or_default();
        let sender = self.tx.clone();
        let preview: String = command.chars().take(60).collect();
        self.status = format!("Running shell: {preview}");
        tokio::spawn(async move {
            let (program, args) = if cfg!(windows) {
                ("cmd", vec!["/C".to_string(), command.clone()])
            } else {
                ("sh", vec!["-c".to_string(), command.clone()])
            };
            let notice = match sandbox
                .run(program, &args, &workspace, allow_network, &cancellation)
                .await
            {
                Ok(output) => {
                    let mut body = String::new();
                    let stdout = output.stdout.trim_end();
                    if !stdout.is_empty() {
                        body.push_str(stdout);
                    }
                    let stderr = output.stderr.trim_end();
                    if !stderr.is_empty() {
                        if !body.is_empty() {
                            body.push('\n');
                        }
                        body.push_str("stderr:\n");
                        body.push_str(stderr);
                    }
                    if body.is_empty() {
                        body.push_str("(no output)");
                    }
                    const LIMIT: usize = 8_000;
                    let over = body.chars().count() > LIMIT;
                    let shown: String = body.chars().take(LIMIT).collect();
                    format!(
                        "$ {command}\n{shown}{}",
                        if over { "\n⋯ output truncated" } else { "" }
                    )
                }
                Err(error) => format!("$ {command}\nfailed: {error:#}"),
            };
            let _ = sender.send(UiEvent::Notice(notice));
        });
    }

    /// Suspend the TUI, run `$EDITOR` on the current draft, and resume with
    /// whatever it left behind.
    async fn run_editor(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let editor = std::env::var("EDITOR").unwrap_or_default();
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            anyhow::bail!("set $EDITOR to compose prompts externally (e.g. EDITOR=nvim)");
        };
        let program = program.to_string();
        let rest: Vec<String> = parts.map(str::to_string).collect();
        let path = std::env::temp_dir().join(format!("r105-editor-{}.md", std::process::id()));
        std::fs::write(&path, &self.input).context("writing editor draft")?;
        restore_terminal(terminal).context("suspending TUI for editor")?;
        let edited = tokio::process::Command::new(&program)
            .args(&rest)
            .arg(&path)
            .status()
            .await;
        let setup = setup_terminal(self.mouse_enabled);
        *terminal = setup.context("restoring TUI after editor")?;
        edited.context("running editor")?;
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        self.input = content.trim_end_matches('\n').to_string();
        self.cursor = self.input.len();
        self.at_cache_key.clear();
        self.status = format!(
            "Draft from {program} ({} chars)",
            self.input.chars().count()
        );
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(3);
                self.follow_transcript = false;
            }
            MouseEventKind::ScrollDown => {
                self.transcript_scroll = self.transcript_scroll.saturating_add(3);
            }
            _ => {}
        }
    }

    fn ring_bell(&self) {
        if self.attention_bell {
            print!("\x07");
            let _ = io::stdout().flush();
        }
    }

    fn refresh_git_branch(&mut self) {
        self.git_branch = git_branch_for(&self.state.workspace);
    }

    fn refresh_skills(&mut self) {
        self.skills_available = count_skills(&self.state.skills_dir);
    }

    /// Reload Markdown-backed custom commands from the global and project
    /// directories. Built-in collisions are dropped here (built-ins win) and
    /// counted so `/commands` can report the shadowing instead of hiding it.
    fn refresh_custom_commands(&mut self) -> usize {
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

    /// Remappable Ctrl actions. `keybindings` maps action names (`cancel`,
    /// `details`, `tasks`, `history`, `redraw`) to `ctrl+<letter>`; anything
    /// else falls back to the built-in default from `KEY_ACTIONS`.
    fn action_key(&self, action: &str, key: &KeyEvent) -> bool {
        let default = KEY_ACTIONS
            .iter()
            .find(|(name, _)| *name == action)
            .map(|(_, shortcut)| *shortcut);
        match (self.state.keybindings.get(action), default) {
            (Some(spec), _) => match_ctrl_spec(spec, key),
            (None, Some(shortcut)) => {
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(shortcut)
            }
            (None, None) => false,
        }
    }

    async fn handle_command(&mut self, parsed: ParsedCommand) -> Result<()> {
        match parsed.name.as_str() {
            "/" | "/help" => self.command_help(&parsed.args),
            "/state" => self.command_state(),
            "/connect" | "/provider" => self.command_connect(&parsed.args),
            "/models" => self.start_model_list(),
            "/model" => {
                if let Some(model) = parsed.args.first() {
                    self.state.model = model.clone();
                    let model = model.clone();
                    self.persist_connection(Some(&model));
                    self.status = format!("Model: {model}");
                } else {
                    self.start_model_list();
                }
            }
            "/health" => self.start_health(),
            "/profiles" => self.start_profiles(),
            "/profile" => self.command_profile(&parsed.args),
            "/history" => self.command_history(),
            "/plan" => {
                self.mode = Mode::Plan;
                self.status = "Mode: plan".into();
            }
            "/build" => {
                self.mode = Mode::Build;
                self.status = "Mode: build".into();
            }
            "/ask" => {
                self.mode = Mode::Ask;
                self.status = "Mode: ask".into();
            }
            "/skills" => self.command_skills(),
            "/skill" => self.command_skill(&parsed.args),
            "/compact" => self.command_compact(),
            "/tokens" => {
                let usage = self.state.token_usage();
                self.push_system(&format!(
                    "Context: {} / {} tokens ({:.1}%, {} confidence)",
                    usage.used_tokens,
                    usage.context_tokens,
                    usage.percent(),
                    usage.source
                ));
            }
            "/quality" => self.command_quality(&parsed.args),
            "/json" => self.command_json(&parsed.args),
            "/max" => self.command_max(&parsed.args),
            "/cache-prompt" => {
                self.state.cache_prompt = parsed
                    .args
                    .first()
                    .map(|value| value != "off")
                    .unwrap_or(!self.state.cache_prompt);
                let enabled = self.state.cache_prompt;
                self.status = format!(
                    "llama.cpp prompt caching: {}",
                    if enabled { "on" } else { "off" }
                );
                self.persist_config(|config| config.cache_prompt = enabled);
            }
            "/config" => self.command_config(&parsed.args).await,
            "/clear" => {
                self.state.history.clear();
                self.streaming.clear();
                self.redo_stack.clear();
                self.status = "Transcript cleared".into();
            }
            "/workspace" => self.command_workspace(&parsed.args),
            "/session" => self.command_session(&parsed.args),
            "/export" => self.command_export(&parsed.args),
            "/mcp" => self.command_mcp(&parsed.args)?,
            "/plugin" => self.push_system(&serde_json::to_string_pretty(
                &crate::plugin::status_from(&self.plugins_dir),
            )?),
            "/theme" => self.command_theme(&parsed.args),
            "/autocompact" => self.command_autocompact(&parsed.args),
            "/reasoning" => self.command_reasoning(&parsed.args),
            "/permissions" => self.command_permissions(&parsed.args),
            "/approve" => self.command_approve(&parsed.args),
            "/preview" => self.command_preview(&parsed.args),
            "/bridge" => {
                self.push_system(&python_bridge_status(self.python_bridge_command.as_deref()))
            }
            "/map" => self.push_system(&self.workspace_map()),
            "/diff" => self.push_system(&self.workspace_diff()),
            "/copy" => {
                self.command_copy(&parsed.args);
            }
            "/tasks" => self.push_system(&format!(
                "busy={} queued={} tool_round={}",
                self.busy,
                self.queue.len(),
                self.tool_round
            )),
            "/retry" => self.command_retry(),
            "/undo" => self.command_undo(),
            "/redo" => self.command_redo(),
            "/editor" => self.command_editor(),
            "/settings" => {
                self.overlay = Overlay::Settings { selected: 0 };
                self.status = "Settings · ↑↓ move · ←/→ change · Esc close".into();
            }
            "/thinking" => self.command_thinking(&parsed.args),
            "/attention" => self.command_attention(&parsed.args),
            "/commands" => self.command_custom_commands(&parsed.args),
            "/sh" => self.command_shell_draft(&parsed.args),
            "/exit" => self.quit = true,
            _ => {
                if let Some(custom) = self.find_custom_command(&parsed.name) {
                    let expanded = custom::substitute_args(&custom.content, &parsed.args);
                    self.submit_prompt(expanded);
                    // `submit_prompt` sets Sending/Steering/Queued status;
                    // keep it and prefix the expansion attribution.
                    let outcome = std::mem::take(&mut self.status);
                    self.status = format!(
                        "Expanded /{} ({}) · {}",
                        custom.name, custom.source, outcome
                    );
                } else if let Some(hit) = command::suggest(&parsed.name, &self.custom_commands) {
                    self.status = format!("Unknown command {}; did you mean {hit}?", parsed.name);
                } else {
                    self.status = format!("Unknown command {}; type /help", parsed.name);
                }
            }
        }
        Ok(())
    }

    /// `/help [command]`: full dump by default, or one entry's usage,
    /// description, and (for customs) argument hint plus source file.
    fn command_help(&mut self, args: &[String]) {
        let Some(topic) = args.first() else {
            self.push_system(&command::help_text_with(&self.custom_commands));
            return;
        };
        if let Some(item) = command::command(topic) {
            self.push_system(&format!("{}\n  {}", item.usage, item.description));
            return;
        }
        if let Some(custom) = self.find_custom_command(topic) {
            let mut output = format!("/{}\n  {}", custom.name, custom.description);
            if let Some(hint) = &custom.argument_hint {
                output.push_str(&format!("\n  arguments: {hint}"));
            }
            output.push_str(&format!(
                "\n  source: {} ({})",
                custom.source,
                custom.path.display()
            ));
            self.push_system(&output);
            return;
        }
        self.push_system(&command::help_text_with(&self.custom_commands));
    }

    /// `/commands [reload]`: list loaded Markdown commands with scope, or
    /// reload both scopes first. Shadowed files (built-in wins) are
    /// reported so nothing is silently ignored.
    fn command_custom_commands(&mut self, args: &[String]) {
        if args.first().is_some_and(|action| action == "reload") {
            let shadowed = self.refresh_custom_commands();
            self.status = if shadowed == 0 {
                format!("Reloaded {} custom command(s)", self.custom_commands.len())
            } else {
                format!(
                    "Reloaded {} custom command(s); {shadowed} shadowed by built-ins",
                    self.custom_commands.len()
                )
            };
        }
        if self.custom_commands.is_empty() {
            self.push_system(&format!(
                "No custom commands. Drop name.md files in\n  {}\n  {}",
                commands_dir(&self.paths).display(),
                project_commands_dir(&self.state.workspace).display()
            ));
            return;
        }
        let mut lines = vec!["Custom commands".to_string(), String::new()];
        for command in &self.custom_commands {
            let shadow = if command::command(&format!("/{}", command.name)).is_some() {
                " (shadowed by built-in)"
            } else {
                ""
            };
            lines.push(format!(
                "  /{:<18} {} ({}){shadow}",
                command.name, command.description, command.source
            ));
        }
        self.push_system(&lines.join("\n"));
    }

    fn command_state(&mut self) {
        let connection = self.backend.connection();
        let bridge = python_bridge_status(self.python_bridge_command.as_deref());
        self.push_system(&format!(
            "mode={}\nprovider={}\nbackend={}\nurl={}\nmodel={}\nquality={}\nprofile={}\nreasoning={}\nthinking={}\nbell={}\npermissions={}\nsandbox={}\n{}",
            self.mode.as_str(),
            connection.display_name(),
            connection.backend,
            connection.base_url,
            self.state.model,
            self.state.quality.as_deref().unwrap_or("auto"),
            self.state.profile.as_deref().unwrap_or("auto"),
            self.state.reasoning_effort,
            if self.state.show_thinking {
                "shown"
            } else {
                "hidden"
            },
            if self.attention_bell { "on" } else { "off" },
            self.state.permission_posture,
            self.sandbox.selected_name(),
            bridge,
        ));
    }

    /// `/sh <plain words>`: ask the model for one shell command and prefill
    /// the composer with `!<command>` for review. Nothing runs without an
    /// explicit Enter, so the existing `!` posture gates and sandbox path
    /// apply unchanged. The draft is a cheap history-free one-shot and never
    /// touches the transcript or the busy/queue machinery.
    fn command_shell_draft(&mut self, args: &[String]) {
        let request = args.join(" ");
        if request.is_empty() {
            self.status = "Usage: /sh <describe the shell command>".into();
            return;
        }
        if self.state.permission_posture == "off" {
            self.status = "Shell drafts are disabled by permission posture 'off'".into();
            return;
        }
        if self.busy {
            self.status = "Busy — draft shell commands when idle".into();
            return;
        }
        let shell = if cfg!(windows) {
            "Windows cmd.exe (run via `cmd /C`)"
        } else {
            "POSIX sh (run via `sh -c`)"
        };
        let prompt = format!(
            "Translate the request into ONE shell command for {} on {}.\n\
             Output ONLY the command: no fences, no surrounding quotes, no explanation, single line.\n\
             Request: {request}",
            shell,
            std::env::consts::OS,
        );
        let backend = self.backend.clone();
        let mut state = self.state.clone();
        state.history.clear();
        let sender = self.tx.clone();
        self.status = "Drafting shell command…".into();
        tokio::spawn(async move {
            let outcome = match backend.chat(&state, &prompt, &[]).await {
                Ok(result) => match clean_shell_draft(&result.content) {
                    Some(command) => Ok(command),
                    None => Err("The model returned no usable command".to_string()),
                },
                Err(error) => Err(format!("Shell draft failed: {error:#}")),
            };
            let _ = sender.send(UiEvent::ShellDraft(outcome));
        });
    }

    fn command_history(&mut self) {
        if self.state.history.is_empty() {
            self.push_system("Transcript is empty");
            return;
        }
        let start = self.state.history.len().saturating_sub(12);
        let mut lines = vec![format!(
            "Last {} of {} messages:",
            self.state.history.len() - start,
            self.state.history.len()
        )];
        for message in &self.state.history[start..] {
            let preview = message
                .content
                .chars()
                .take(160)
                .collect::<String>()
                .replace('\n', " ");
            lines.push(format!("[{}] {}", message.role, preview));
        }
        self.push_system(&lines.join("\n"));
    }

    fn command_quality(&mut self, args: &[String]) {
        const VALID: [&str; 3] = ["fast", "balanced", "best"];
        let Some(value) = args.first() else {
            self.push_system(&format!(
                "quality={}",
                self.state.quality.as_deref().unwrap_or("auto")
            ));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.status = format!("Unknown quality; choose {}", VALID.join(", "));
            return;
        }
        self.state.quality = Some(value.clone());
        self.persist_config(|config| config.quality = Some(value.clone()));
        self.status = format!("Quality: {value}");
    }

    fn command_profile(&mut self, args: &[String]) {
        const VALID: [&str; 7] = [
            "simple",
            "strict_json",
            "coding",
            "complex_reasoning",
            "long_context_qa",
            "tool_agent",
            "creative",
        ];
        let Some(value) = args.first() else {
            self.state.profile = None;
            self.persist_config(|config| config.profile = None);
            self.status = "Router profile: auto".into();
            return;
        };
        let value = value.to_ascii_lowercase();
        if value == "auto" {
            self.state.profile = None;
            self.persist_config(|config| config.profile = None);
            self.status = "Router profile: auto".into();
        } else if VALID.contains(&value.as_str()) {
            self.state.profile = Some(value.clone());
            self.persist_config(|config| config.profile = Some(value.clone()));
            self.status = format!("Router profile: {value}");
        } else {
            self.status = format!("Unknown profile; choose auto, {}", VALID.join(", "));
        }
    }

    fn command_json(&mut self, args: &[String]) {
        self.state.json_mode = toggle_value(args.first(), self.state.json_mode);
        self.status = format!(
            "JSON response mode: {}",
            if self.state.json_mode { "on" } else { "off" }
        );
    }

    fn command_max(&mut self, args: &[String]) {
        let Some(value) = args.first() else {
            self.state.max_tokens = None;
            self.status = "Maximum completion tokens: auto".into();
            return;
        };
        match value.parse::<u32>() {
            Ok(value) if value > 0 => {
                self.state.max_tokens = Some(value);
                self.status = format!("Maximum completion tokens: {value}");
            }
            _ => self.status = "Usage: /max <positive token count>".into(),
        }
    }

    async fn command_config(&mut self, args: &[String]) {
        let action = args.first().map(String::as_str).unwrap_or("reload");
        let config = match Config::load(&self.paths) {
            Ok(config) => config,
            Err(error) => {
                self.status = format!("Config read failed: {error:#}");
                return;
            }
        };
        if action == "show" {
            match serde_json::to_string_pretty(&config) {
                Ok(value) => self.push_system(&value),
                Err(error) => self.status = format!("Config formatting failed: {error}"),
            }
            return;
        }
        if action != "reload" {
            self.status = "Usage: /config show|reload".into();
            return;
        }

        let previous = self.backend.connection().clone();
        let mut connection_changed = false;
        if config.backend.is_some() || config.url.is_some() || config.provider.is_some() {
            let mut connection = provider::resolve_connection(
                config.provider.as_deref(),
                config.backend.as_deref(),
                config.url.as_deref(),
            );
            // A key entered in the guided TUI lives only in memory. Retain it
            // across a reload when the endpoint remains the same.
            if connection.api_key.is_none()
                && connection.base_url == previous.base_url
                && connection.provider_id == previous.provider_id
            {
                connection.api_key = previous.api_key.clone();
            }
            if connection.backend != previous.backend
                || connection.base_url != previous.base_url
                || connection.provider_id != previous.provider_id
            {
                match self.backend.with_connection(connection) {
                    Ok(candidate) => {
                        self.backend = candidate;
                        connection_changed = true;
                    }
                    Err(error) => {
                        self.status = format!("Config connection rejected: {error}");
                        return;
                    }
                }
            }
        }

        self.state.theme = config.theme.clone();
        self.state.quality = config.quality.clone();
        self.state.profile = config.profile.clone();
        if let Some(model) = config.model.clone() {
            self.state.model = model;
        }
        self.state.auto_compact = config.auto_compact;
        self.state.cache_prompt = config.cache_prompt;
        self.state.reasoning_effort = config.reasoning_effort.clone();
        self.state.show_thinking = config.show_thinking;
        self.state.thinking_default_expanded = config.thinking_default_expanded;
        self.state.permission_posture = config.permission_posture.clone();
        self.state.keybindings = config.keybindings.clone();
        self.state.skills_dir = config.skills_dir.clone();
        self.state.model_contexts = config.model_contexts.clone();
        self.attention_bell = config.attention_bell;
        let mouse_note = if config.mouse != self.mouse_enabled {
            self.mouse_enabled = config.mouse;
            " · mouse capture applies on restart"
        } else {
            ""
        };
        if let Some(tokens) = config.context_tokens {
            self.state.context_tokens = tokens;
        }
        self.plugins_dir = config.plugins_dir.clone();
        self.python_bridge_command = config.python_bridge_command.clone();
        self.python_approved |= config.auto_approve_execute_python;
        self.sandbox = Sandbox::detect(
            &config.sandbox_backend,
            config.docker_image.clone(),
            config.timeout_seconds,
        );
        let connection_note = if connection_changed {
            ", connection updated"
        } else {
            ""
        };
        self.refresh_skills();
        self.refresh_custom_commands();
        self.refresh_git_branch();
        self.status = format!("Config reloaded{connection_note}{mouse_note}");
    }

    fn command_autocompact(&mut self, args: &[String]) {
        self.state.auto_compact = toggle_value(args.first(), self.state.auto_compact);
        let enabled = self.state.auto_compact;
        self.persist_config(|config| config.auto_compact = enabled);
        self.status = format!(
            "Automatic compaction: {}",
            if enabled { "on" } else { "off" }
        );
    }

    fn command_reasoning(&mut self, args: &[String]) {
        const VALID: [&str; 5] = ["auto", "off", "low", "medium", "high"];
        let Some(value) = args.first() else {
            self.push_system(&format!("reasoning_effort={}", self.state.reasoning_effort));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.status = format!("Unknown reasoning effort; choose {}", VALID.join(", "));
            return;
        }
        self.state.reasoning_effort = value.clone();
        self.persist_config(|config| config.reasoning_effort = value.clone());
        self.status = format!("Reasoning effort: {value}");
    }

    fn command_permissions(&mut self, args: &[String]) {
        const VALID: [&str; 4] = ["full-access", "restricted", "sandboxed", "off"];
        let Some(value) = args.first() else {
            self.push_system(&format!(
                "permission_posture={} (valid: {})",
                self.state.permission_posture,
                VALID.join(", ")
            ));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.status = format!("Unknown permission posture; choose {}", VALID.join(", "));
            return;
        }
        self.state.permission_posture = value.clone();
        self.persist_config(|config| config.permission_posture = value.clone());
        self.status = format!("Permission posture: {value}");
    }

    fn command_approve(&mut self, args: &[String]) {
        if !matches!(
            args.first().map(String::as_str),
            Some("execute_python" | "python")
        ) {
            self.status = "Usage: /approve execute_python".into();
            return;
        }
        self.python_approved = true;
        self.status = "Python bridge approved for this session".into();
    }

    fn command_preview(&mut self, args: &[String]) {
        let Some(requested) = args.first() else {
            self.status = "Usage: /preview <filename>".into();
            return;
        };
        let path = match safe_path(&self.state.workspace, requested) {
            Ok(path) => path,
            Err(error) => {
                self.status = format!("Preview path rejected: {error}");
                return;
            }
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => self.push_system(&format!(
                "--- {requested} ---\n{}",
                content.chars().take(2_000).collect::<String>()
            )),
            Err(error) => self.status = format!("Preview failed: {error}"),
        }
    }

    fn command_copy(&mut self, args: &[String]) {
        if self.last_response.is_empty() {
            self.status = "There is no response to copy".into();
            return;
        }
        let Some(requested) = args.first() else {
            self.status = if copy_to_clipboard(&self.last_response) {
                format!("Copied {} characters", self.last_response.chars().count())
            } else {
                "Clipboard unavailable (try pbcopy, wl-copy, xclip, or clip)".into()
            };
            return;
        };
        let index: usize = match requested.parse() {
            Ok(number) if number >= 1 => number,
            _ => {
                self.status = "Usage: /copy [n] (nth fenced code block)".into();
                return;
            }
        };
        let blocks = code_blocks(&self.last_response);
        match blocks.get(index - 1) {
            Some(block) => {
                self.status = if copy_to_clipboard(block) {
                    format!(
                        "Copied code block {index} ({} characters)",
                        block.chars().count()
                    )
                } else {
                    "Clipboard unavailable (try pbcopy, wl-copy, xclip, or clip)".into()
                };
            }
            None => {
                self.status = format!(
                    "Code block {index} not found ({} fenced block(s) in last response)",
                    blocks.len()
                );
            }
        }
    }

    fn persist_config<F>(&mut self, update: F)
    where
        F: FnOnce(&mut Config),
    {
        match Config::load(&self.paths) {
            Ok(mut config) => {
                update(&mut config);
                if let Err(error) = config.save(&self.paths) {
                    self.status = format!("Setting changed, but config save failed: {error}");
                }
            }
            Err(error) => self.status = format!("Setting changed, but config read failed: {error}"),
        }
    }

    fn command_compact(&mut self) {
        if self.busy {
            self.status = "Finish the active request before compacting".into();
            return;
        }
        if self.state.history.len() < 4 {
            self.status = "There is not enough conversation to compact yet".into();
            return;
        }
        self.start_compaction(false);
    }

    fn command_retry(&mut self) {
        if self.busy {
            self.status = "Finish the active request before retrying".into();
            return;
        }
        let Some(prompt) = self.last_failed_prompt.clone().or_else(|| {
            if self.input.trim().is_empty() {
                None
            } else {
                Some(self.input.trim().to_string())
            }
        }) else {
            self.status = "Nothing to retry".into();
            return;
        };
        self.last_failed_prompt = None;
        self.input.clear();
        self.cursor = 0;
        self.start_prompt(prompt, None);
    }

    fn start_compaction(&mut self, automatic: bool) {
        if self.state.history.len() < 4 {
            self.status = "There is not enough conversation to compact yet".into();
            return;
        }
        let keep = (self.state.history.len() / 3).max(1);
        let split = self.state.history.len().saturating_sub(keep);
        let older = self.state.history[..split].to_vec();
        let recent = self.state.history[split..].to_vec();
        let transcript = older
            .iter()
            .map(|message| format!("{}: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n\n");
        let prompt = format!(
            "Summarize the following conversation for a future coding harness turn. \
             Preserve decisions, constraints, file paths, unresolved work, and user preferences. \
             Be concise and factual.\n\n{transcript}"
        );
        let backend = self.backend.clone();
        let mut state = self.state.clone();
        state.history.clear();
        let sender = self.tx.clone();
        self.busy = true;
        self.request_started = Some(Instant::now());
        self.awaiting_first_token = true;
        self.slow_hint_shown = false;
        self.cancellation = Some(CancellationToken::new());
        self.status = if automatic {
            "Context near its limit; compacting…".into()
        } else {
            "Compacting context…".into()
        };
        tokio::spawn(async move {
            match backend.chat(&state, &prompt, &[]).await {
                Ok(result) => {
                    let _ = sender.send(UiEvent::Compacted {
                        summary: result.content,
                        recent,
                    });
                }
                Err(error) => {
                    let _ =
                        sender.send(UiEvent::ChatError(format!("compaction failed: {error:#}")));
                }
            }
        });
    }

    fn command_skills(&mut self) {
        let mut names = std::fs::read_dir(&self.state.skills_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                (entry.path().extension().and_then(|value| value.to_str()) == Some("md"))
                    .then(|| entry.file_name().to_string_lossy().to_string())
            })
            .collect::<Vec<_>>();
        names.sort();
        let output = if names.is_empty() {
            "No Markdown skills found".to_string()
        } else {
            format!(
                "Skills in {}:\n{}",
                self.state.skills_dir.display(),
                names.join("\n")
            )
        };
        self.push_system(&output);
    }

    fn command_skill(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            Some("use") => {
                let Some(name) = args.get(1).map(String::as_str) else {
                    self.status = "Usage: /skill use <name>".into();
                    return;
                };
                let Some(name) = skill_name(name) else {
                    self.status = "Skill names must be local Markdown filenames".into();
                    return;
                };
                let path = self.state.skills_dir.join(format!("{name}.md"));
                if !path.is_file() {
                    self.status = format!("Skill not found: {}", path.display());
                } else if !self.state.active_skills.contains(&name) {
                    self.state.active_skills.push(name.clone());
                    let params = args
                        .iter()
                        .skip(2)
                        .filter_map(|value| value.split_once('='))
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    if !params.is_empty() {
                        self.state.skill_params.insert(name.clone(), params);
                    }
                    self.status = format!("Skill active: {name}");
                } else {
                    self.status = format!("Skill already active: {name}");
                }
            }
            Some("show") => {
                let Some(name) = args.get(1).and_then(|value| skill_name(value)) else {
                    self.status = "Usage: /skill show <name>".into();
                    return;
                };
                let path = self.state.skills_dir.join(format!("{name}.md"));
                match std::fs::read_to_string(&path) {
                    Ok(content) => self.push_system(&format!("Skill: {name}\n\n{content}")),
                    Err(error) => self.status = format!("Skill read failed: {error}"),
                }
            }
            Some("drop") => {
                let Some(name) = args.get(1).and_then(|value| skill_name(value)) else {
                    self.status = "Usage: /skill drop <name>".into();
                    return;
                };
                let before = self.state.active_skills.len();
                self.state.active_skills.retain(|item| item != &name);
                self.state.skill_params.remove(&name);
                self.status = if before == self.state.active_skills.len() {
                    format!("Skill was not active: {name}")
                } else {
                    format!("Skill inactive: {name}")
                };
            }
            Some("clear") => {
                self.state.active_skills.clear();
                self.status = "All skills cleared".into();
            }
            _ => {
                self.status = "Usage: /skill use|show|drop|clear <name>".into();
            }
        }
    }

    fn command_connect(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            None => {
                self.overlay = Overlay::Providers {
                    selected: 0,
                    scroll: 0,
                };
                self.status = "Choose a provider; credentials stay in memory".into();
            }
            Some("status") | Some("show") => {
                let connection = self.backend.connection();
                self.push_system(&format!(
                    "provider={} backend={} url={} model={}",
                    connection.display_name(),
                    connection.backend,
                    connection.base_url,
                    self.state.model
                ));
            }
            Some("url") | Some("custom") => {
                if let Some(url) = args.get(1) {
                    if provider::valid_url(url) {
                        self.start_provider("custom".into(), Some(url.clone()));
                    } else {
                        self.status = "Invalid custom URL".into();
                    }
                } else {
                    self.open_url_overlay("custom".into());
                }
            }
            Some(provider_id) => {
                let provider_id = provider::aliases(provider_id);
                if let Some(preset) = provider::preset(&provider_id) {
                    if preset.asks_for_url && args.get(1).is_none() {
                        self.open_url_overlay(provider_id);
                    } else {
                        self.start_provider(provider_id, args.get(1).cloned());
                    }
                } else {
                    self.status =
                        format!("Unknown provider '{provider_id}'; use /connect to browse");
                }
            }
        }
    }

    fn open_url_overlay(&mut self, provider: String) {
        self.input.clear();
        self.cursor = 0;
        self.status = match provider::preset(&provider) {
            Some(preset) if preset.base_url.is_some() => {
                format!(
                    "Enter {} base URL; Enter uses the local default",
                    preset.label
                )
            }
            _ => "Enter an OpenAI-compatible http(s) base URL".into(),
        };
        self.overlay = Overlay::CustomUrl { provider };
    }

    fn command_mcp(&mut self, args: &[String]) -> Result<()> {
        match args.first().map(String::as_str) {
            Some("reconnect") => {
                let server = args.get(1).cloned();
                self.status = match server.as_deref() {
                    Some(name) => format!("Reconnecting MCP server {name}…"),
                    None => "Reconnecting MCP servers…".into(),
                };
                let sender = self.tx.clone();
                tokio::spawn(async move {
                    let notice = match crate::mcp::reconnect(server.as_deref()).await {
                        Ok(message) => message,
                        Err(error) => format!("MCP reconnect failed: {error:#}"),
                    };
                    let _ = sender.send(UiEvent::Notice(notice));
                });
            }
            Some("tools") => {
                let tools = crate::mcp::definitions();
                self.push_system(
                    &serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".into()),
                );
            }
            Some("list") | Some("status") | None => {
                self.push_system(&serde_json::to_string_pretty(&crate::mcp::status())?);
            }
            Some(other) => {
                self.status = format!("Usage: /mcp list|tools|reconnect [server] (got {other})");
            }
        }
        Ok(())
    }

    fn start_provider(&mut self, id: String, entered: Option<String>) {
        let Some(preset) = provider::preset(&id) else {
            self.status = format!("Unknown provider '{id}'");
            return;
        };
        let entered_url = entered
            .as_deref()
            .filter(|value| provider::valid_url(value));
        let mut connection = provider::resolve_connection(Some(preset.id), None, entered_url);
        if preset.api_key_required && entered.is_none() && connection.api_key.is_none() {
            self.overlay = Overlay::ApiKey {
                provider: preset.id.to_string(),
            };
            self.status = format!(
                "{} requires {} (the key stays in memory)",
                preset.label,
                preset.api_key_env.unwrap_or("an API key")
            );
            return;
        }
        if preset.id == "custom" {
            if entered_url.is_none() {
                self.status = "Custom connections require a valid URL".into();
                return;
            }
        } else if entered_url.is_none()
            && let Some(key) = entered
        {
            connection.api_key = Some(key);
        }
        match self.backend.with_connection(connection.clone()) {
            Ok(candidate) => {
                self.status = format!("Checking {} and loading models…", preset.label);
                self.pending_connection = Some(connection);
                let sender = self.tx.clone();
                tokio::spawn(async move {
                    match candidate.list_models().await {
                        Ok(value) => {
                            let models = extract_models(&value);
                            let _ = sender.send(UiEvent::ModelsLoaded {
                                backend: candidate,
                                models,
                            });
                        }
                        Err(error) => {
                            let _ = sender
                                .send(UiEvent::Notice(format!("connection failed: {error:#}")));
                        }
                    }
                });
            }
            Err(error) => self.status = format!("connection failed: {error}"),
        }
    }

    fn start_model_list(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        self.status = "Refreshing model list…".into();
        tokio::spawn(async move {
            match backend.list_models().await {
                Ok(value) => {
                    let models = extract_models(&value);
                    if models.is_empty() {
                        let _ = sender.send(UiEvent::Notice("Provider returned no models".into()));
                    } else {
                        let _ = sender.send(UiEvent::ModelsLoaded { backend, models });
                    }
                }
                Err(error) => {
                    let _ = sender.send(UiEvent::Notice(format!("model list failed: {error:#}")));
                }
            }
        });
    }

    fn start_health(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        self.status = "Checking backend…".into();
        tokio::spawn(async move {
            let notice = match backend.health().await {
                Ok(value) => {
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                }
                Err(error) => format!("health failed: {error:#}"),
            };
            let _ = sender.send(UiEvent::Notice(notice));
        });
    }

    fn start_profiles(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        self.status = "Loading router profiles…".into();
        tokio::spawn(async move {
            let notice = match backend.profiles().await {
                Ok(value) => {
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                }
                Err(error) => format!("profiles failed: {error:#}"),
            };
            let _ = sender.send(UiEvent::Notice(notice));
        });
    }

    fn persist_connection(&mut self, model: Option<&String>) {
        let Ok(mut config) = Config::load(&self.paths) else {
            self.status = "connected, but could not read config to persist connection".into();
            return;
        };
        let connection = self.backend.connection();
        config.provider = connection.provider_id.clone();
        config.backend = Some(connection.backend.clone());
        config.url = Some(connection.base_url.clone());
        if let Some(model) = model {
            config.model = Some(model.clone());
        }
        if let Err(error) = config.save(&self.paths) {
            self.status = format!("connected, but could not save config: {error}");
        }
    }

    fn command_workspace(&mut self, args: &[String]) {
        if let Some(path) = args.first() {
            let path = PathBuf::from(path).expanduser();
            if let Err(error) = std::fs::create_dir_all(&path) {
                self.status = format!("workspace error: {error}");
            } else {
                self.state.workspace = path;
                self.refresh_git_branch();
                self.refresh_custom_commands();
                self.status = format!("Workspace: {}", self.state.workspace.display());
            }
        } else {
            self.push_system(&format!("Workspace: {}", self.state.workspace.display()));
        }
    }

    fn command_session(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            Some("save") => {
                let name = args.get(1).map(String::as_str).unwrap_or("default");
                match session::save(&self.paths, name, &self.state) {
                    Ok(path) => self.status = format!("Saved {}", path.display()),
                    Err(error) => self.status = format!("Session save failed: {error}"),
                }
            }
            Some("load") => {
                let name = args.get(1).map(String::as_str).unwrap_or("default");
                match session::load(&self.paths, name, &mut self.state) {
                    Ok(count) => {
                        self.redo_stack.clear();
                        self.follow_transcript = true;
                        self.status = format!("Loaded {name} ({count} messages)");
                    }
                    Err(error) => self.status = format!("Session load failed: {error}"),
                }
            }
            Some("list") => {
                let items = session::list(&self.paths);
                self.push_system(&serde_json::to_string_pretty(&items).unwrap_or_default());
            }
            Some("search") => {
                let query = args.get(1).map(String::as_str).unwrap_or_default();
                self.push_system(
                    &serde_json::to_string_pretty(&session::search(&self.paths, query))
                        .unwrap_or_default(),
                );
            }
            Some("delete") => {
                let name = args.get(1).map(String::as_str).unwrap_or_default();
                self.status = match session::delete(&self.paths, name) {
                    Ok(true) => format!("Deleted session {name}"),
                    Ok(false) => format!("Session not found: {name}"),
                    Err(error) => format!("Session delete failed: {error}"),
                };
            }
            Some("diff") => {
                let name = args.get(1).map(String::as_str).unwrap_or("default");
                match session::diff(&self.paths, name, &self.state) {
                    Ok(value) => self.push_system(&value),
                    Err(error) => self.status = format!("Session diff failed: {error}"),
                }
            }
            Some("fork") => {
                let Some(name) = args.get(1).map(String::as_str) else {
                    self.status = "Usage: /session fork <name>".into();
                    return;
                };
                match session::save(&self.paths, name, &self.state) {
                    Ok(path) => {
                        self.status = format!(
                            "Forked current session as {name} ({}); keep working here or /session load {name}",
                            path.display()
                        );
                    }
                    Err(error) => self.status = format!("Session fork failed: {error}"),
                }
            }
            _ => self.status = "Usage: /session save|load|list|search|delete|diff|fork".into(),
        }
    }

    fn command_undo(&mut self) {
        if self.busy {
            self.status = "Finish the active request before undoing".into();
            return;
        }
        let Some(index) = self
            .state
            .history
            .iter()
            .rposition(|message| message.role == "user")
        else {
            self.status = "Nothing to undo".into();
            return;
        };
        let removed: Vec<Message> = self.state.history.drain(index..).collect();
        let prompt = removed.first().map(|item| item.content.clone());
        let Some(prompt) = prompt else {
            self.status = "Nothing to undo".into();
            return;
        };
        let count = removed.len();
        self.redo_stack.push(UndoEntry { messages: removed });
        self.input = prompt;
        self.cursor = self.input.len();
        self.at_cache_key.clear();
        self.follow_transcript = true;
        self.status = format!("Undid {count} message(s); prompt restored · /redo to re-apply");
    }

    fn command_redo(&mut self) {
        if self.busy {
            self.status = "Finish the active request before redoing".into();
            return;
        }
        let Some(entry) = self.redo_stack.pop() else {
            self.status = "Nothing to redo".into();
            return;
        };
        let count = entry.messages.len();
        self.state.history.extend(entry.messages);
        self.follow_transcript = true;
        self.status = format!("Redid {count} message(s)");
    }

    /// Any new user-authored turn invalidates the redo stack.
    fn command_editor(&mut self) {
        let editor = std::env::var("EDITOR").unwrap_or_default();
        if editor.trim().is_empty() {
            self.status = "Set $EDITOR to compose prompts externally (e.g. EDITOR=nvim)".into();
            return;
        }
        self.editor_requested = true;
    }

    fn command_thinking(&mut self, args: &[String]) {
        self.state.show_thinking = toggle_value(args.first(), self.state.show_thinking);
        let enabled = self.state.show_thinking;
        self.persist_config(|config| config.show_thinking = enabled);
        self.status = format!(
            "Thinking blocks: {} (display only; effort via /reasoning)",
            if enabled { "shown" } else { "hidden" }
        );
    }

    fn command_attention(&mut self, args: &[String]) {
        self.attention_bell = toggle_value(args.first(), self.attention_bell);
        let enabled = self.attention_bell;
        self.persist_config(|config| config.attention_bell = enabled);
        self.status = format!("Completion bell: {}", if enabled { "on" } else { "off" });
    }

    fn command_export(&mut self, args: &[String]) {
        let format = args.first().map(String::as_str).unwrap_or("markdown");
        let extension = match format {
            "md" | "markdown" => "md",
            "txt" | "text" => "txt",
            "json" => "json",
            "html" => "html",
            "pdf" => "pdf",
            _ => {
                self.status = format!("Unsupported export format: {format}");
                return;
            }
        };
        let path = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
            self.state
                .workspace
                .join(format!("r105-export.{extension}"))
        });
        let path = if path.is_absolute() {
            path
        } else {
            match safe_path(&self.state.workspace, &path.to_string_lossy()) {
                Ok(path) => path,
                Err(error) => {
                    self.status = format!("Export path rejected: {error}");
                    return;
                }
            }
        };
        match export::write(&self.state, format, &path) {
            Ok(()) => self.status = format!("Exported {}", path.display()),
            Err(error) => self.status = format!("Export failed: {error}"),
        }
    }

    fn command_theme(&mut self, args: &[String]) {
        if let Some(theme) = args.first() {
            if THEMES.contains(&theme.as_str()) {
                self.state.theme = theme.clone();
                self.persist_config(|config| config.theme = theme.clone());
                self.status = format!("Theme: {theme}");
            } else {
                self.status = format!("Unknown theme; choose {}", THEMES.join(", "));
            }
            return;
        }
        let selected = THEMES
            .iter()
            .position(|name| *name == self.state.theme)
            .unwrap_or(0);
        self.overlay = Overlay::Theme {
            selected,
            original: self.state.theme.clone(),
        };
        self.status = "Choose a theme — preview is live, Enter keeps it, Esc reverts".into();
    }

    fn workspace_map(&self) -> String {
        let mut lines = Vec::new();
        for entry in walkdir::WalkDir::new(&self.state.workspace)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != self.state.workspace)
            .take(80)
        {
            let relative = entry
                .path()
                .strip_prefix(&self.state.workspace)
                .unwrap_or(entry.path());
            lines.push(format!(
                "{} {}",
                if entry.file_type().is_dir() {
                    "▸"
                } else {
                    "·"
                },
                relative.display()
            ));
        }
        if lines.is_empty() {
            "Workspace is empty".into()
        } else {
            lines.join("\n")
        }
    }

    fn workspace_diff(&self) -> String {
        match OsCommand::new("git")
            .args([
                "-C",
                &self.state.workspace.to_string_lossy(),
                "diff",
                "--stat",
            ])
            .output()
        {
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if value.is_empty() {
                    "No Git changes".into()
                } else {
                    value
                }
            }
            Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
            Err(error) => format!("git diff unavailable: {error}"),
        }
    }

    fn push_system(&mut self, content: &str) {
        self.state
            .history
            .push(Message::system(content.to_string()));
        self.follow_transcript = true;
    }

    fn cancel_work(&mut self, status: &str) {
        if let Some(token) = &self.cancellation {
            token.cancel();
            self.status = status.into();
        } else if !self.busy {
            self.status = status.into();
        }
    }

    fn insert_text(&mut self, value: &str) {
        self.input.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = previous_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
    }

    fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let end = next_boundary(&self.input, self.cursor);
        self.input.drain(self.cursor..end);
    }

    fn history_previous(&mut self) {
        if self.state.history.is_empty() {
            return;
        }
        let value = self
            .state
            .history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|m| m.content.clone());
        if let Some(value) = value {
            self.input = value;
            self.cursor = self.input.len();
        }
    }

    fn history_next(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    fn palette_active(&self) -> bool {
        matches!(self.overlay, Overlay::None)
            && self.input.starts_with('/')
            && !self.input.chars().any(char::is_whitespace)
    }

    fn palette_items(&self) -> Vec<command::PaletteItem> {
        command::palette_items(self.input.trim(), &self.custom_commands)
    }

    /// Exact `/name` match against loaded custom commands (input is
    /// already lowercased by the parser; names are stored lowercased).
    fn is_custom_command(&self, value: &str) -> bool {
        self.custom_commands
            .iter()
            .any(|command| format!("/{}", command.name) == value.to_ascii_lowercase())
    }

    fn find_custom_command(&self, name: &str) -> Option<CustomCommand> {
        let name = name.strip_prefix('/').unwrap_or(name).to_ascii_lowercase();
        self.custom_commands
            .iter()
            .find(|command| command.name == name)
            .cloned()
    }

    /// The `@path` token immediately before the cursor, if any. The `@`
    /// must start the line or follow whitespace so `user@host` never counts.
    fn at_token(&self) -> Option<String> {
        let end = self.cursor.min(self.input.len());
        let before = self.input.get(..end)?;
        let at = before.rfind('@')?;
        if at > 0
            && !before
                .get(..at)
                .is_some_and(|head| head.ends_with(char::is_whitespace))
        {
            return None;
        }
        let token = before.get(at + 1..)?;
        if token.is_empty() || !token.chars().all(is_path_char) {
            return None;
        }
        Some(token.to_string())
    }

    fn at_menu_items(&mut self) -> Vec<String> {
        if !matches!(self.overlay, Overlay::None) || self.palette_active() {
            return Vec::new();
        }
        let Some(query) = self.at_token() else {
            return Vec::new();
        };
        let key = format!("{}:{}", self.input, self.cursor);
        if key == self.at_cache_key {
            return self.at_cache_items.clone();
        }
        let items = complete_files(&self.state.workspace, &query);
        self.at_cache_key = key;
        self.at_selected = 0;
        self.at_cache_items = items.clone();
        items
    }

    fn at_menu_open(&mut self) -> bool {
        !self.at_menu_items().is_empty()
    }

    fn at_menu_active(&mut self) -> bool {
        !self.at_cache_key.is_empty()
            && self.at_token().is_some()
            && !self.at_cache_items.is_empty()
            && format!("{}:{}", self.input, self.cursor) == self.at_cache_key
    }

    /// Replace the `@token` before the cursor with the selected pick.
    /// Directories keep a trailing `/` so completion can continue; files
    /// take a trailing space so typing resumes naturally.
    fn accept_at_complete(&mut self) -> bool {
        let items = self.at_cache_items.clone();
        let Some(pick) = items.get(self.at_selected).cloned() else {
            return false;
        };
        let end = self.cursor.min(self.input.len());
        let before = self.input[..end].to_string();
        let Some(at) = before.rfind('@') else {
            return false;
        };
        let suffix = if self.state.workspace.join(&pick).is_dir() {
            "/"
        } else {
            " "
        };
        let replacement = format!("@{pick}{suffix}");
        self.input.replace_range(at..end, &replacement);
        self.cursor = at + replacement.len();
        self.at_cache_key.clear();
        self.at_cache_items.clear();
        self.at_selected = 0;
        true
    }

    /// Value completion for a command's argument (`/theme dr<Tab>`). Only
    /// the argument right after the command (or one subcommand deeper for
    /// `/skill` and `/session`) completes; anything more complex stays
    /// manual. Requires the cursor at the end of a single-line input so
    /// replacement is a plain suffix swap.
    fn arg_menu_items(&mut self) -> Vec<String> {
        if !matches!(self.overlay, Overlay::None)
            || self.palette_active()
            || self.cursor != self.input.len()
            || self.input.contains('\n')
            || self.at_token().is_some()
        {
            return Vec::new();
        }
        let trailing_space = self.input.ends_with(char::is_whitespace);
        let mut words: Vec<&str> = self.input.split_whitespace().collect();
        if words.is_empty() || !words[0].starts_with('/') {
            return Vec::new();
        }
        // `/cmd ` (trailing space) means an empty token is being completed.
        if trailing_space {
            words.push("");
        }
        if words.len() != 2 && words.len() != 3 {
            return Vec::new();
        }
        let key = format!("{}:{}", self.input, self.cursor);
        if key == self.arg_cache_key {
            return self.arg_cache_items.clone();
        }
        let token = words.last().unwrap_or(&"").to_ascii_lowercase();
        let mut candidates = self.arg_candidates(words[0], words.get(1));
        candidates.retain(|candidate| candidate.to_ascii_lowercase().starts_with(&token));
        candidates.truncate(8);
        self.arg_cache_key = key;
        self.arg_selected = 0;
        self.arg_cache_items = candidates.clone();
        candidates
    }

    /// Candidate values for the argument under the cursor. `first` is the
    /// already-typed first argument when completing the second position.
    fn arg_candidates(&self, command: &str, first: Option<&&str>) -> Vec<String> {
        let command = command.to_ascii_lowercase();
        // Second position: names of skills and sessions behind their
        // subcommands.
        if let Some(first) = first.filter(|_| self.arg_position() == 2) {
            match (command.as_str(), first.to_ascii_lowercase().as_str()) {
                ("/skill", "use" | "show" | "drop") => return self.skill_names(),
                ("/session", "load" | "delete" | "diff") => {
                    return session::list(&self.paths)
                        .iter()
                        .map(|item| item.name.clone())
                        .collect();
                }
                _ => return Vec::new(),
            }
        }
        match command.as_str() {
            "/model" => self.known_models.clone(),
            "/preview" => complete_files(&self.state.workspace, &self.arg_token()),
            "/connect" => {
                let mut values: Vec<String> = command::static_arg_values("/connect")
                    .unwrap_or_default()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                values.extend(provider::PRESETS.iter().map(|preset| preset.id.to_string()));
                values.sort();
                values.dedup();
                values
            }
            _ => command::static_arg_values(command.as_str())
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// Which argument position the cursor completes: 1 for `/cmd <tok>`,
    /// 2 for `/cmd <fixed> <tok>`.
    fn arg_position(&self) -> usize {
        let words = self.input.split_whitespace().count();
        if self.input.ends_with(char::is_whitespace) {
            words
        } else {
            words.saturating_sub(1)
        }
    }

    /// The partial token after the last space (empty when the input ends
    /// with a space).
    fn arg_token(&self) -> String {
        if self.input.ends_with(char::is_whitespace) {
            return String::new();
        }
        self.input
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string()
    }

    /// Sorted skill stems (`review` for `review.md`), mirroring `/skills`.
    fn skill_names(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(&self.state.skills_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                (entry.path().extension().and_then(|value| value.to_str()) == Some("md")).then(
                    || {
                        entry
                            .path()
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default()
                            .to_string()
                    },
                )
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn arg_menu_open(&mut self) -> bool {
        !self.arg_menu_items().is_empty()
    }

    fn arg_menu_active(&mut self) -> bool {
        !self.arg_cache_key.is_empty()
            && !self.arg_cache_items.is_empty()
            && format!("{}:{}", self.input, self.cursor) == self.arg_cache_key
    }

    /// Replace the partial argument after the last space with the selected
    /// pick plus a trailing space so typing resumes naturally.
    fn accept_arg_complete(&mut self) -> bool {
        let items = self.arg_cache_items.clone();
        let Some(pick) = items.get(self.arg_selected).cloned() else {
            return false;
        };
        let Some(space) = self.input.rfind(' ') else {
            return false;
        };
        self.input.replace_range(space + 1.., &format!("{pick} "));
        self.cursor = self.input.len();
        self.arg_cache_key.clear();
        self.arg_cache_items.clear();
        self.arg_selected = 0;
        true
    }

    fn settings_rows(&self) -> Vec<(String, String)> {
        vec![
            ("Theme".into(), self.state.theme.clone()),
            ("Permissions".into(), self.state.permission_posture.clone()),
            ("Reasoning".into(), self.state.reasoning_effort.clone()),
            (
                "Autocompact".into(),
                if self.state.auto_compact {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
            (
                "Show thinking".into(),
                if self.state.show_thinking {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
            (
                "Bell on done".into(),
                if self.attention_bell {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
        ]
    }

    fn cycle_setting(&mut self, index: usize, direction: i32) {
        match index {
            0 => {
                let position = THEMES
                    .iter()
                    .position(|name| *name == self.state.theme)
                    .unwrap_or(0);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(THEMES.len() - 1)
                };
                let theme = THEMES[next].to_string();
                self.state.theme = theme.clone();
                self.persist_config(|config| config.theme = theme.clone());
                self.status = format!("Theme: {theme}");
            }
            1 => {
                const VALID: [&str; 4] = ["full-access", "restricted", "sandboxed", "off"];
                let position = VALID
                    .iter()
                    .position(|name| *name == self.state.permission_posture)
                    .unwrap_or(2);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(VALID.len() - 1)
                };
                let value = VALID[next].to_string();
                self.state.permission_posture = value.clone();
                self.persist_config(|config| config.permission_posture = value.clone());
                self.status = format!("Permission posture: {value}");
            }
            2 => {
                const VALID: [&str; 5] = ["auto", "off", "low", "medium", "high"];
                let position = VALID
                    .iter()
                    .position(|name| *name == self.state.reasoning_effort)
                    .unwrap_or(0);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(VALID.len() - 1)
                };
                let value = VALID[next].to_string();
                self.state.reasoning_effort = value.clone();
                self.persist_config(|config| config.reasoning_effort = value.clone());
                self.status = format!("Reasoning effort: {value}");
            }
            3 => {
                self.state.auto_compact = !self.state.auto_compact;
                let enabled = self.state.auto_compact;
                self.persist_config(|config| config.auto_compact = enabled);
                self.status = format!(
                    "Automatic compaction: {}",
                    if enabled { "on" } else { "off" }
                );
            }
            4 => {
                self.state.show_thinking = !self.state.show_thinking;
                let enabled = self.state.show_thinking;
                self.persist_config(|config| config.show_thinking = enabled);
                self.status = format!(
                    "Thinking blocks: {}",
                    if enabled { "shown" } else { "hidden" }
                );
            }
            5 => {
                self.attention_bell = !self.attention_bell;
                let enabled = self.attention_bell;
                self.persist_config(|config| config.attention_bell = enabled);
                self.status = format!("Completion bell: {}", if enabled { "on" } else { "off" });
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        let palette = self.palette_items();
        let palette_height = if self.palette_active() && !palette.is_empty() {
            palette.len().min(8) as u16 + 2
        } else {
            0
        };
        let file_items = self.at_menu_items();
        let arg_items = self.arg_menu_items();
        // The `@file` and argument-value menus never co-show (the latter
        // requires no `@` token), so they share one chunk.
        let complete_rows = file_items.len().max(arg_items.len());
        let file_height = if complete_rows == 0 {
            0
        } else {
            complete_rows.min(8) as u16 + 2
        };
        if !palette.is_empty() {
            self.palette_selected = self.palette_selected.min(palette.len() - 1);
            self.palette_scroll = command::ensure_visible(
                self.palette_selected,
                self.palette_scroll,
                palette_height.saturating_sub(2) as usize,
                palette.len(),
            );
        }
        let composer_lines = self.input.lines().count().max(1) as u16;
        let composer_height = (composer_lines + 2).clamp(3, 7);
        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(3),
                Constraint::Length(palette_height),
                Constraint::Length(file_height),
                Constraint::Length(composer_height),
                Constraint::Length(2),
            ])
            .split(area);
        self.draw_header(frame, chunks[0]);
        self.draw_transcript(frame, chunks[1]);
        if palette_height > 0 {
            self.draw_palette(frame, chunks[2], &palette);
        }
        if file_height > 0 {
            if !file_items.is_empty() {
                self.draw_file_complete(frame, chunks[3], &file_items);
            } else {
                self.draw_arg_complete(frame, chunks[3], &arg_items);
            }
        }
        self.draw_composer(frame, chunks[4]);
        self.draw_footer(frame, chunks[5]);
        self.draw_overlay(frame, area);
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let connection = self.backend.connection();
        let accent = accent_color(&self.state.theme);
        let title = Line::from(vec![
            Span::styled(
                " r105 ",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "AI harness",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} · {}", self.mode.as_str(), connection.display_name()),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled(self.state.model.clone(), Style::default().fg(Color::Green)),
        ]);
        let workspace = self.state.workspace.display().to_string();
        let context = format!(
            "{} · {} · {} skill{}",
            workspace,
            self.sandbox.selected_name(),
            self.skills_available,
            if self.skills_available == 1 { "" } else { "s" }
        );
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(context, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(
                if self.busy {
                    "● working"
                } else {
                    "○ ready"
                },
                Style::default().fg(if self.busy {
                    Color::Yellow
                } else {
                    Color::Green
                }),
            ),
        ]);
        frame.render_widget(Paragraph::new(vec![title, line]), area);
    }

    fn draw_transcript(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let mut lines = Vec::new();
        let palette = theme_palette(&self.state.theme);
        let show_thinking = self.state.show_thinking;
        let thinking_expanded = self.state.thinking_default_expanded;
        for message in &self.state.history {
            let color = match message.role.as_str() {
                "user" => palette.user,
                "assistant" => palette.assistant,
                "tool" => palette.tool,
                _ => Color::Magenta,
            };
            let label = message.role.to_ascii_uppercase();
            lines.push(Line::from(Span::styled(
                format!(" {label} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            if message.role == "assistant"
                && let Some(body) = thinking_body(&message.content)
            {
                push_thinking_lines(&mut lines, body, show_thinking, thinking_expanded);
            } else if message.role == "tool" && !self.show_details {
                // Error results always expand: a collapsed red line hides
                // exactly what the user needs to see.
                let failed = message.content.contains("tool error:");
                if failed {
                    for line in message.content.lines() {
                        lines.push(Line::from(format!("  {line}")));
                    }
                } else {
                    let first = message.content.lines().next().unwrap_or_default();
                    lines.push(Line::from(Span::styled(
                        format!("  {}…", first.chars().take(100).collect::<String>()),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            } else {
                for line in message.content.lines() {
                    lines.push(Line::from(format!("  {line}")));
                }
            }
            if !message.tool_calls.is_empty() {
                let names: Vec<String> = message
                    .tool_calls
                    .iter()
                    .map(|c| c.function.name.clone())
                    .collect();
                lines.push(Line::from(Span::styled(
                    format!(
                        "  ↳ {} tool call(s): {}",
                        message.tool_calls.len(),
                        tool_names(&names)
                    ),
                    Style::default().fg(Color::Yellow),
                )));
            }
            lines.push(Line::from(""));
        }
        if !self.streaming.is_empty() {
            lines.push(Line::from(Span::styled(
                " ASSISTANT ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in self.streaming.lines() {
                lines.push(Line::from(format!("  {line}")));
            }
        }
        let max_scroll = lines.len().saturating_sub(area.height as usize);
        if self.follow_transcript {
            self.transcript_scroll = max_scroll;
        } else {
            self.transcript_scroll = self.transcript_scroll.min(max_scroll);
            if self.transcript_scroll >= max_scroll {
                // Scrolled back to the bottom (e.g. via mouse wheel):
                // re-follow new output.
                self.follow_transcript = true;
            }
        }
        let block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" transcript ");
        frame.render_widget(
            Paragraph::new(lines)
                .block(block)
                .wrap(Wrap { trim: false })
                .scroll((self.transcript_scroll.min(u16::MAX as usize) as u16, 0)),
            area,
        );
    }

    fn draw_file_complete(&mut self, frame: &mut Frame<'_>, area: Rect, items: &[String]) {
        let viewport = area.height.saturating_sub(2) as usize;
        let scroll = command::ensure_visible(self.at_selected, 0, viewport, items.len());
        let selection = selection_style(&self.state.theme);
        let rows = items
            .iter()
            .skip(scroll)
            .take(viewport)
            .enumerate()
            .map(|(offset, item)| {
                let index = scroll + offset;
                let style = if index == self.at_selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(format!(" @{item}"), style))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" files · Tab accept · ↑↓ choose "),
            ),
            area,
        );
    }

    fn draw_arg_complete(&mut self, frame: &mut Frame<'_>, area: Rect, items: &[String]) {
        let viewport = area.height.saturating_sub(2) as usize;
        let scroll = command::ensure_visible(self.arg_selected, 0, viewport, items.len());
        let selection = selection_style(&self.state.theme);
        let rows = items
            .iter()
            .skip(scroll)
            .take(viewport)
            .enumerate()
            .map(|(offset, item)| {
                let index = scroll + offset;
                let style = if index == self.arg_selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(format!(" {item}"), style))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" values · Tab accept · ↑↓ choose "),
            ),
            area,
        );
    }

    fn draw_settings(&self, frame: &mut Frame<'_>, area: Rect, selected: usize) {
        let rows = self.settings_rows();
        let height = (rows.len() as u16 + 4).min(area.height.max(1));
        let width = 56.min(area.width.max(1));
        let rect = centered(area, width, height);
        let selection = selection_style(&self.state.theme);
        let lines = rows
            .iter()
            .enumerate()
            .map(|(index, (label, value))| {
                let style = if index == selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(format!(" {label:<14} {value}"), style))
            })
            .collect::<Vec<_>>();
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" settings · ↑↓ move · ←/→ change · Esc close "),
            ),
            rect,
        );
    }

    fn draw_palette(&mut self, frame: &mut Frame<'_>, area: Rect, items: &[command::PaletteItem]) {
        let viewport = area.height.saturating_sub(2) as usize;
        let scroll = command::ensure_visible(
            self.palette_selected,
            self.palette_scroll,
            viewport,
            items.len(),
        );
        self.palette_scroll = scroll;
        let selection = selection_style(&self.state.theme);
        let rows = items
            .iter()
            .skip(scroll)
            .take(viewport)
            .enumerate()
            .map(|(offset, item)| {
                let index = scroll + offset;
                let style = if index == self.palette_selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                // `*` flags Markdown-backed rows, matching `/help`.
                let name = if item.custom {
                    format!("{}*", item.name)
                } else {
                    item.name.clone()
                };
                Line::from(Span::styled(
                    format!(" {name:<18} {}", item.description),
                    style,
                ))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" commands · ↑↓ choose · Enter accept · * custom "),
            ),
            area,
        );
    }

    fn draw_composer(&self, frame: &mut Frame<'_>, area: Rect) {
        let title = if self.busy {
            format!(
                " composer · {} queued · Esc/Ctrl-X cancel ",
                self.queue.len()
            )
        } else {
            " composer · Enter send · Alt/Shift-Enter newline ".into()
        };
        frame.render_widget(
            Paragraph::new(format!("> {}", self.input))
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let usage = self.state.token_usage();
        let percent = usage.percent();
        let filled = (percent / 10.0).round() as usize;
        let bar = format!(
            "{}{}",
            "█".repeat(filled.min(10)),
            "░".repeat(10usize.saturating_sub(filled.min(10)))
        );
        let session_tokens = if self.session_in == 0 && self.session_out == 0 {
            "tokens –".to_string()
        } else {
            format!(
                "↑{} ↓{}",
                compact_number(self.session_in),
                compact_number(self.session_out)
            )
        };
        let branch = self
            .git_branch
            .as_deref()
            .map(|name| format!(" ⎇{name}"))
            .unwrap_or_default();
        let first = Line::from(vec![
            Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            Span::styled(session_tokens, Style::default().fg(Color::DarkGray)),
            Span::styled(branch, Style::default().fg(Color::DarkGray)),
        ]);
        let second = Line::from(vec![
            Span::styled(
                format!(
                    " context {bar} {:.0}% {}/{}",
                    percent, usage.used_tokens, usage.context_tokens
                ),
                Style::default().fg(if percent > 85.0 {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw("  "),
            Span::styled(
                "Tab mode · /help · @file · !cmd · /sh",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(vec![first, second]), area);
    }

    fn draw_overlay(&mut self, frame: &mut Frame<'_>, area: Rect) {
        if let Overlay::Settings { selected } = &self.overlay {
            // Read first so the &mut match below is not needed for settings.
            let selected = *selected;
            self.draw_settings(frame, area, selected);
            return;
        }
        match &mut self.overlay {
            Overlay::None => {}
            Overlay::Providers { selected, scroll } => {
                let items = provider::PRESETS
                    .iter()
                    .map(provider_label)
                    .collect::<Vec<_>>();
                render_picker(
                    frame,
                    area,
                    " Connect · provider ",
                    &items,
                    selected,
                    scroll,
                    selection_style(&self.state.theme),
                );
            }
            Overlay::Models {
                items,
                selected,
                scroll,
                active,
            } => {
                let display: Vec<String> =
                    items.iter().map(|model| model.display(active)).collect();
                render_picker(
                    frame,
                    area,
                    " Models · ● active · Enter select ",
                    &display,
                    selected,
                    scroll,
                    selection_style(&self.state.theme),
                );
            }
            Overlay::ApiKey { provider } => {
                let rect = centered(area, 72, 7);
                frame.render_widget(Clear, rect);
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(format!("API key for {provider}")),
                        Line::from(""),
                        Line::from(format!("  {}", "•".repeat(self.input.chars().count()))),
                        Line::from(""),
                        Line::from("Enter submit · Esc cancel · key is never saved"),
                    ])
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Credentials "),
                    ),
                    rect,
                );
            }
            Overlay::Theme { selected, .. } => {
                let items: Vec<String> = THEMES
                    .iter()
                    .map(|name| {
                        if *name == self.state.theme {
                            format!("● {name} (active)")
                        } else {
                            format!("  {name}")
                        }
                    })
                    .collect();
                // Theme picker is small; reuse the shared picker renderer
                // with a zero scroll so ↑/↓ + live preview stay consistent.
                let mut scroll = 0usize;
                // Preview uses the highlighted row's theme so the whole
                // screen recolors live as you move.
                let preview = THEMES.get(*selected).copied().unwrap_or("r105");
                render_picker(
                    frame,
                    area,
                    " Theme · live preview · Enter keep · Esc revert ",
                    &items,
                    selected,
                    &mut scroll,
                    selection_style(preview),
                );
            }
            Overlay::Settings { .. } => {}
            Overlay::CustomUrl { provider } => {
                let rect = centered(area, 78, 7);
                frame.render_widget(Clear, rect);
                let preset = provider::preset(provider);
                let title = preset
                    .map(|preset| format!(" {} connection ", preset.label))
                    .unwrap_or_else(|| " Custom connection ".into());
                let prompt = preset
                    .map(|preset| format!("{} base URL", preset.label))
                    .unwrap_or_else(|| "OpenAI-compatible base URL".into());
                let hint = preset
                    .and_then(|preset| preset.base_url)
                    .map(|default_url| format!("Empty = {default_url} · LAN: replace 127.0.0.1"))
                    .unwrap_or_else(|| "Enter submit · Ctrl+A clear · Esc cancel".into());
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from(prompt),
                        Line::from(""),
                        Line::from(format!("  {}", self.input)),
                        Line::from(""),
                        Line::from(hint),
                    ])
                    .block(Block::default().borders(Borders::ALL).title(title)),
                    rect,
                );
            }
        }
    }
}

const MAX_TOOL_ROUNDS: usize = 8;

/// Remappable composer actions and their built-in defaults. Only
/// `ctrl+<letter>` specs are accepted: predictable to parse, hard to typo
/// into something destructive. Ctrl+C, Esc, Tab, and Enter stay fixed.
const KEY_ACTIONS: [(&str, char); 5] = [
    ("cancel", 'x'),
    ("details", 'o'),
    ("tasks", 't'),
    ("history", 'r'),
    ("redraw", 'l'),
];

fn is_known_key_action(action: &str) -> bool {
    KEY_ACTIONS.iter().any(|(name, _)| *name == action)
}

fn ctrl_spec_letter(spec: &str) -> Option<char> {
    let rest = spec.trim().to_ascii_lowercase();
    let letter = rest.strip_prefix("ctrl+")?;
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(first), None) if first.is_ascii_alphabetic() => Some(first),
        _ => None,
    }
}

fn valid_ctrl_spec(spec: &str) -> bool {
    ctrl_spec_letter(spec).is_some()
}

fn match_ctrl_spec(spec: &str, key: &KeyEvent) -> bool {
    let Some(letter) = ctrl_spec_letter(spec) else {
        return false;
    };
    key.modifiers == KeyModifiers::CONTROL
        && matches!(key.code, KeyCode::Char(found) if found.to_ascii_lowercase() == letter)
}

fn is_path_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '_' | '/' | '.' | '-' | '~' | '\\' | '+')
}

/// `@path` tokens in a prompt, in order and deduplicated. The `@` must open
/// the line or follow whitespace so `user@host` never counts as a file.
fn extract_file_refs(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut refs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'@' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'_' | b'/' | b'.' | b'-' | b'~' | b'\\' | b'+'))
            {
                end += 1;
            }
            if end > index + 1 {
                let token = &input[index + 1..end];
                if !refs.iter().any(|item| item == token) {
                    refs.push(token.to_string());
                }
            }
            index = end;
        } else {
            index += 1;
        }
    }
    refs
}

fn score_file_candidate(relative: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = relative.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(&query) {
        return Some(800 - relative.len().min(700) as i32);
    }
    let file = candidate.rsplit('/').next().unwrap_or(&candidate);
    if file.starts_with(&query) {
        return Some(700 - relative.len().min(600) as i32);
    }
    if candidate.contains(&query) {
        return Some(500 - relative.len().min(400) as i32);
    }
    // Ordered-subsequence fallback so `mrs` still finds `main.rs`.
    let mut rest = candidate.chars();
    for needle in query.chars() {
        rest.find(|value| *value == needle)?;
    }
    Some(100)
}

/// Fuzzy workspace files for `@` completion. Skips hidden entries, `.git`,
/// and build output; capped so a huge tree cannot stall a keystroke.
fn complete_files(workspace: &Path, query: &str) -> Vec<String> {
    let mut scored: Vec<(i32, String)> = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .take(400)
    {
        let relative = entry.path().strip_prefix(workspace).unwrap_or(entry.path());
        if relative.as_os_str().is_empty() {
            continue;
        }
        let display = relative.display().to_string().replace('\\', "/");
        if display
            .split('/')
            .any(|part| part.starts_with('.') || part == "target" || part == "node_modules")
        {
            continue;
        }
        if let Some(score) = score_file_candidate(&display, query) {
            scored.push((score, display));
        }
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored.truncate(8);
    scored.into_iter().map(|(_, path)| path).collect()
}

/// Reduce a `/sh` model reply to one runnable line: drop ```` ``` ````
/// fences, skip blanks, strip a leading `$ ` prompt echo. `None` when
/// nothing usable remains.
fn clean_shell_draft(output: &str) -> Option<String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```"))?;
    let line = line.strip_prefix("$ ").unwrap_or(line).trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Reasoning-only replies arrive wrapped by the backend; the wrapper always
/// covers the whole message, so ordinary model text is never misread.
fn thinking_body(content: &str) -> Option<&str> {
    content
        .trim()
        .strip_prefix("<thinking>")
        .and_then(|rest| rest.strip_suffix("</thinking>"))
        .map(str::trim)
}

fn push_thinking_lines(lines: &mut Vec<Line>, body: &str, show: bool, expanded: bool) {
    let dim = Style::default().fg(Color::DarkGray);
    if !show {
        lines.push(Line::from(Span::styled(
            "  ⋯ thinking hidden (/thinking to show)",
            dim,
        )));
        return;
    }
    let body_lines: Vec<&str> = body.lines().collect();
    if body_lines.is_empty() {
        lines.push(Line::from(Span::styled("  ⋯ empty thinking block", dim)));
    } else if expanded || body_lines.len() <= 3 {
        for line in &body_lines {
            lines.push(Line::from(Span::styled(format!("  {line}"), dim)));
        }
    } else {
        for line in body_lines.iter().take(2) {
            lines.push(Line::from(Span::styled(format!("  {line}"), dim)));
        }
        lines.push(Line::from(Span::styled(
            format!("  ⋯ {} more thinking lines", body_lines.len() - 2),
            dim,
        )));
    }
}

/// Fenced code blocks of a response, in order. Unclosed fences are ignored.
fn code_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            if let Some(block) = current.take() {
                blocks.push(block);
            } else {
                current = Some(String::new());
            }
            continue;
        }
        if let Some(block) = current.as_mut() {
            if !block.is_empty() {
                block.push('\n');
            }
            block.push_str(line);
        }
    }
    blocks
}

fn compact_number(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 1_000_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    }
}

fn slow_start_due(
    busy: bool,
    awaiting: bool,
    shown: bool,
    started: Option<Instant>,
    now: Instant,
) -> bool {
    busy && awaiting
        && !shown
        && started.is_some_and(|start| now.duration_since(start) >= Duration::from_secs(15))
}

fn git_branch_for(workspace: &Path) -> Option<String> {
    let output = OsCommand::new("git")
        .args([
            "-C",
            &workspace.to_string_lossy(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// Global custom-command directory: `commands/` under the config root,
/// next to `skills/` and `sessions/`.
fn commands_dir(paths: &ConfigPaths) -> PathBuf {
    paths.config_dir.join("commands")
}

/// Project custom-command directory inside the active workspace.
fn project_commands_dir(workspace: &Path) -> PathBuf {
    workspace.join(".r105").join("commands")
}

fn count_skills(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(|value| value != '.')
                })
                .count()
        })
        .unwrap_or(0)
}

fn tool_names(names: &[String]) -> String {
    const MAX_SHOWN: usize = 3;
    const MAX_CHARS: usize = 80;
    if names.is_empty() {
        return "none".into();
    }
    let mut text = names
        .iter()
        .take(MAX_SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_SHOWN {
        text.push_str(&format!(", +{} more", names.len() - MAX_SHOWN));
    }
    if text.len() > MAX_CHARS {
        text.truncate(MAX_CHARS);
        text.push('…');
    }
    text
}

fn provider_label(preset: &Preset) -> String {
    format!(
        "{}  ·  {}  [{}]",
        preset.label, preset.description, preset.id
    )
}

fn python_bridge_status(spec: Option<&str>) -> String {
    crate::python_bridge::status(spec)
}

fn toggle_value(value: Option<&String>, current: bool) -> bool {
    match value.map(|value| value.to_ascii_lowercase()).as_deref() {
        None => !current,
        Some("on" | "true" | "yes" | "1") => true,
        Some("off" | "false" | "no" | "0") => false,
        Some(_) => current,
    }
}

fn copy_to_clipboard(value: &str) -> bool {
    let commands: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("clip", &[]),
    ];
    for (program, args) in commands {
        let Ok(mut child) = OsCommand::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let Some(mut stdin) = child.stdin.take() else {
            continue;
        };
        if stdin.write_all(value.as_bytes()).is_err() {
            let _ = child.kill();
            continue;
        }
        drop(stdin);
        if child.wait().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}

fn skill_name(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return None;
    }
    let name = if trimmed.ends_with(".md") {
        trimmed.strip_suffix(".md")?.to_string()
    } else {
        trimmed.to_string()
    };
    let path = Path::new(&name);
    (path.components().count() == 1
        && path
            .components()
            .next()
            .is_some_and(|component| matches!(component, std::path::Component::Normal(_))))
    .then_some(name)
}

/// Named theme palette: accent drives the header + selection highlight,
/// role colors drive transcript labels. High-contrast maximizes separation.
struct ThemePalette {
    accent: Color,
    user: Color,
    assistant: Color,
    tool: Color,
}

fn theme_palette(theme: &str) -> ThemePalette {
    match theme {
        "dracula" => ThemePalette {
            accent: Color::Magenta,
            user: Color::Cyan,
            assistant: Color::Magenta,
            tool: Color::Yellow,
        },
        "solarized-dark" => ThemePalette {
            accent: Color::Blue,
            user: Color::Blue,
            assistant: Color::Green,
            tool: Color::Yellow,
        },
        "high-contrast" => ThemePalette {
            accent: Color::Yellow,
            user: Color::White,
            assistant: Color::White,
            tool: Color::Yellow,
        },
        _ => ThemePalette {
            accent: Color::Cyan,
            user: Color::Cyan,
            assistant: Color::Green,
            tool: Color::Yellow,
        },
    }
}

fn accent_color(theme: &str) -> Color {
    theme_palette(theme).accent
}

fn selection_style(theme: &str) -> Style {
    if theme == "high-contrast" {
        return Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD);
    }
    Style::default()
        .fg(Color::Black)
        .bg(accent_color(theme))
        .add_modifier(Modifier::BOLD)
}

fn render_picker(
    frame: &mut Frame<'_>,
    screen: Rect,
    title: &str,
    items: &[String],
    selected: &mut usize,
    scroll: &mut usize,
    selection: Style,
) {
    let height = (items.len().min(screen.height.saturating_sub(8) as usize) as u16 + 4)
        .max(8)
        .min(screen.height);
    let width = screen.width.saturating_sub(8).min(104);
    let rect = centered(screen, width, height);
    let viewport = rect.height.saturating_sub(4) as usize;
    *selected = (*selected).min(items.len().saturating_sub(1));
    *scroll = command::ensure_visible(*selected, *scroll, viewport, items.len());
    let lines = items
        .iter()
        .skip(*scroll)
        .take(viewport)
        .enumerate()
        .map(|(offset, item)| {
            let index = *scroll + offset;
            let style = if index == *selected {
                selection
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(
                format!(" {} {}", if index == *selected { "▶" } else { " " }, item),
                style,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: false }),
        rect,
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn extract_models(value: &Value) -> Vec<ModelInfo> {
    let source = value.get("data").or_else(|| value.get("models"));
    let Some(items) = source.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut models: Vec<ModelInfo> = Vec::new();
    for item in items {
        let (id, status) = match item {
            Value::String(value) => (Some(value.clone()), None),
            Value::Object(object) => {
                let id = object
                    .get("id")
                    .or_else(|| object.get("name"))
                    .or_else(|| object.get("model"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let status = object.get("status").and_then(|status| match status {
                    Value::String(value) => Some(value.clone()),
                    Value::Object(object) => object
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    _ => None,
                });
                (id, status)
            }
            _ => (None, None),
        };
        if let Some(id) = id
            && !models.iter().any(|model| model.id == id)
        {
            models.push(ModelInfo { id, status });
        }
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models
}

fn previous_boundary(input: &str, cursor: usize) -> usize {
    input[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_boundary(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .chars()
        .next()
        .map(|value| cursor + value.len_utf8())
        .unwrap_or(cursor)
}

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
        (
            UiApp::new(backend, state, paths, config, false),
            workspace,
            skills,
        )
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
