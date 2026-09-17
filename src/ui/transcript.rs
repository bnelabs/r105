//! UiApp transcript: Transcript state: notices, status, sections, checkpoints.

use super::*;

impl Pane {
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
        self.status_tone = crate::ui::events::StatusTone::Muted;
    }

    /// Completed-action confirmation (green tone).
    pub(crate) fn set_ok(&mut self, text: String) {
        self.status = text;
        self.status_tone = crate::ui::events::StatusTone::Success;
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
        self.status_tone = crate::ui::events::StatusTone::Error;
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

    /// Drop overrides for messages that left the transcript so the maps
    /// cannot grow without bound. Overrides are view-local: redoing an
    /// undone exchange renders it with the global defaults again.
    pub(crate) fn prune_sections(&mut self) {
        self.section_state
            .retain(|id, _| self.state.history.iter().any(|message| message.id == *id));
        self.block_filters
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

    /// `/expand [n|#n|all|none]`: flip one transcript section's expanded
    /// state. Bare toggles the most recent section; `n` is a section
    /// gutter number, `#n` a block number. Gutter numbers come from the
    /// last draw (`section_order`), and a draw always precedes input in
    /// the event loop.
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
            Some(text) => {
                // `#n` addresses a block: resolve through the section
                // table so a tool/thinking message toggles like `[n]`.
                if let Some(rest) = text.strip_prefix('#') {
                    let Ok(number) = rest.parse::<usize>() else {
                        self.set_error(format!(
                            "Usage: /expand [1-{}|#block|all|none]",
                            self.section_order.len().max(1)
                        ));
                        return;
                    };
                    if number < 1 || number > self.state.history.len() {
                        self.set_error(format!(
                            "Block {number} out of range (1-{})",
                            self.state.history.len()
                        ));
                        return;
                    }
                    let id = self.section_id(number - 1);
                    let Some((_, default)) = self
                        .section_order
                        .iter()
                        .find(|(sid, _)| *sid == id)
                        .cloned()
                    else {
                        self.set_status(format!("Block {number} is not a collapsible section"));
                        return;
                    };
                    if let Some(next) =
                        self.toggle_section(&id, default, &format!("Block {number}"))
                    {
                        self.set_status(format!(
                            "Block {number} {}",
                            if next { "expanded" } else { "collapsed" }
                        ));
                    }
                    return;
                }
                match text.parse::<usize>() {
                    Ok(number) if number >= 1 && number <= self.section_order.len() => {
                        let (id, default) = self.section_order[number - 1].clone();
                        if let Some(next) =
                            self.toggle_section(&id, default, &format!("Section {number}"))
                        {
                            self.set_status(format!(
                                "Section {number} {}",
                                if next { "expanded" } else { "collapsed" }
                            ));
                        }
                    }
                    _ => self.set_error(format!(
                        "Usage: /expand [1-{}|#block|all|none]",
                        self.section_order.len().max(1)
                    )),
                }
            }
            None => {
                let last = self.section_order.last().cloned();
                match last {
                    Some((id, default)) => {
                        let number = self.section_order.len();
                        if let Some(next) =
                            self.toggle_section(&id, default, &format!("Section {number}"))
                        {
                            self.set_status(format!(
                                "Section {number} {}",
                                if next { "expanded" } else { "collapsed" }
                            ));
                        }
                    }
                    None => self.set_status("Nothing to expand".into()),
                }
            }
        }
    }

    /// Flip one section by id, honoring the failed-tool guard. Returns
    /// the new expanded state, or `None` when the guard refused.
    pub(crate) fn toggle_section(&mut self, id: &str, default: bool, label: &str) -> Option<bool> {
        if self.section_failed(id) {
            self.set_status(format!("{label} failed and stays expanded"));
            return None;
        }
        let next = !self.section_expanded(id, default);
        self.section_state.insert(id.to_string(), next);
        Some(next)
    }
}

/// Per-block output filter (spec 0017): a stored query that hides
/// non-matching lines when the block renders. Keyed by message id in
/// `UiApp.block_filters`, pruned with the section overrides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockFilter {
    pub pattern: String,
    pub regex: bool,
    pub case: bool,
    pub invert: bool,
    pub context: usize,
}

impl BlockFilter {
    /// Human-readable flag summary (`"needle" · regex · case · ctx 2`).
    pub(crate) fn describe(&self) -> String {
        let mut parts = vec![format!("\"{}\"", self.pattern)];
        if self.regex {
            parts.push("regex".into());
        }
        if self.case {
            parts.push("case".into());
        }
        if self.invert {
            parts.push("invert".into());
        }
        if self.context > 0 {
            parts.push(format!("ctx {}", self.context));
        }
        parts.join(" · ")
    }
}

/// Parsed `/filter` intent: block numbers are 1-based history positions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterAction {
    Set(usize, BlockFilter),
    Clear(usize),
    List,
}

pub(crate) const FILTER_USAGE: &str =
    "Usage: /filter <block> <pattern> [--regex] [--case] [--invert] [--context N]";

/// Block addresses accept the rendered `#n` form or a bare number, so
/// what `/block` prints can be pasted into `/filter` and `/rerun`.
pub(crate) fn parse_block_number(text: &str) -> Option<usize> {
    text.strip_prefix('#').unwrap_or(text).parse().ok()
}

