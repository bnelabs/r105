//! UiApp transcript: Transcript state: notices, status, sections, checkpoints.

use super::*;

impl UiApp {
    pub(crate) fn push_system(&mut self, content: &str) {
        // Fold consecutive duplicates: reconnect loops and repeated
        // fallback warnings collapse into one line with a ×N suffix
        // instead of scrolling the transcript with identical notes.
        if let Some(last) = self.state.history.last_mut()
            && last.role == "system"
        {
            let (base, count) = split_repeat_suffix(&last.content);
            if base == content {
                last.content = format!("{content} (×{})", count + 1);
                self.follow_transcript = true;
                return;
            }
        }
        self.state
            .history
            .push(Message::system(content.to_string()));
        self.follow_transcript = true;
    }

    /// Neutral status note (muted tone).
    pub(crate) fn set_status(&mut self, text: String) {
        self.status = text;
        self.status_tone = StatusTone::Muted;
    }

    /// Completed-action confirmation (green tone).
    pub(crate) fn set_ok(&mut self, text: String) {
        self.status = text;
        self.status_tone = StatusTone::Success;
    }

    /// Switch mode in both mirrors: the Tab-cycle label and the session
    /// state that gates tools and prompts. Splitting them was the 0012 bug.
    pub(crate) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.state.mode = mode.as_str().to_string();
        self.set_ok(format!("Mode: {}", mode.as_str()));
    }

    /// Re-derive the Tab-cycle label after a session load/rewind replaced
    /// the state. Unknown values fall back to build (gates fail open).
    pub(crate) fn sync_mode_from_state(&mut self) {
        self.mode = match self.state.mode.as_str() {
            "plan" => Mode::Plan,
            "ask" => Mode::Ask,
            _ => Mode::Build,
        };
    }

    /// Failure or blocked-action notice (red tone).
    pub(crate) fn set_error(&mut self, text: String) {
        self.status = text;
        self.status_tone = StatusTone::Error;
    }

    /// Stable ID for a history message, assigning `m<N>` lazily so IDs
    /// survive undo/redo/compact (which move messages but never renumber).
    pub(crate) fn section_id(&mut self, index: usize) -> String {
        if self.state.history[index].id.is_empty() {
            let id = format!("m{}", self.next_msg_id);
            self.next_msg_id += 1;
            self.state.history[index].id = id.clone();
            id
        } else {
            self.state.history[index].id.clone()
        }
    }

    /// Effective expanded state: the per-section override wins, otherwise
    /// the global default for this section kind.
    pub(crate) fn section_expanded(&self, id: &str, default: bool) -> bool {
        self.section_state.get(id).copied().unwrap_or(default)
    }

    /// A failed tool result always renders expanded: collapsing it would
    /// hide exactly what the user needs to see.
    pub(crate) fn section_failed(&self, id: &str) -> bool {
        self.state.history.iter().any(|message| {
            message.id == id && message.role == "tool" && message.content.contains("tool error:")
        })
    }

    /// Drop overrides for messages that left the transcript so the map
    /// cannot grow without bound. Overrides are view-local: redoing an
    /// undone exchange renders it with the global defaults again.
    pub(crate) fn prune_sections(&mut self) {
        self.section_state
            .retain(|id, _| self.state.history.iter().any(|message| message.id == *id));
    }

    /// Advance the ID counter past anything already in history (session
    /// load), so fresh messages never collide with restored IDs. An empty
    /// transcript leaves the counter alone: monotonic is enough.
    pub(crate) fn reseed_msg_ids(&mut self) {
        if self
            .state
            .history
            .iter()
            .any(|message| !message.id.is_empty())
        {
            let max = self
                .state
                .history
                .iter()
                .filter_map(|message| message.id.strip_prefix('m')?.parse::<u64>().ok())
                .max()
                .unwrap_or(0);
            self.next_msg_id = self.next_msg_id.max(max + 1);
        }
    }

    /// `/expand [n|all|none]`: flip one transcript section's expanded
    /// state. Bare toggles the most recent section; gutter numbers come
    /// from the last draw (`section_order`), and a draw always precedes
    /// input in the event loop.
    pub(crate) fn command_expand(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            Some("all") | Some("none") => {
                let value = args.first().is_some_and(|action| action == "all");
                let count = self.section_order.len();
                for (id, _) in self.section_order.clone() {
                    // Failed tool results stay expanded whatever is asked.
                    if value || !self.section_failed(&id) {
                        self.section_state.insert(id, value);
                    }
                }
                self.set_ok(format!(
                    "{} {count} section(s)",
                    if value { "Expanded" } else { "Collapsed" }
                ));
            }
            Some(text) => match text.parse::<usize>() {
                Ok(number) if number >= 1 && number <= self.section_order.len() => {
                    let (id, default) = self.section_order[number - 1].clone();
                    if self.section_failed(&id) {
                        self.set_status(format!(
                            "Section {number} is a failed tool result and stays expanded"
                        ));
                        return;
                    }
                    let next = !self.section_expanded(&id, default);
                    self.section_state.insert(id, next);
                    self.set_status(format!(
                        "Section {number} {}",
                        if next { "expanded" } else { "collapsed" }
                    ));
                }
                _ => self.set_error(format!(
                    "Usage: /expand [1-{}|all|none]",
                    self.section_order.len().max(1)
                )),
            },
            None => {
                let last = self.section_order.last().cloned();
                match last {
                    Some((id, default)) => {
                        let number = self.section_order.len();
                        if self.section_failed(&id) {
                            self.set_status(format!(
                                "Section {number} is a failed tool result and stays expanded"
                            ));
                            return;
                        }
                        let next = !self.section_expanded(&id, default);
                        self.section_state.insert(id, next);
                        self.set_status(format!(
                            "Section {number} {}",
                            if next { "expanded" } else { "collapsed" }
                        ));
                    }
                    None => self.set_status("No expandable sections in the transcript".into()),
                }
            }
        }
    }
}

