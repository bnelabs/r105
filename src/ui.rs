//! Native Ratatui harness UI.
//!
//! The screen keeps the transcript central, makes the composer persistent,
//! and moves secondary work into small overlays. Long-running requests and
//! tools are cancellable, messages sent while busy are queued, and every
//! picker shares the same visible-window calculation.

use std::{
    collections::VecDeque,
    io::{self, stdout},
    path::{Path, PathBuf},
    process::Command as OsCommand,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
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
    ModelsLoaded {
        backend: Backend,
        models: Vec<String>,
    },
    Notice(String),
}

#[derive(Debug)]
enum Overlay {
    None,
    Providers {
        selected: usize,
        scroll: usize,
    },
    Models {
        items: Vec<String>,
        selected: usize,
        scroll: usize,
    },
    ApiKey {
        provider: String,
    },
    CustomUrl,
}

pub async fn run(
    backend: Backend,
    state: ChatState,
    paths: ConfigPaths,
    plugins_dir: PathBuf,
) -> Result<()> {
    let mut terminal = setup_terminal()?;
    let result = UiApp::new(backend, state, paths, plugins_dir)
        .event_loop(&mut terminal)
        .await;
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut output = stdout();
    execute!(output, EnterAlternateScreen).context("entering alternate screen")?;
    let backend = CrosstermBackend::new(output);
    Terminal::new(backend).context("creating terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode().context("disabling terminal raw mode")?;
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
    queue: VecDeque<String>,
    active_user: Option<String>,
    tool_round: usize,
    cancellation: Option<CancellationToken>,
    pending_connection: Option<Connection>,
    sandbox: Sandbox,
    last_response: String,
    tx: mpsc::UnboundedSender<UiEvent>,
    rx: mpsc::UnboundedReceiver<UiEvent>,
    quit: bool,
}

impl UiApp {
    fn new(backend: Backend, state: ChatState, paths: ConfigPaths, plugins_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let config = Config::load(&paths).unwrap_or_default();
        let sandbox = Sandbox::detect(
            &config.sandbox_backend,
            config.docker_image.clone(),
            config.timeout_seconds,
        );
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
            status: "Ready".into(),
            queue: VecDeque::new(),
            active_user: None,
            tool_round: 0,
            cancellation: None,
            pending_connection: None,
            sandbox,
            last_response: String::new(),
            tx,
            rx,
            quit: false,
        }
    }

    async fn event_loop(
        mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        loop {
            self.process_events();
            terminal.draw(|frame| self.draw(frame))?;
            if self.quit {
                if let Some(cancellation) = &self.cancellation {
                    cancellation.cancel();
                }
                if !self.state.history.is_empty()
                    && let Err(error) = session::save(&self.paths, "__autosave__", &self.state)
                {
                    eprintln!("warning: could not save autosession: {error}");
                }
                break;
            }
            if event::poll(Duration::from_millis(45))?
                && let Event::Key(key) = event::read()?
            {
                self.handle_key(key).await?;
            }
            tokio::task::yield_now().await;
        }
        Ok(())
    }

    fn process_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                UiEvent::Backend(BackendEvent::Token(token)) => {
                    self.streaming.push_str(&token);
                    self.status = "Generating…".into();
                    self.follow_transcript = true;
                }
                UiEvent::Backend(BackendEvent::Status(status)) => self.status = status,
                UiEvent::ChatDone(result) => self.chat_done(result),
                UiEvent::ChatError(error) => {
                    self.streaming.clear();
                    self.busy = false;
                    self.cancellation = None;
                    self.status = format!("Request failed: {error}");
                    self.start_next_queued();
                }
                UiEvent::ToolsDone(results) => {
                    for result in &results {
                        self.state.history.push(Message::tool(
                            result.call_id.clone(),
                            format!("[{}]\n{}", result.name, result.content),
                        ));
                    }
                    self.status = format!("{} tool result(s) received; continuing…", results.len());
                    self.start_continue();
                }
                UiEvent::Compacted { summary, recent } => {
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
                        self.status =
                            format!("Connected; choose a model ({} available)", models.len());
                        self.overlay = Overlay::Models {
                            items: models,
                            selected: 0,
                            scroll: 0,
                        };
                    }
                }
                UiEvent::Notice(notice) => self.push_system(&notice),
            }
        }
    }

    fn chat_done(&mut self, result: ChatResult) {
        self.streaming.clear();
        self.state.last_usage = result.usage.clone();
        if let Some(prompt) = self.active_user.take() {
            self.state.history.push(Message::user(prompt));
        }
        self.state.history.push(Message::assistant_with_tools(
            result.content.clone(),
            result.tool_calls.clone(),
        ));
        self.last_response = result.content.clone();
        self.follow_transcript = true;

        if !result.tool_calls.is_empty() && self.tool_round < 8 {
            self.tool_round += 1;
            self.status = format!("Running {} tool call(s)…", result.tool_calls.len());
            let context = ToolContext {
                workspace: self.state.workspace.clone(),
                plugins_dir: self.plugins_dir.clone(),
                sandbox: self.sandbox.clone(),
                cancellation: self.cancellation.clone().unwrap_or_default(),
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
            if self.state.auto_compact
                && self.state.history.len() >= 4
                && self.state.token_usage().percent() >= 80.0
            {
                self.start_compaction(true);
            } else {
                self.busy = false;
                self.cancellation = None;
                self.status = format!("Done in {:.2}s", result.wall_seconds);
                self.start_next_queued();
            }
        }
    }

    fn start_prompt(&mut self, prompt: String) {
        if self.busy {
            self.queue.push_back(prompt);
            self.status = format!("Queued prompt ({} waiting)", self.queue.len());
            return;
        }
        self.busy = true;
        self.active_user = Some(prompt.clone());
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
        if let Some(prompt) = self.queue.pop_front() {
            self.start_prompt(prompt);
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('o') {
            self.show_details = !self.show_details;
            self.status = if self.show_details {
                "Details expanded".into()
            } else {
                "Details collapsed".into()
            };
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
            self.status = "History search: type /session search <term>".into();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
            self.transcript_scroll = 0;
            self.follow_transcript = true;
            self.status = "Redrawn".into();
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
                        if id == "custom" {
                            self.input.clear();
                            self.cursor = 0;
                            self.status = "Enter an OpenAI-compatible http(s) base URL".into();
                            self.overlay = Overlay::CustomUrl;
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
                            self.state.model = model.clone();
                            let model = model.clone();
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
            Overlay::CustomUrl => match key.code {
                KeyCode::Enter => {
                    let value = self.input.trim().to_string();
                    self.input.clear();
                    self.cursor = 0;
                    if provider::valid_url(&value) {
                        self.start_provider("custom".into(), Some(value));
                    } else {
                        self.status = "Enter an http:// or https:// URL without credentials".into();
                        self.overlay = Overlay::CustomUrl;
                    }
                }
                KeyCode::Char(character) => {
                    self.insert_text(&character.to_string());
                    self.overlay = Overlay::CustomUrl;
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.overlay = Overlay::CustomUrl;
                }
                _ => self.overlay = Overlay::CustomUrl,
            },
            Overlay::None => {}
        }
        Ok(())
    }

    async fn submit(&mut self) -> Result<()> {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        if self.palette_active() && command::command(&value).is_none() {
            if let Some(item) = self.palette_items().get(self.palette_selected) {
                self.input = format!("{} ", item.name);
                self.cursor = self.input.len();
            }
            return Ok(());
        }
        self.input.clear();
        self.cursor = 0;
        if let Some(parsed) = command::parse(&value) {
            self.handle_command(parsed).await
        } else {
            self.start_prompt(value);
            Ok(())
        }
    }

    async fn handle_command(&mut self, parsed: ParsedCommand) -> Result<()> {
        match parsed.name.as_str() {
            "/help" => self.push_system(&command::help_text()),
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
            "/cache-prompt" => {
                self.state.cache_prompt = parsed
                    .args
                    .first()
                    .map(|value| value != "off")
                    .unwrap_or(!self.state.cache_prompt);
                self.status = format!(
                    "llama.cpp prompt caching: {}",
                    if self.state.cache_prompt { "on" } else { "off" }
                );
            }
            "/clear" => {
                self.state.history.clear();
                self.streaming.clear();
                self.status = "Transcript cleared".into();
            }
            "/workspace" => self.command_workspace(&parsed.args),
            "/session" => self.command_session(&parsed.args),
            "/export" => self.command_export(&parsed.args),
            "/mcp" => self.command_mcp(&parsed.args)?,
            "/plugin" => self.push_system(&serde_json::to_string_pretty(&crate::plugin::status())?),
            "/theme" => self.command_theme(&parsed.args),
            "/map" => self.push_system(&self.workspace_map()),
            "/diff" => self.push_system(&self.workspace_diff()),
            "/copy" => {
                self.status = if self.last_response.is_empty() {
                    "There is no response to copy".into()
                } else {
                    "Last response is selected for copy; use your terminal clipboard command if needed".into()
                }
            }
            "/tasks" => self.push_system(&format!(
                "busy={} queued={} tool_round={}",
                self.busy,
                self.queue.len(),
                self.tool_round
            )),
            "/exit" => self.quit = true,
            _ => self.status = format!("Unknown command {}; type /help", parsed.name),
        }
        Ok(())
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
                    self.overlay = Overlay::CustomUrl;
                    self.status = "Enter an OpenAI-compatible http(s) base URL".into();
                }
            }
            Some(provider_id) => {
                if provider::preset(provider_id).is_some() {
                    self.start_provider(provider_id.to_string(), args.get(1).cloned());
                } else {
                    self.status =
                        format!("Unknown provider '{provider_id}'; use /connect to browse");
                }
            }
        }
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
                    Ok(count) => self.status = format!("Loaded {name} ({count} messages)"),
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
            _ => self.status = "Usage: /session save|load|list|search|delete|diff".into(),
        }
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
        let valid = ["r105", "dracula", "solarized-dark", "high-contrast"];
        if let Some(theme) = args.first() {
            if valid.contains(&theme.as_str()) {
                self.state.theme = theme.clone();
                self.status = format!("Theme: {theme}");
            } else {
                self.status = format!("Unknown theme; choose {}", valid.join(", "));
            }
        } else {
            self.push_system(&format!("Theme: {}", self.state.theme));
        }
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

    fn palette_items(&self) -> Vec<&'static command::CommandSpec> {
        command::filtered(self.input.trim())
    }

    fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        let area = frame.area();
        let palette = self.palette_items();
        let palette_height = if self.palette_active() && !palette.is_empty() {
            palette.len().min(8) as u16 + 2
        } else {
            0
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
                Constraint::Length(composer_height),
                Constraint::Length(2),
            ])
            .split(area);
        self.draw_header(frame, chunks[0]);
        self.draw_transcript(frame, chunks[1]);
        if palette_height > 0 {
            self.draw_palette(frame, chunks[2], &palette);
        }
        self.draw_composer(frame, chunks[3]);
        self.draw_footer(frame, chunks[4]);
        self.draw_overlay(frame, area);
    }

    fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let connection = self.backend.connection();
        let title = Line::from(vec![
            Span::styled(
                " r105 ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
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
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(workspace, Style::default().fg(Color::DarkGray)),
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
        for message in &self.state.history {
            let color = match message.role.as_str() {
                "user" => Color::Cyan,
                "assistant" => Color::Green,
                "tool" => Color::Yellow,
                _ => Color::Magenta,
            };
            let label = message.role.to_ascii_uppercase();
            lines.push(Line::from(Span::styled(
                format!(" {label} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            if message.role == "tool" && !self.show_details {
                let first = message.content.lines().next().unwrap_or_default();
                lines.push(Line::from(Span::styled(
                    format!("  {}…", first.chars().take(100).collect::<String>()),
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for line in message.content.lines() {
                    lines.push(Line::from(format!("  {line}")));
                }
            }
            if !message.tool_calls.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  ↳ {} tool call(s)", message.tool_calls.len()),
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

    fn draw_palette(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        items: &[&'static command::CommandSpec],
    ) {
        let viewport = area.height.saturating_sub(2) as usize;
        let scroll = command::ensure_visible(
            self.palette_selected,
            self.palette_scroll,
            viewport,
            items.len(),
        );
        self.palette_scroll = scroll;
        let rows = items
            .iter()
            .skip(scroll)
            .take(viewport)
            .enumerate()
            .map(|(offset, item)| {
                let index = scroll + offset;
                let style = if index == self.palette_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(
                    format!(" {:<18} {}", item.name, item.description),
                    style,
                ))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" commands · ↑↓ choose · Enter accept "),
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
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "context {bar} {:.0}% {}/{}",
                    percent, usage.used_tokens, usage.context_tokens
                ),
                Style::default().fg(if percent > 85.0 {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            ),
            Span::raw("  "),
            Span::styled("Tab mode · /help", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn draw_overlay(&mut self, frame: &mut Frame<'_>, area: Rect) {
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
                );
            }
            Overlay::Models {
                items,
                selected,
                scroll,
            } => {
                render_picker(
                    frame,
                    area,
                    " Models · provider response ",
                    items,
                    selected,
                    scroll,
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
            Overlay::CustomUrl => {
                let rect = centered(area, 78, 7);
                frame.render_widget(Clear, rect);
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("OpenAI-compatible base URL"),
                        Line::from(""),
                        Line::from(format!("  {}", self.input)),
                        Line::from(""),
                        Line::from(
                            "Example: https://api.example.com/v1 · Enter submit · Esc cancel",
                        ),
                    ])
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" Custom connection "),
                    ),
                    rect,
                );
            }
        }
    }
}

fn provider_label(preset: &Preset) -> String {
    format!(
        "{}  ·  {}  [{}]",
        preset.label, preset.description, preset.id
    )
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

fn render_picker(
    frame: &mut Frame<'_>,
    screen: Rect,
    title: &str,
    items: &[String],
    selected: &mut usize,
    scroll: &mut usize,
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
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
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

fn extract_models(value: &Value) -> Vec<String> {
    let source = value.get("data").or_else(|| value.get("models"));
    let Some(items) = source.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut models = items
        .iter()
        .filter_map(|item| match item {
            Value::String(value) => Some(value.clone()),
            Value::Object(object) => object
                .get("id")
                .or_else(|| object.get("name"))
                .or_else(|| object.get("model"))
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
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