/// Parse `/filter` args against the history length. Returns the 0-based
/// message index plus the action. A bare block number clears its filter.
pub(crate) fn parse_block_filter(
    history_len: usize,
    args: &[String],
) -> Result<FilterAction, String> {
    let Some(first) = args.first() else {
        return Ok(FilterAction::List);
    };
    let number: usize =
        parse_block_number(first).ok_or_else(|| format!("{FILTER_USAGE} (got {first})"))?;
    if number < 1 || number > history_len.max(1) || history_len == 0 {
        return Err(format!("Block {number} out of range (1-{history_len})"));
    }
    let index = number - 1;
    let mut filter = BlockFilter {
        pattern: String::new(),
        regex: false,
        case: false,
        invert: false,
        context: 0,
    };
    let mut pattern: Vec<String> = Vec::new();
    let mut rest = &args[1..];
    let mut clear = false;
    while let Some((flag, tail)) = rest.split_first() {
        match flag.as_str() {
            "--regex" => filter.regex = true,
            "--case" => filter.case = true,
            "--invert" => filter.invert = true,
            "--clear" => clear = true,
            "--context" => {
                let value = tail
                    .first()
                    .and_then(|text| text.parse::<usize>().ok())
                    .ok_or_else(|| format!("{FILTER_USAGE} (--context needs a number)"))?;
                filter.context = value;
                rest = &tail[1.min(tail.len())..];
                continue;
            }
            other if other.starts_with("--") => {
                return Err(format!("{FILTER_USAGE} (unknown flag {other})"));
            }
            _ => pattern.push(flag.clone()),
        }
        rest = tail;
    }
    if clear || pattern.is_empty() {
        return Ok(FilterAction::Clear(index));
    }
    filter.pattern = pattern.join(" ");
    if filter.regex
        && let Err(error) = regex::Regex::new(&filter.pattern)
    {
        return Err(format!("Bad regex {}: {error}", filter.pattern));
    }
    Ok(FilterAction::Set(index, filter))
}

/// Filtered body: shown lines plus how many were hidden. Context keeps
/// lines near a hit; invert keeps lines away from one.
pub(crate) struct FilteredLines {
    pub shown: Vec<String>,
    pub hidden: usize,
}

pub(crate) fn apply_block_filter(content: &str, filter: &BlockFilter) -> FilteredLines {
    let lines: Vec<&str> = content.lines().collect();
    let matcher = regex::Regex::new(&filter.pattern).ok();
    let mut hits = vec![false; lines.len()];
    for (index, line) in lines.iter().enumerate() {
        let matched = if filter.regex {
            matcher
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(line))
        } else if filter.case {
            line.contains(filter.pattern.as_str())
        } else {
            line.to_ascii_lowercase()
                .contains(&filter.pattern.to_ascii_lowercase())
        };
        hits[index] = if filter.invert { !matched } else { matched };
    }
    let mut shown = Vec::new();
    let mut hidden = 0;
    for (index, line) in lines.iter().enumerate() {
        let near = ((index.saturating_sub(filter.context))..=(index + filter.context))
            .any(|near| hits.get(near).copied().unwrap_or(false));
        if near {
            shown.push((*line).to_string());
        } else {
            hidden += 1;
        }
    }
    FilteredLines { shown, hidden }
}

/// Clipboard payload for `/copy out`: the filtered view when the block
/// carries a filter, otherwise the verbatim content.
pub(crate) fn block_copy_content(content: &str, filter: Option<&BlockFilter>) -> String {
    match filter {
        Some(filter) => apply_block_filter(content, filter).shown.join("\n"),
        None => content.to_string(),
    }
}

/// Resolve a `/rerun` target to the prompt text to resubmit: an explicit/// block number must hold a user message, otherwise the most recent user
/// message wins. Pure so the async resubmit stays thin.
pub(crate) fn rerun_target(history: &[Message], arg: Option<usize>) -> Result<String, String> {
    match arg {
        Some(number) => {
            if number < 1 || number > history.len() {
                return Err(format!("Block {number} out of range (1-{})", history.len()));
            }
            let message = &history[number - 1];
            if message.role != "user" {
                return Err(format!(
                    "Block {number} is {} output, not a prompt — rerun needs a user block",
                    message.role.to_ascii_uppercase()
                ));
            }
            Ok(message.content.clone())
        }
        None => history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|message| message.content.clone())
            .ok_or_else(|| "No user prompt yet".to_string()),
    }
}

/// Reduce a `#` model reply to one runnable line: drop ```` ``` ````
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
        return;
    }
    let body_lines: Vec<&str> = body.lines().collect();
    if expanded {
        if body_lines.is_empty() {
            lines.push(Line::from(Span::styled("  ┊ thinking (empty)", dim)));
        } else {
            lines.push(Line::from(Span::styled("  ┊ thinking", dim)));
            for line in &body_lines {
                lines.push(Line::from(Span::styled(format!("  ┊ {line}"), dim)));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            format!(
                "  ◌ thought · {} line{} · click or /expand to inspect",
                body_lines.len(),
                if body_lines.len() == 1 { "" } else { "s" }
            ),
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
