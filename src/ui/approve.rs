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

/// The exact text `execute` will match lists against: repaired arguments
/// (fenced JSON counts as the object it becomes), then summarized. Card
/// approvals allow-list this text, so pre-check and enforcement agree.
fn call_text(call: &ToolCall) -> (Value, String) {
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
        match verdict {
            ApprovalVerdict::Once => {
                pending.approved.push((item.index, call));
                self.set_ok(format!("Approved `{}`", item.summary));
            }
            ApprovalVerdict::Always => {
                self.policy.allow_session(&item.summary);
                pending.approved.push((item.index, call));
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
    pub(crate) fn approval_card(&self) -> Option<(String, String, usize)> {
        let pending = self.pending_tools.as_ref()?;
        let current = pending.queue.front()?;
        Some((
            current.name.clone(),
            current.summary.clone(),
            pending.queue.len() - 1,
        ))
    }
}
