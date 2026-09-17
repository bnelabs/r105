//! UiApp approvals: pre-check partition, card decisions, ordered finalize.
//!
//! The model may request several calls at once; each resolves
//! independently to run-now, pre-denied, or card-gated. Cards resolve
//! sequentially and execution preserves the original call order, so the
//! follow-up request sees results exactly as requested.

use std::collections::VecDeque;

use serde_json::Value;

use super::*;
use crate::approve::{self, Decision};
use crate::model::ToolCall;

/// One call awaiting a card decision; `index` is its slot in the
/// original request so denied slots merge back in order.
pub(crate) struct PendingApproval {
    pub index: usize,
    pub name: String,
    pub summary: String,
    pub preview: Option<String>,
}

pub(crate) struct PendingTools {
    pub calls: Vec<ToolCall>,
    pub context: ToolContext,
    pub approved: Vec<(usize, ToolCall)>,
    pub results: Vec<Option<ToolResult>>,
    pub queue: VecDeque<PendingApproval>,
}

pub(crate) enum ApprovalVerdict {
    Once,
    Always,
    Deny,
}

/// Same lenient parse as `execute_calls`: an undecodable argument string
/// is still a value, so policy sees what execution will see.
fn precheck_args(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// Read-only preview for the approval card. Never writes.
/// Diff preview for a write-like call, shared with window approvals.
/// Pure filesystem read; `None` means no preview line.
pub(crate) fn approval_preview(
    name: &str,
    args: &Value,
    workspace: &std::path::Path,
) -> Option<String> {
    match name {
        "write_file" => {
            let path = args.get("path").and_then(Value::as_str)?;
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let old = std::fs::read_to_string(workspace.join(path)).unwrap_or_default();
            if old.is_empty() {
                Some(format!("create {path} ({} bytes)", content.len()))
            } else {
                Some(format!(
                    "{}: {} -> {} lines",
                    path,
                    old.lines().count(),
                    content.lines().count()
                ))
            }
        }
        "edit_file" => {
            let path = args.get("path").and_then(Value::as_str)?;
            let old_text = args
                .get("old_text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let new_text = args
                .get("new_text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let replace_all = args
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let current = std::fs::read_to_string(workspace.join(path)).unwrap_or_default();
            crate::edit::preview_edit(&current, old_text, new_text, replace_all)
                .map(|diff| format!("{path}: {diff}"))
                .ok()
        }
        "apply_patch" => {
            let patch = args
                .get("patch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            crate::edit::preview_patch(workspace, patch).ok()
        }
        _ => None,
    }
}

/// The exact text `execute` will match lists against: repaired arguments
/// (fenced JSON counts as the object it becomes), then summarized. Card
/// approvals allow-list this text, so pre-check and enforcement agree.
/// Shared with window approvals so both surfaces read identically.
pub(crate) fn call_text(call: &ToolCall) -> (Value, String) {
    let args = tool::repair_arguments(&precheck_args(&call.function.arguments));
    let summary = approve::summarize(&call.function.name, &args);
    (args, summary)
}

fn denied_result(call: &ToolCall, reason: String) -> ToolResult {
    ToolResult {
        name: call.function.name.clone(),
        call_id: call.id.clone(),
        content: format!("tool error: {reason}"),
    }
}

impl UiApp {
    /// Split fresh calls into run-now / pre-denied / card-gated. When
    /// cards take over the caller must return early: the round stays
    /// paused (`busy`) until every card resolves or cancel clears it.
    pub(crate) fn precheck_tool_calls(&mut self, calls: Vec<ToolCall>, context: ToolContext) {
        let mut approved = Vec::new();
        let mut results = vec![None; calls.len()];
        let mut queue = VecDeque::new();
        for (index, call) in calls.iter().enumerate() {
            let (args, summary) = call_text(call);
            let preview = approval_preview(&call.function.name, &args, &context.workspace);
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
                Decision::Ask(_) => queue.push_back(PendingApproval {
                    index,
                    name: call.function.name.clone(),
                    summary,
                    preview,
                }),
            }
        }
        if queue.is_empty() {
            if approved.is_empty() {
                // Everything pre-denied: skip the spawn, deliver in order.
                let merged = results
                    .into_iter()
                    .map(|result| {
                        result.unwrap_or_else(|| ToolResult {
                            name: String::new(),
                            call_id: String::new(),
                            content: "tool error: dropped".into(),
                        })
                    })
                    .collect();
                let _ = self
                    .tx
                    .send(crate::ui::events::UiEvent::ToolsDone(merged).at(self.id));
                return;
            }
            self.set_status(format!("Running {}…", approved.len()));
            self.spawn_tool_calls(context, approved, results);
            return;
        }
        self.pending_tools = Some(PendingTools {
            calls,
            context,
            approved,
            results,
            queue,
        });
        self.overlay = Overlay::Approval;
        self.show_approval_card();
    }

    fn show_approval_card(&mut self) {
        let Some(pending) = &self.pending_tools else {
            return;
        };
        let Some(current) = pending.queue.front() else {
            return;
        };
        let remaining = pending.queue.len() - 1;
        self.set_status(format!(
            "Approve `{}`{} · y once · a always · n deny",
            current.summary,
            if remaining > 0 {
                format!(" (+{remaining} more)")
            } else {
                String::new()
            }
        ));
    }

    /// Resolve the front card and advance: next card, or ordered spawn.
    pub(crate) fn resolve_approval(&mut self, verdict: ApprovalVerdict) {
        let Some(mut pending) = self.pending_tools.take() else {
            self.overlay = Overlay::None;
            return;
        };
        let Some(item) = pending.queue.pop_front() else {
            self.pending_tools = Some(pending);
            return;
        };
        let call = pending.calls[item.index].clone();
        // The spawned execution re-resolves policy as the floor: record
        // this exact call text in the spawn context so the enforcement
        // sees the approval the card just granted.
        pending.context.policy.allow_session(&item.summary);
        // Per-file grants let later writes to the same path skip cards.
        let (grant_args, _) = call_text(&call);
        let touched = approve::touched_paths(&call.function.name, &grant_args);
        match verdict {
            ApprovalVerdict::Once => {
                pending.approved.push((item.index, call));
                self.set_ok(format!("Approved `{}`", item.summary));
            }
            ApprovalVerdict::Always => {
                self.policy.allow_session(&item.summary);
                pending.context.policy.allow_session(&item.summary);
                for path in &touched {
                    self.policy.allow_session_file(path);
                    pending.context.policy.allow_session_file(path);
                }
                self.set_ok(format!("Always allow `{}`", item.summary));
            }
            ApprovalVerdict::Deny => {
                pending.results[item.index] = Some(denied_result(&call, "denied by user".into()));
                self.set_status(format!("Denied `{}`", item.summary));
            }
        }
        if pending.queue.is_empty() {
            self.overlay = Overlay::None;
            let PendingTools {
                context,
                approved,
                results,
                ..
            } = pending;
            if approved.is_empty() {
                let merged = results
                    .into_iter()
                    .map(|result| {
                        result.unwrap_or_else(|| ToolResult {
                            name: String::new(),
                            call_id: String::new(),
                            content: "tool error: dropped".into(),
                        })
                    })
                    .collect();
                let _ = self
                    .tx
                    .send(crate::ui::events::UiEvent::ToolsDone(merged).at(self.id));
                return;
            }
            self.spawn_tool_calls(context, approved, results);
        } else {
            self.pending_tools = Some(pending);
            self.overlay = Overlay::Approval;
            self.show_approval_card();
        }
    }

    /// Run the approved subset, then merge execution results back into
    /// the original call order (pre-denied and user-denied slots keep
    /// their prepared results).
    fn spawn_tool_calls(
        &mut self,
        context: ToolContext,
        approved: Vec<(usize, ToolCall)>,
        mut results: Vec<Option<ToolResult>>,
    ) {
        let order: Vec<(usize, String)> = approved
            .iter()
            .map(|(index, call)| (*index, call.id.clone()))
            .collect();
        let calls: Vec<ToolCall> = approved.into_iter().map(|(_, call)| call).collect();
        if !results.iter().any(Option::is_none) && calls.is_empty() {
            return;
        }
        self.set_status(format!("Running {}…", calls.len()));
        let sender = self.tx.clone();
        let pane = self.id;
        tokio::spawn(async move {
            match tool::execute_calls(&calls, &context, None).await {
                Ok(done) => {
                    for result in done {
                        if let Some((index, _)) = order.iter().find(|(_, id)| *id == result.call_id)
                        {
                            results[*index] = Some(result);
                        }
                    }
                    let merged = results
                        .into_iter()
                        .map(|result| {
                            result.unwrap_or_else(|| ToolResult {
                                name: String::new(),
                                call_id: String::new(),
                                content: "tool error: dropped".into(),
                            })
                        })
                        .collect();
                    let _ = sender.send(crate::ui::events::UiEvent::ToolsDone(merged).at(pane));
                }
                Err(error) => {
                    let _ = sender.send(
                        crate::ui::events::UiEvent::ChatError(format!("tool execution: {error:#}"))
                            .at(pane),
                    );
                }
            }
        });
    }

    /// Card copy for the overlay renderer; `None` when no card is up.
    pub(crate) fn approval_card(&self) -> Option<(String, String, Option<String>, usize)> {
        let pending = self.pending_tools.as_ref()?;
        let current = pending.queue.front()?;
        Some((
            current.name.clone(),
            current.summary.clone(),
            current.preview.clone(),
            pending.queue.len() - 1,
        ))
    }
}