/// Reduce a `/sh` model reply to one runnable line: drop ```` ``` ````
/// fences, skip blanks, strip a leading `$ ` prompt echo. `None` when
/// nothing usable remains.
pub(crate) fn clean_shell_draft(output: &str) -> Option<String> {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("```"))?;
    let line = line.strip_prefix("$ ").unwrap_or(line).trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Reasoning-only replies arrive wrapped by the backend; the wrapper always
/// covers the whole message, so ordinary model text is never misread.
pub(crate) fn thinking_body(content: &str) -> Option<&str> {
    content
        .trim()
        .strip_prefix("<thinking>")
        .and_then(|rest| rest.strip_suffix("</thinking>"))
        .map(str::trim)
}

/// Split history for compaction: summarize `older`, keep `recent`
/// verbatim. The kept tail follows the existing last-third ratio, then
/// walks the split backward past leading tool messages so it never
/// starts mid-exchange with results whose call was summarized away.
pub(crate) fn split_compact(history: &[Message]) -> (&[Message], &[Message]) {
    let keep = (history.len() / 3).max(1);
    let mut split = history.len().saturating_sub(keep);
    while split > 0 && history[split].role == "tool" {
        split -= 1;
    }
    (&history[..split], &history[split..])
}

/// Split a trailing ` (×N)` repeat suffix folded by [`UiApp::push_system`].
/// Returns the base text and the repeat count (1 when no suffix).
pub(crate) fn split_repeat_suffix(content: &str) -> (&str, usize) {
    if let Some(start) = content.rfind(" (×") {
        // " (" is two ASCII bytes and × is two UTF-8 bytes, so both cut
        // points are char boundaries; the guards keep this total.
        if content.ends_with(')')
            && let Ok(count) = content[start + 4..content.len() - 1].parse::<usize>()
        {
            return (&content[..start], count);
        }
    }
    (content, 1)
}

pub(crate) fn push_thinking_lines(lines: &mut Vec<Line>, body: &str, show: bool, expanded: bool) {
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
pub(crate) fn code_blocks(text: &str) -> Vec<String> {
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

pub(crate) fn compact_number(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 1_000_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    }
}

pub(crate) fn slow_start_due(
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
