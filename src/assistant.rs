//! Headless AI orchestrator for windowed surfaces (spec 0023).
//!
//! Mirrors the TUI request lifecycle without any UI: stream the reply,
//! record history with the reasoning echo, precheck tool calls against
//! the approval policy, run the allowed subset in parallel, and follow
//! up until the model stops calling tools (at most [`MAX_TOOL_ROUNDS`]
//! rounds). Window surfaces drive it through [`AssistantHandle`] and
//! render [`AssistantEvent`]s; one verdict per `ApprovalNeeded`.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    approve::{self, Decision, Policy},
    backend::{Backend, BackendEvent},
    model::{ChatState, Message, ToolCall},
    sandbox::Sandbox,
    tool::{self, ToolContext, ToolResult},
};

#[cfg(test)]
mod lifecycle_tests;

/// Same round cap as the TUI so both surfaces stop together.
pub const MAX_TOOL_ROUNDS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalVerdict {
    Once,
    Always,
    Deny,
}

#[derive(Debug, Clone)]
pub enum AssistantEvent {
    Token(String),
    Reasoning(String),
    Status(String),
    ToolStarted(Vec<String>),
    ApprovalNeeded {
        id: String,
        index: usize,
        total: usize,
        name: String,
        summary: String,
        preview: Option<String>,
    },
    Done {
        response: String,
        tool_rounds: usize,
        wall_seconds: f64,
    },
    Error(String),
}

#[derive(Debug)]
enum Control {
    Ask(String, CancellationToken),
    Verdict(String, ApprovalVerdict),
}

/// Static inputs for the orchestrator task. The policy mutates inside
/// the task (session allows), so each window session gets one handle.
#[derive(Clone)]
pub struct AssistantParts {
    pub plugins_dir: PathBuf,
    pub sandbox: Sandbox,
    pub policy: Policy,
    pub persistence: Option<(crate::config::ConfigPaths, String)>,
}

impl AssistantParts {
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            plugins_dir: config.plugins_dir.clone(),
            sandbox: Sandbox::detect(
                &config.sandbox_backend,
                config.docker_image.clone(),
                config.timeout_seconds,
            ),
            policy: Policy::from_config(config).unwrap_or_else(|_| Policy::locked_down()),
            persistence: None,
        }
    }
}

pub struct AssistantHandle {
    pub events: mpsc::UnboundedReceiver<AssistantEvent>,
    control: mpsc::UnboundedSender<Control>,
    cancel: Arc<Mutex<Option<CancellationToken>>>,
}

impl AssistantHandle {
    /// Queue a prompt. Prompts run in order; concurrent asks wait.
    pub fn ask(&self, prompt: String) {
        let token = CancellationToken::new();
        let mut active = self.cancel.lock().unwrap();
        if active.is_none() {
            *active = Some(token.clone());
        }
        let _ = self.control.send(Control::Ask(prompt, token));
    }

    /// Answer the pending approval card. Exactly one verdict per
    /// `ApprovalNeeded`; extras are ignored by the task.
    pub fn verdict(&self, id: String, verdict: ApprovalVerdict) {
        let _ = self.control.send(Control::Verdict(id, verdict));
    }

    /// Cancel the in-flight run. Queued prompts survive.
    pub fn cancel(&self) {
        if let Some(cancel) = self.cancel.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    }
}

impl Drop for AssistantHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Spawn the orchestrator task. It owns `state` (history persists
/// across prompts for the window session) and reports over `events`.
pub fn spawn_assistant(
    backend: Backend,
    state: ChatState,
    parts: AssistantParts,
) -> AssistantHandle {
    let (events_tx, events_rx) = mpsc::unbounded_channel();
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(Mutex::new(None));
    let runner_cancel = cancel.clone();
    tokio::spawn(async move {
        run_session(backend, state, parts, events_tx, control_rx, runner_cancel).await;
    });
    AssistantHandle {
        events: events_rx,
        control: control_tx,
        cancel,
    }
}

fn emit(events: &mpsc::UnboundedSender<AssistantEvent>, event: AssistantEvent) {
    let _ = events.send(event);
}

async fn run_session(
    backend: Backend,
    mut state: ChatState,
    mut parts: AssistantParts,
    events: mpsc::UnboundedSender<AssistantEvent>,
    mut control: mpsc::UnboundedReceiver<Control>,
    active_cancel: Arc<Mutex<Option<CancellationToken>>>,
) {
    complete_pending_tools(&mut state);
    let mut queue: VecDeque<(String, CancellationToken)> = VecDeque::new();
    loop {
        while let Some((prompt, run_cancel)) = queue.pop_front() {
            if events.is_closed() {
                return;
            }
            *active_cancel.lock().unwrap() = Some(run_cancel.clone());
            run_prompt(
                &backend,
                &mut state,
                &mut parts,
                &events,
                &mut control,
                &mut queue,
                &run_cancel,
                prompt,
            )
            .await;
            *active_cancel.lock().unwrap() = None;
            complete_pending_tools(&mut state);
            checkpoint(&state, &parts, &events);
        }
        match control.recv().await {
            Some(Control::Ask(prompt, token)) => queue.push_back((prompt, token)),
            // No run is active: verdicts have nothing to do.
            Some(Control::Verdict(_, _)) => {}
            None => return,
        }
    }
}

/// Wait for the verdict to the current card. Queued prompts park in
/// `queue`; cancel aborts the run (returns `None`).
async fn await_verdict(
    control: &mut mpsc::UnboundedReceiver<Control>,
    queue: &mut VecDeque<(String, CancellationToken)>,
    run_cancel: &CancellationToken,
    approval_id: &str,
) -> Option<ApprovalVerdict> {
    loop {
        tokio::select! {
            _ = run_cancel.cancelled() => return None,
            message = control.recv() => match message {
                Some(Control::Verdict(id, verdict)) if id == approval_id => return Some(verdict),
                Some(Control::Verdict(_, _)) => {},
                Some(Control::Ask(prompt, token)) => queue.push_back((prompt,token)),
                None => return None,
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_prompt(
    backend: &Backend,
    state: &mut ChatState,
    parts: &mut AssistantParts,
    events: &mpsc::UnboundedSender<AssistantEvent>,
    control: &mut mpsc::UnboundedReceiver<Control>,
    queue: &mut VecDeque<(String, CancellationToken)>,
    run_cancel: &CancellationToken,
    prompt: String,
) {
    let started = std::time::Instant::now();
    let tools = tool::definitions_from(&parts.plugins_dir);
    let mut active_user = Some(prompt.clone());
    emit(
        events,
        AssistantEvent::Status(format!("Asking {}…", state.model)),
    );
    let mut result =
        match stream_forward(backend, state, Some(prompt), &tools, events, run_cancel).await {
            Ok(result) => result,
            Err(error) => {
                emit(events, AssistantEvent::Error(format!("{error:#}")));
                return;
            }
        };
    let mut tool_rounds = 0usize;
    loop {
        let (content, calls) = record_turn(state, &result, &mut active_user);
        checkpoint(state, parts, events);
        if calls.is_empty() {
            emit(
                events,
                AssistantEvent::Done {
                    response: content,
                    tool_rounds,
                    wall_seconds: started.elapsed().as_secs_f64(),
                },
            );
            return;
        }
        if tool_rounds >= MAX_TOOL_ROUNDS {
            emit(
                events,
                AssistantEvent::Status(format!(
                    "Tool-round limit ({MAX_TOOL_ROUNDS}) reached; {} call(s) not executed",
                    calls.len()
                )),
            );
            emit(
                events,
                AssistantEvent::Done {
                    response: content,
                    tool_rounds,
                    wall_seconds: started.elapsed().as_secs_f64(),
                },
            );
            return;
        }
        tool_rounds += 1;
        let names: Vec<String> = calls
            .iter()
            .map(|call| call.function.name.clone())
            .collect();
        emit(events, AssistantEvent::ToolStarted(names));
        let context = tool_context(state, parts, run_cancel);
        // Policy pre-check mirrors the TUI: allow now, deny now, or
        // pause on a card. Execution re-resolves inside `execute`, so
        // a verdict cannot be bypassed by a later path.
        let mut approved: Vec<(usize, ToolCall)> = Vec::new();
        let mut results: Vec<Option<ToolResult>> = vec![None; calls.len()];
        let mut cards: Vec<(usize, String, String, Option<String>)> = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            let (args, summary) = crate::ui::call_text(call);
            match approve::resolve(
                &call.function.name,
                &args,
                &context.mode,
                context.allow_code,
                context.allow_network,
                &context.policy,
            ) {
                Decision::Allow => approved.push((index, call.clone())),
                Decision::Deny(reason) => {
                    results[index] = Some(denied_result(
                        call,
                        format!("tool '{}' denied: {reason}", call.function.name),
                    ));
                }
                Decision::Ask(_) => {
                    let preview =
                        crate::ui::approval_preview(&call.function.name, &args, &context.workspace);
                    cards.push((index, call.function.name.clone(), summary, preview));
                }
            }
        }
        let total = cards.len();
        let mut execution_policy = parts.policy.clone();
        let mut cancelled = false;
        for (position, (index, name, summary, preview)) in cards.into_iter().enumerate() {
            let approval_id = uuid::Uuid::new_v4().to_string();
            emit(
                events,
                AssistantEvent::ApprovalNeeded {
                    id: approval_id.clone(),
                    index: position,
                    total,
                    name,
                    summary: summary.clone(),
                    preview,
                },
            );
            let Some(verdict) = await_verdict(control, queue, run_cancel, &approval_id).await
            else {
                cancelled = true;
                break;
            };
            let call = calls[index].clone();
            // The execution re-resolves policy as the floor: record the
            // exact call text so the grant the card gives is visible.
            let (grant_args, _) = crate::ui::call_text(&call);
            let touched = approve::touched_paths(&call.function.name, &grant_args);
            grant_verdict(
                &mut parts.policy,
                &mut execution_policy,
                verdict,
                &summary,
                &touched,
            );
            match verdict {
                ApprovalVerdict::Once => {
                    approved.push((index, call));
                    emit(
                        events,
                        AssistantEvent::Status(format!("Approved `{summary}`")),
                    );
                }
                ApprovalVerdict::Always => {
                    approved.push((index, call));
                    emit(
                        events,
                        AssistantEvent::Status(format!("Always allow `{summary}`")),
                    );
                }
                ApprovalVerdict::Deny => {
                    results[index] = Some(denied_result(&call, "denied by user".to_string()));
                    emit(
                        events,
                        AssistantEvent::Status(format!("Denied `{summary}`")),
                    );
                }
            }
        }
        if cancelled || run_cancel.is_cancelled() {
            emit(events, AssistantEvent::Status("cancelled".to_string()));
            let response = results
                .iter()
                .flatten()
                .map(|result| result.content.clone())
                .collect::<Vec<_>>()
                .join("\n");
            emit(
                events,
                AssistantEvent::Done {
                    response,
                    tool_rounds,
                    wall_seconds: started.elapsed().as_secs_f64(),
                },
            );
            return;
        }
        if approved.is_empty() {
            // Everything denied: report the denials back so the model
            // can adjust, exactly like the TUI round.
            let merged = merge_results(results);
            for outcome in &merged {
                state.history.push(Message::tool(
                    outcome.call_id.clone(),
                    outcome.content.clone(),
                ));
            }
            result = match stream_forward(backend, state, None, &tools, events, run_cancel).await {
                Ok(result) => result,
                Err(error) => {
                    emit(events, AssistantEvent::Error(format!("{error:#}")));
                    return;
                }
            };
            continue;
        }
        let order: Vec<ToolCall> = approved.iter().map(|(_, call)| call.clone()).collect();
        let mut context = tool_context(state, parts, run_cancel);
        context.policy = execution_policy;
        match tool::execute_calls(&order, &context, None).await {
            Ok(outcomes) => {
                for ((index, _), outcome) in approved.iter().zip(outcomes) {
                    results[*index] = Some(outcome);
                }
            }
            Err(error) => {
                emit(
                    events,
                    AssistantEvent::Error(format!("tool error: {error:#}")),
                );
                return;
            }
        }
        for outcome in merge_results(results) {
            state.history.push(Message::tool(
                outcome.call_id.clone(),
                outcome.content.clone(),
            ));
        }
        result = match stream_forward(backend, state, None, &tools, events, run_cancel).await {
            Ok(result) => result,
            Err(error) => {
                emit(events, AssistantEvent::Error(format!("{error:#}")));
                return;
            }
        };
    }
}

/// Once grants exist only in the context used for this execution batch.
fn grant_verdict(
    session: &mut Policy,
    execution: &mut Policy,
    verdict: ApprovalVerdict,
    summary: &str,
    touched: &[String],
) {
    match verdict {
        ApprovalVerdict::Once => execution.allow_session(summary),
        ApprovalVerdict::Always => {
            session.allow_session(summary);
            execution.allow_session(summary);
            for path in touched {
                session.allow_session_file(path);
                execution.allow_session_file(path);
            }
        }
        ApprovalVerdict::Deny => {}
    }
}

fn checkpoint(
    state: &ChatState,
    parts: &AssistantParts,
    events: &mpsc::UnboundedSender<AssistantEvent>,
) {
    if let Some((paths, name)) = &parts.persistence
        && let Err(error) = crate::session::save(paths, name, state)
    {
        emit(
            events,
            AssistantEvent::Status(format!("Session save failed: {error:#}")),
        );
    }
}

/// Interrupted rounds must still supply a result for every recorded tool call.
fn complete_pending_tools(state: &mut ChatState) {
    let Some(index) = state
        .history
        .iter()
        .rposition(|message| message.role == "assistant")
    else {
        return;
    };
    let pending: Vec<_> = state.history[index]
        .tool_calls
        .iter()
        .filter(|call| {
            !state.history[index + 1..]
                .iter()
                .any(|message| message.tool_call_id.as_deref() == Some(call.id.as_str()))
        })
        .map(|call| call.id.clone())
        .collect();
    for id in pending {
        state.history.push(Message::tool(
            id,
            "Tool did not complete: run stopped before a result was recorded.",
        ));
    }
}

fn tool_context(
    state: &ChatState,
    parts: &AssistantParts,
    run_cancel: &CancellationToken,
) -> ToolContext {
    ToolContext {
        workspace: state.workspace.clone(),
        plugins_dir: parts.plugins_dir.clone(),
        sandbox: parts.sandbox.clone(),
        cancellation: run_cancel.clone(),
        allow_network: state.permission_posture != "restricted"
            && state.permission_posture != "off",
        allow_code: state.permission_posture != "off",
        mode: state.mode.clone(),
        policy: parts.policy.clone(),
        todos: Arc::new(Mutex::new(Vec::new())),
    }
}

/// Stream one turn, forwarding tokens into assistant events.
async fn stream_forward(
    backend: &Backend,
    state: &ChatState,
    prompt: Option<String>,
    tools: &[Value],
    events: &mpsc::UnboundedSender<AssistantEvent>,
    run_cancel: &CancellationToken,
) -> Result<crate::model::ChatResult> {
    let (backend_tx, mut backend_rx) = mpsc::unbounded_channel::<BackendEvent>();
    let forward = events.clone();
    let pump = tokio::spawn(async move {
        while let Some(event) = backend_rx.recv().await {
            match event {
                BackendEvent::Token(text) => emit(&forward, AssistantEvent::Token(text)),
                BackendEvent::Reasoning(text) => emit(&forward, AssistantEvent::Reasoning(text)),
                BackendEvent::Status(text) => emit(&forward, AssistantEvent::Status(text)),
            }
        }
    });
    let outcome = match prompt {
        Some(prompt) => {
            backend
                .stream_chat(state, &prompt, tools, backend_tx, run_cancel.clone())
                .await
        }
        None => {
            backend
                .stream_continue(state, tools, backend_tx, run_cancel.clone())
                .await
        }
    };
    let _ = pump.await;
    outcome
}

/// Record a finished turn in history (TUI `chat_done` semantics) and
/// return the display text plus pending tool calls.
fn record_turn(
    state: &mut ChatState,
    result: &crate::model::ChatResult,
    active_user: &mut Option<String>,
) -> (String, Vec<ToolCall>) {
    if let Some(prompt) = active_user.take() {
        state.history.push(Message::user(prompt));
    }
    let reasoning = if result.reasoning.is_empty() {
        crate::ui::thinking_body(&result.content)
            .unwrap_or_default()
            .to_string()
    } else {
        result.reasoning.clone()
    };
    if reasoning.is_empty() {
        state.history.push(Message::assistant_with_tools(
            result.content.clone(),
            result.tool_calls.clone(),
        ));
    } else {
        state.history.push(Message::assistant_with_reasoning(
            result.content.clone(),
            reasoning,
            result.tool_calls.clone(),
        ));
    }
    (result.content.clone(), result.tool_calls.clone())
}

fn denied_result(call: &ToolCall, reason: String) -> ToolResult {
    ToolResult {
        name: call.function.name.clone(),
        call_id: call.id.clone(),
        content: format!("tool error: {reason}"),
    }
}

fn merge_results(results: Vec<Option<ToolResult>>) -> Vec<ToolResult> {
    results
        .into_iter()
        .map(|result| {
            result.unwrap_or_else(|| ToolResult {
                name: String::new(),
                call_id: String::new(),
                content: "tool error: dropped".into(),
            })
        })
        .collect()
}

/// Multiline composer buffer. Cursor is a character index; all ops
/// clamp so out-of-range states are unrepresentable.
#[derive(Debug, Clone, Default)]
pub struct ComposerState {
    text: String,
    cursor: usize,
}

impl ComposerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    fn byte_of(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map(|(byte, _)| byte)
            .unwrap_or(self.text.len())
    }

    pub fn insert_char(&mut self, ch: char) {
        let byte = self.byte_of(self.cursor);
        self.text.insert(byte, ch);
        self.cursor += 1;
    }

    pub fn insert_text(&mut self, text: &str) {
        let byte = self.byte_of(self.cursor);
        self.text.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let end = self.byte_of(self.cursor);
        let start = self.byte_of(self.cursor - 1);
        self.text.drain(start..end);
        self.cursor -= 1;
    }

    /// Ctrl+W: delete back to the previous word boundary.
    pub fn delete_word_back(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let bytes = self.text.as_bytes();
        let mut index = self.cursor;
        let is_space = |at: usize| {
            bytes[self.byte_of(at)..]
                .first()
                .is_some_and(|byte| (*byte as char).is_whitespace())
        };
        while index > 0 && is_space(index - 1) {
            index -= 1;
        }
        while index > 0 && !is_space(index - 1) {
            index -= 1;
        }
        let start = self.byte_of(index);
        let end = self.byte_of(self.cursor);
        self.text.drain(start..end);
        self.cursor = index;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
    }

    pub fn move_home(&mut self) {
        let byte = self.byte_of(self.cursor);
        let line_start = self.text[..byte].rfind('\n').map(|at| at + 1).unwrap_or(0);
        self.cursor = self.text[..line_start].chars().count();
    }

    pub fn move_end(&mut self) {
        let byte = self.byte_of(self.cursor);
        let rest = &self.text[byte..];
        let advance = rest
            .find('\n')
            .map(|at| self.text[byte..byte + at].chars().count())
            .unwrap_or_else(|| rest.chars().count());
        self.cursor += advance;
    }
}

/// One AI answer in the window timeline.
#[derive(Debug, Clone)]
pub struct AiBlock {
    pub seq: u64,
    pub prompt: String,
    pub response: String,
    pub reasoning: String,
    pub tool_rounds: usize,
    pub wall_seconds: f64,
    pub done: bool,
    pub error: Option<String>,
}

/// Append-only AI answers with sequence numbers starting at 1.
#[derive(Debug, Default)]
pub struct AiHistory {
    blocks: Vec<AiBlock>,
    next_seq: u64,
}

impl AiHistory {
    pub fn from_messages(messages: &[Message]) -> Self {
        let mut history = Self::new();
        let mut current = None;
        for message in messages {
            if message.role == "user" {
                current = Some(history.begin(&message.content));
            } else if message.role == "assistant"
                && let Some(seq) = current
            {
                if !message.content.is_empty() {
                    if history
                        .blocks
                        .last()
                        .is_some_and(|block| !block.response.is_empty())
                    {
                        history.push_token(seq, "\n\n");
                    }
                    history.push_token(seq, &message.content);
                }
                history.push_reasoning(seq, &message.reasoning_content);
                history.finish(seq, 0, 0.0);
            }
        }
        history
    }

    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_seq: 1,
        }
    }

    pub fn begin(&mut self, prompt: &str) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.blocks.push(AiBlock {
            seq,
            prompt: prompt.to_string(),
            response: String::new(),
            reasoning: String::new(),
            tool_rounds: 0,
            wall_seconds: 0.0,
            done: false,
            error: None,
        });
        seq
    }

    pub fn push_token(&mut self, seq: u64, text: &str) {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.seq == seq) {
            block.response.push_str(text);
        }
    }

    pub fn push_reasoning(&mut self, seq: u64, text: &str) {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.seq == seq) {
            block.reasoning.push_str(text);
        }
    }

    pub fn finish(&mut self, seq: u64, tool_rounds: usize, wall_seconds: f64) {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.seq == seq) {
            block.tool_rounds = tool_rounds;
            block.wall_seconds = wall_seconds;
            block.done = true;
        }
    }

    /// Fill a missed stream: when the UI never saw tokens (a dropped
    /// frame of events), the `Done` payload still carries the text.
    pub fn set_response_if_empty(&mut self, seq: u64, response: &str) {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.seq == seq)
            && block.response.is_empty()
        {
            block.response = response.to_string();
        }
    }

    pub fn fail(&mut self, seq: u64, error: String) {
        if let Some(block) = self.blocks.iter_mut().find(|block| block.seq == seq) {
            block.error = Some(error);
            block.done = true;
        }
    }

    pub fn list(&self) -> &[AiBlock] {
        &self.blocks
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_grants_have_the_requested_lifetime() {
        let args = serde_json::json!({"path": "note.txt", "content": "hello"});
        for verdict in [
            ApprovalVerdict::Once,
            ApprovalVerdict::Always,
            ApprovalVerdict::Deny,
        ] {
            let mut session = Policy {
                write: approve::Action::Ask,
                ..Policy::default()
            };
            let mut execution = session.clone();
            grant_verdict(
                &mut session,
                &mut execution,
                verdict,
                "write_file note.txt",
                &["note.txt".into()],
            );
            let resolve = |policy: &Policy| {
                approve::resolve("write_file", &args, "build", true, false, policy)
            };
            assert_eq!(
                matches!(resolve(&execution), Decision::Allow),
                verdict != ApprovalVerdict::Deny
            );
            assert_eq!(
                matches!(resolve(&session), Decision::Allow),
                verdict == ApprovalVerdict::Always
            );
        }
    }

    #[test]
    fn interrupted_tool_history_is_repaired_without_duplicate_results() {
        let mut state =
            ChatState::from_config(&crate::config::Config::default(), std::env::temp_dir());
        let calls = serde_json::from_value(serde_json::json!([
            {"id":"one","function":{"name":"read_file","arguments":"{}"}},
            {"id":"two","function":{"name":"read_file","arguments":"{}"}}
        ]))
        .unwrap();
        state.history.push(Message::assistant_with_tools("", calls));
        state.history.push(Message::tool("one", "existing result"));
        complete_pending_tools(&mut state);
        complete_pending_tools(&mut state);
        assert_eq!(state.history.len(), 3);
        assert_eq!(state.history[1].content, "existing result");
        assert_eq!(state.history[2].tool_call_id.as_deref(), Some("two"));
    }

    #[tokio::test]
    async fn cancelled_session_accepts_the_next_prompt() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 16384];
            assert!(first.read(&mut buffer).await.unwrap() > 0);
            started_tx.send(()).unwrap();
            let (mut second, _) = listener.accept().await.unwrap();
            assert!(second.read(&mut buffer).await.unwrap() > 0);
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"resumed\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            second.write_all(response.as_bytes()).await.unwrap();
        });
        let config = crate::config::Config::default();
        let backend = Backend::new(
            crate::backend::Connection {
                provider_id: None,
                backend: "direct".into(),
                base_url: format!("http://{address}"),
                api_key: None,
                model: "test".into(),
            },
            5,
        )
        .unwrap();
        let mut assistant = spawn_assistant(
            backend,
            ChatState::from_config(&config, std::env::temp_dir()),
            AssistantParts::from_config(&config),
        );
        assistant.cancel(); // Idle cancellation must also be harmless.
        assistant.ask("first".into());
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        assistant.ask("second".into());
        assistant.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match assistant.events.recv().await.unwrap() {
                    AssistantEvent::Done { response, .. } => {
                        assert_eq!(response, "resumed");
                        break;
                    }
                    _ => continue,
                }
            }
        })
        .await
        .unwrap();
        server.await.unwrap();
    }

    #[test]
    fn composer_inserts_and_moves() {
        let mut composer = ComposerState::new();
        composer.insert_text("hello");
        composer.insert_newline();
        composer.insert_text("world");
        assert_eq!(composer.text(), "hello\nworld");
        composer.move_left();
        composer.move_left();
        composer.backspace();
        assert_eq!(composer.text(), "hello\nwold");
    }

    #[test]
    fn composer_home_end_stay_on_line() {
        let mut composer = ComposerState::new();
        composer.insert_text("ab\ncdef");
        composer.move_home();
        composer.insert_char('X');
        assert_eq!(composer.text(), "ab\nXcdef");
        composer.move_end();
        composer.insert_char('Y');
        assert_eq!(composer.text(), "ab\nXcdefY");
    }

    #[test]
    fn composer_word_kill_stops_at_boundary() {
        let mut composer = ComposerState::new();
        composer.insert_text("write file foo.txt  ");
        composer.delete_word_back();
        assert_eq!(composer.text(), "write file ");
        composer.delete_word_back();
        assert_eq!(composer.text(), "write ");
        composer.clear();
        assert!(composer.text().is_empty());
        assert_eq!(composer.cursor(), 0);
    }

    #[test]
    fn composer_unicode_cursor_is_char_based() {
        let mut composer = ComposerState::new();
        composer.insert_text("héllo");
        assert_eq!(composer.cursor(), 5);
        composer.move_left();
        composer.insert_char('X');
        assert_eq!(composer.text(), "héllXo");
    }

    #[test]
    fn ai_history_streams_and_finishes() {
        let mut history = AiHistory::new();
        let seq = history.begin("2+2?");
        history.push_token(seq, "4");
        history.push_reasoning(seq, "math");
        history.finish(seq, 0, 1.5);
        let block = history.list().last().unwrap();
        assert_eq!(block.response, "4");
        assert_eq!(block.reasoning, "math");
        assert!(block.done);
        assert!(history.len() == 1 && !history.is_empty());
    }

    #[test]
    fn ai_history_fail_marks_error() {
        let mut history = AiHistory::new();
        let seq = history.begin("boom");
        history.fail(seq, "backend down".to_string());
        assert_eq!(
            history.list().last().unwrap().error.as_deref(),
            Some("backend down")
        );
    }

    #[test]
    fn session_allow_turns_ask_into_allow() {
        use crate::approve::{Action, Policy};
        use serde_json::json;
        let mut policy = Policy {
            exec: Action::Ask,
            ..Policy::default()
        };
        let args = json!({"code": "print(1)"});
        assert!(matches!(
            approve::resolve("execute_rust", &args, "build", true, false, &policy),
            Decision::Ask(_)
        ));
        policy.allow_session("execute_rust print(1)");
        assert!(matches!(
            approve::resolve("execute_rust", &args, "build", true, false, &policy),
            Decision::Allow
        ));
    }

    #[test]
    fn record_turn_echoes_reasoning_like_tui() {
        let config: crate::config::Config = serde_json::from_value(serde_json::json!({})).unwrap();
        let mut state = ChatState::from_config(&config, std::env::temp_dir());
        let result = crate::model::ChatResult {
            content: "done".to_string(),
            reasoning: "trace".to_string(),
            tool_calls: Vec::new(),
            raw: Value::Null,
            usage: crate::model::Usage::default(),
            wall_seconds: 0.1,
        };
        let mut active = Some("hi".to_string());
        let (content, calls) = record_turn(&mut state, &result, &mut active);
        assert_eq!(content, "done");
        assert!(calls.is_empty() && active.is_none());
        assert_eq!(state.history.len(), 2);
    }
}
