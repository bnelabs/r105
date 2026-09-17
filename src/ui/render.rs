//! UiApp render: Ratatui rendering: transcript, palette, composer, overlays.

use super::tabs::{LayoutNode, SplitDirection};
use super::*;

#[derive(Clone, Copy)]
struct PaneFrame {
    index: usize,
    rect: Rect,
}

#[derive(Clone, Copy)]
struct LayoutSeparator {
    rect: Rect,
    direction: SplitDirection,
    first: usize,
    second: usize,
}

fn collect_layout_frames(
    layout: &LayoutNode,
    rect: Rect,
    frames: &mut Vec<PaneFrame>,
    separators: &mut Vec<LayoutSeparator>,
) {
    match layout {
        LayoutNode::Pane(index) => frames.push(PaneFrame {
            index: *index,
            rect,
        }),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let ratio = (*ratio as u32).clamp(100, 900);
            match direction {
                SplitDirection::Right => {
                    if rect.width >= 3 {
                        let usable = rect.width - 1;
                        let first_width = ((usable as u32 * ratio) / 1000)
                            .clamp(1, usable.saturating_sub(1) as u32)
                            as u16;
                        let second_width = usable - first_width;
                        let first_rect = Rect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        };
                        let separator = Rect {
                            x: rect.x + first_width,
                            y: rect.y,
                            width: 1,
                            height: rect.height,
                        };
                        let second_rect = Rect {
                            x: separator.x + 1,
                            y: rect.y,
                            width: second_width,
                            height: rect.height,
                        };
                        let first_index = first_leaf(first);
                        let second_index = first_leaf(second);
                        collect_layout_frames(first, first_rect, frames, separators);
                        collect_layout_frames(second, second_rect, frames, separators);
                        separators.push(LayoutSeparator {
                            rect: separator,
                            direction: *direction,
                            first: first_index,
                            second: second_index,
                        });
                    } else {
                        let first_width = (rect.width / 2).max(1);
                        let first_rect = Rect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        };
                        let second_rect = Rect {
                            x: rect.x + first_width,
                            y: rect.y,
                            width: rect.width.saturating_sub(first_width),
                            height: rect.height,
                        };
                        collect_layout_frames(first, first_rect, frames, separators);
                        collect_layout_frames(second, second_rect, frames, separators);
                    }
                }
                SplitDirection::Down => {
                    if rect.height >= 3 {
                        let usable = rect.height - 1;
                        let first_height = ((usable as u32 * ratio) / 1000)
                            .clamp(1, usable.saturating_sub(1) as u32)
                            as u16;
                        let second_height = usable - first_height;
                        let first_rect = Rect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        };
                        let separator = Rect {
                            x: rect.x,
                            y: rect.y + first_height,
                            width: rect.width,
                            height: 1,
                        };
                        let second_rect = Rect {
                            x: rect.x,
                            y: separator.y + 1,
                            width: rect.width,
                            height: second_height,
                        };
                        let first_index = first_leaf(first);
                        let second_index = first_leaf(second);
                        collect_layout_frames(first, first_rect, frames, separators);
                        collect_layout_frames(second, second_rect, frames, separators);
                        separators.push(LayoutSeparator {
                            rect: separator,
                            direction: *direction,
                            first: first_index,
                            second: second_index,
                        });
                    } else {
                        let first_height = (rect.height / 2).max(1);
                        let first_rect = Rect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        };
                        let second_rect = Rect {
                            x: rect.x,
                            y: rect.y + first_height,
                            width: rect.width,
                            height: rect.height.saturating_sub(first_height),
                        };
                        collect_layout_frames(first, first_rect, frames, separators);
                        collect_layout_frames(second, second_rect, frames, separators);
                    }
                }
            }
        }
    }
}

fn first_leaf(layout: &LayoutNode) -> usize {
    match layout {
        LayoutNode::Pane(index) => *index,
        LayoutNode::Split { first, .. } => first_leaf(first),
    }
}

impl UiApp {
    pub(crate) fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
        self.ensure_active_layout();
        let area = frame.area();
        // A visible pane takes a fixed left column; narrow screens keep
        // the full width for the transcript instead.
        let (side, area) = if self.sidebar_visible && area.width >= 50 {
            let width = sidebar::SIDEBAR_WIDTH.min(area.width * 40 / 100).max(20);
            let columns = ratatui::layout::Layout::default()
                .direction(ratatui::layout::Direction::Horizontal)
                .constraints([Constraint::Length(width), Constraint::Min(20)])
                .split(area);
            (Some(columns[0]), columns[1])
        } else {
            (None, area)
        };
        if let Some(side) = side {
            self.draw_sidebar(frame, side);
        } else {
            self.last_sidebar_rect = Rect::default();
        }
        let rows = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(3)])
            .split(area);
        self.draw_tab_bar(frame, rows[0]);
        let content = rows[1];
        let count = self.panes.len();
        let focused = self.focus;
        let (frames, separators) = if self.pane_zoomed() {
            (
                vec![PaneFrame {
                    index: focused,
                    rect: content,
                }],
                Vec::new(),
            )
        } else {
            let mut frames = Vec::with_capacity(count);
            let mut separators = Vec::new();
            if let Some(layout) = self.active_layout() {
                collect_layout_frames(layout, content, &mut frames, &mut separators);
            }
            (frames, separators)
        };
        for pane in frames {
            self.focus = pane.index;
            self.draw_pane(
                frame,
                pane.rect,
                pane.index,
                pane.index == focused,
                count > 1 || self.pane_zoomed(),
            );
        }
        self.focus = focused;
        self.draw_layout_separators(frame, &separators);
        self.draw_overlay(frame, area);
    }

    fn draw_layout_separators(
        &self,
        frame: &mut ratatui::Frame<'_>,
        separators: &[LayoutSeparator],
    ) {
        let accent = accent_color(&self.state.theme);
        for separator in separators {
            let active = self.focus >= separator.first.min(separator.second)
                && self.focus <= separator.first.max(separator.second);
            let style = if active {
                Style::default().fg(accent)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            match separator.direction {
                SplitDirection::Right => {
                    let lines = std::iter::repeat_n(
                        Line::from(Span::styled("│", style)),
                        separator.rect.height as usize,
                    )
                    .collect::<Vec<_>>();
                    frame.render_widget(Paragraph::new(lines), separator.rect);
                }
                SplitDirection::Down => {
                    let line = Line::from(Span::styled(
                        "─".repeat(separator.rect.width as usize),
                        style,
                    ));
                    frame.render_widget(Paragraph::new(line), separator.rect);
                }
            }
        }
    }

    /// One pane: transcript, completion panels, composer, and footer. Split
    /// panes use a compact header and a tree-owned divider instead of boxing
    /// every surface; the active header/divider carry the focus signal. Rendering
    /// a background pane sets `focus` for the call, so every draw helper
    /// addresses the right pane through `Deref`.
    pub(crate) fn draw_pane(
        &mut self,
        frame: &mut Frame<'_>,
        rect: Rect,
        index: usize,
        focused: bool,
        framed: bool,
    ) {
        let theme = self.state.theme.clone();
        let area = if framed {
            let label = self.current_session.clone().unwrap_or_else(|| {
                if self.title.is_empty() {
                    format!("session {}", index + 1)
                } else {
                    self.title.clone()
                }
            });
            let marker = if self.busy {
                " …"
            } else if self.attention && !focused {
                " •"
            } else {
                ""
            };
            let header_style = if focused {
                Style::default()
                    .fg(accent_color(&theme))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let header = format!(
                "{} {} {}{} ",
                if focused { "›" } else { " " },
                index + 1,
                label,
                marker
            );
            let header_rect = Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: 1.min(rect.height),
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(header, header_style))),
                header_rect,
            );
            Rect {
                x: rect.x,
                y: rect.y.saturating_add(1),
                width: rect.width,
                height: rect.height.saturating_sub(1),
            }
        } else {
            rect
        };
        self.panes[index].rect = rect;
        self.draw_pane_body(frame, area, focused);
    }

    /// The composer-side panels (palette, `@`, args, shell, history)
    /// belong to the focused pane: an unfocused pane must not echo the
    /// shared history search or the focused pane's palette.
    fn draw_pane_body(&mut self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let palette = if focused {
            self.palette_items()
        } else {
            Vec::new()
        };
        let palette_height = if self.palette_active() && !palette.is_empty() {
            palette.len().min(8) as u16 + 2
        } else {
            0
        };
        let file_items = if focused {
            self.at_menu_items()
        } else {
            Vec::new()
        };
        let arg_items = if focused {
            self.arg_menu_items()
        } else {
            Vec::new()
        };
        let sh_items = if focused {
            self.sh_menu_items()
        } else {
            Vec::new()
        };
        let hist_rows = if focused {
            self.hist_rows()
        } else {
            Vec::new()
        };
        // The `@file`, argument-value, shell, and history panels never
        // co-show (each requires its own input shape), so they share
        // one chunk.
        let complete_rows = file_items
            .len()
            .max(arg_items.len())
            .max(sh_items.len())
            .max(hist_rows.len());
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
        // One rule row carries the shell affordance; the input sits
        // directly under it, with no box.
        let composer_height = (composer_lines + 1).clamp(2, 7);
        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(palette_height),
                Constraint::Length(file_height),
                Constraint::Length(composer_height),
                Constraint::Length(2),
            ])
            .split(area);
        self.last_transcript_rect = chunks[0];
        self.last_composer_rect = chunks[3];
        if palette_height > 0 {
            self.last_palette_rect = Some(chunks[1]);
        } else {
            self.last_palette_rect = None;
        }
        self.last_palette_count = palette.len();
        self.draw_transcript(frame, chunks[0]);
        if palette_height > 0 {
            self.draw_palette(frame, chunks[1], &palette);
        }
        if file_height > 0 {
            if self.hist_open() {
                self.last_sh_rect = None;
                self.draw_hist_search(frame, chunks[2], &hist_rows);
            } else {
                self.last_hist_rect = None;
                if !file_items.is_empty() {
                    self.last_sh_rect = None;
                    self.draw_file_complete(frame, chunks[2], &file_items);
                } else if !arg_items.is_empty() {
                    self.last_sh_rect = None;
                    self.draw_arg_complete(frame, chunks[2], &arg_items);
                } else if !sh_items.is_empty() {
                    self.draw_sh_complete(frame, chunks[2], &sh_items);
                } else {
                    self.last_sh_rect = None;
                }
            }
        } else {
            self.last_sh_rect = None;
            self.last_hist_rect = None;
        }
        self.last_sh_count = sh_items.len();
        self.draw_composer(frame, chunks[3]);
        self.draw_footer(frame, chunks[4]);
    }

    pub(crate) fn draw_transcript(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.transcript_height = area.height;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut headers: Vec<Option<String>> = Vec::new();
        let palette = theme_palette(&self.state.theme);
        let show_thinking = self.state.show_thinking;
        let thinking_default = self.state.thinking_default_expanded;
        let details_default = self.show_details;
        // Lazy IDs first: a short &mut pass so the render below (and the
        // section bookkeeping) only needs shared borrows.
        for index in 0..self.state.history.len() {
            self.section_id(index);
        }
        let mut order: Vec<(String, bool)> = Vec::new();
        for (index, message) in self.state.history.iter().enumerate() {
            let block = index + 1;
            if !lines.is_empty() {
                // A blank row separates turns without the old debug-style
                // role rules. The glyph on the first content line carries
                // the role, like modern coding harnesses.
                lines.push(Line::default());
                headers.push(None);
            }
            // Reasoning lives in its own field on new messages; older
            // reasoning-only replies arrive wrapped in the content.
            let thinking = if message.role == "assistant" {
                if !message.reasoning_content.is_empty() {
                    Some(message.reasoning_content.as_str())
                } else {
                    thinking_body(&message.content)
                }
            } else {
                None
            };
            // A wrapped-only message has no visible reply beyond the trace.
            let wrapped_only = message.role == "assistant"
                && message.reasoning_content.is_empty()
                && thinking.is_some();
            let is_tool = message.role == "tool";
            let failed = is_tool && message.content.contains("tool error:");
            // A message is one expandable section only when it carries
            // tool output or visible reasoning. Ordinary user/assistant
            // turns are intentionally not sectioned or numbered on screen.
            let is_section = is_tool || (thinking.is_some() && show_thinking);
            let header_id = if is_section {
                let default = if is_tool {
                    details_default
                } else {
                    thinking_default
                };
                order.push((message.id.clone(), default));
                Some(message.id.clone())
            } else {
                None
            };
            let filtered = self
                .block_filters
                .get(&message.id)
                .map(|filter| apply_block_filter(&message.content, filter));
            headers.push(header_id.clone());

            match message.role.as_str() {
                "user" => {
                    // User prompts are the visual anchor of a turn. Keep
                    // block addressing available to commands internally,
                    // but never expose `USER #N` bookkeeping in the transcript.
                    push_prefixed_filtered(
                        &mut lines,
                        filtered.as_ref(),
                        &message.content,
                        ">",
                        "│",
                        Style::default()
                            .fg(palette.user)
                            .add_modifier(Modifier::BOLD),
                        Style::default(),
                    );
                    if let Some(filtered) = &filtered {
                        push_filter_trailer(&mut lines, filtered.hidden, block);
                    }
                }
                "assistant" => {
                    if let Some(body) = thinking {
                        let expanded = if is_section {
                            let (id, default) = order.last().cloned().unwrap_or_default();
                            self.section_expanded(&id, default)
                        } else {
                            thinking_default
                        };
                        let body = match &filtered {
                            // Wrapped-only traces filter like ordinary
                            // content; separate traces retain their own
                            // reasoning field.
                            Some(filtered) if wrapped_only => filtered.shown.join("\n"),
                            _ => body.to_string(),
                        };
                        push_thinking_lines(&mut lines, &body, show_thinking, expanded);
                        if let Some(filtered) = &filtered
                            && wrapped_only
                        {
                            push_filter_trailer(&mut lines, filtered.hidden, block);
                        }
                    }
                    // Reasoning is never allowed to become the answer. The
                    // visible assistant content is answer-first and gets a
                    // single compact marker rather than a role banner.
                    if !wrapped_only && !message.content.is_empty() {
                        push_prefixed_filtered(
                            &mut lines,
                            filtered.as_ref(),
                            &message.content,
                            "●",
                            "│",
                            Style::default()
                                .fg(palette.assistant)
                                .add_modifier(Modifier::BOLD),
                            Style::default(),
                        );
                        if let Some(filtered) = &filtered {
                            push_filter_trailer(&mut lines, filtered.hidden, block);
                        }
                    } else if message.content.is_empty() && thinking.is_some() && !show_thinking {
                        // A hidden reasoning-only response still needs a
                        // stable visual footprint while it is in history.
                        lines.push(Line::from(Span::styled(
                            "  · thinking hidden",
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                "tool" => {
                    let expanded = failed || self.section_expanded(&message.id, details_default);
                    let (name, body) = tool_payload(&message.content);
                    let output_lines = body.lines().count();
                    let status = if failed { "✗ failed" } else { "✓ done" };
                    let detail = if output_lines == 0 {
                        "no output".to_string()
                    } else if output_lines == 1 {
                        "1 line".to_string()
                    } else {
                        format!("{output_lines} lines")
                    };
                    let marker = format!("  ⏺ {name} · {status} · {detail}");
                    lines.push(Line::from(Span::styled(
                        marker,
                        Style::default()
                            .fg(palette.tool)
                            .add_modifier(Modifier::BOLD),
                    )));
                    if expanded {
                        push_indented_filtered(
                            &mut lines,
                            filtered.as_ref(),
                            &body,
                            "│",
                            Style::default(),
                        );
                        if let Some(filtered) = &filtered {
                            push_filter_trailer(&mut lines, filtered.hidden, block);
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            "    click or /expand to inspect output",
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                _ => {
                    // Notices and system messages are quiet metadata, never
                    // full-width role cards.
                    push_prefixed_filtered(
                        &mut lines,
                        filtered.as_ref(),
                        &message.content,
                        "·",
                        " ",
                        Style::default().fg(Color::DarkGray),
                        Style::default().fg(Color::DarkGray),
                    );
                }
            }
            if !message.tool_calls.is_empty() {
                let names: Vec<String> = message
                    .tool_calls
                    .iter()
                    .map(|c| c.function.name.clone())
                    .collect();
                let count = if message.tool_calls.len() == 1 {
                    "1 call".to_string()
                } else {
                    format!("{} calls", message.tool_calls.len())
                };
                lines.push(Line::from(Span::styled(
                    format!("  ⏺ {count} · {}", tool_names(&names)),
                    Style::default().fg(palette.tool),
                )));
            }
            while headers.len() < lines.len() {
                headers.push(None);
            }
        }
        if !self.state.todos.is_empty() {
            use crate::model::TodoStatus;

            order.push(("todos".to_string(), true));
            let done = self
                .state
                .todos
                .iter()
                .filter(|item| item.status == TodoStatus::Completed)
                .count();
            lines.push(Line::from(Span::styled(
                format!("  ◷ tasks · {done}/{} complete", self.state.todos.len()),
                Style::default()
                    .fg(palette.tool)
                    .add_modifier(Modifier::BOLD),
            )));
            headers.push(Some("todos".to_string()));
            if self.section_expanded("todos", true) {
                for item in &self.state.todos {
                    let marker = match item.status {
                        TodoStatus::Completed => "✓",
                        TodoStatus::InProgress => "▶",
                        TodoStatus::Pending => "·",
                    };
                    lines.push(Line::from(format!("  {marker} {}", item.content)));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!(
                        "    click or /expand to inspect {done}/{} tasks",
                        self.state.todos.len()
                    ),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            while headers.len() < lines.len() {
                headers.push(None);
            }
        }
        self.section_order = order;
        if (!self.streaming_reasoning.is_empty() && self.state.show_thinking
            || !self.streaming.is_empty())
            && !lines.is_empty()
        {
            lines.push(Line::default());
            headers.push(None);
        }
        if !self.streaming_reasoning.is_empty() && self.state.show_thinking {
            lines.push(Line::from(Span::styled(
                "  ◌ thinking…",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )));
            while headers.len() < lines.len() {
                headers.push(None);
            }
        }
        if !self.streaming.is_empty() {
            push_prefixed_text(
                &mut lines,
                &self.streaming,
                "●",
                "│",
                Style::default()
                    .fg(palette.assistant)
                    .add_modifier(Modifier::BOLD),
                Style::default(),
            );
            while headers.len() < lines.len() {
                headers.push(None);
            }
        }
        while headers.len() < lines.len() {
            headers.push(None);
        }
        self.transcript_header_rows = headers;
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
        self.last_transcript_scroll = self.transcript_scroll;
        // No side rails: the conversation owns the pane edge to edge.
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((self.transcript_scroll.min(u16::MAX as usize) as u16, 0)),
            area,
        );
    }

    pub(crate) fn draw_file_complete(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        items: &[String],
    ) {
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
                    .title(" Files · Tab fills "),
            ),
            area,
        );
    }

    pub(crate) fn draw_arg_complete(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        items: &[String],
    ) {
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
                    .title(" Values · Tab fills "),
            ),
            area,
        );
    }

    /// Ctrl+R reverse-search panel: match rows with the highlight, and
    /// the live query in the title.
    pub(crate) fn draw_hist_search(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        rows: &[(String, bool)],
    ) {
        self.last_hist_rect = Some(area);
        self.last_hist_count = rows.len();
        let viewport = area.height.saturating_sub(2) as usize;
        let selected = rows.iter().position(|(_, selected)| *selected).unwrap_or(0);
        let scroll = command::ensure_visible(selected, 0, viewport, rows.len());
        let selection = selection_style(&self.state.theme);
        let query = self
            .hist_search
            .as_ref()
            .map(|search| search.query.clone())
            .unwrap_or_default();
        let title = if query.is_empty() {
            " history · type to search · Enter accepts · Esc cancels ".to_string()
        } else {
            format!(" history · {query} ")
        };
        let lines = rows
            .iter()
            .skip(scroll)
            .take(viewport)
            .map(|(text, selected)| {
                let style = if *selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                Line::from(Span::styled(format!(" {text}"), style))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
    }

    pub(crate) fn draw_sh_complete(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        items: &[crate::suggest::ShellCandidate],
    ) {
        self.last_sh_rect = Some(area);
        let viewport = area.height.saturating_sub(2) as usize;
        let scroll = command::ensure_visible(self.sh_selected, 0, viewport, items.len());
        let selection = selection_style(&self.state.theme);
        let rows = items
            .iter()
            .skip(scroll)
            .take(viewport)
            .enumerate()
            .map(|(offset, item)| {
                let index = scroll + offset;
                let style = if index == self.sh_selected {
                    selection
                } else {
                    Style::default().fg(Color::White)
                };
                let mut spans = vec![Span::styled(format!(" {}", item.text), style)];
                let detail = if item.detail.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", item.detail)
                };
                spans.push(Span::styled(
                    format!(" · {}{detail}", item.kind),
                    Style::default().fg(Color::DarkGray),
                ));
                Line::from(spans)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Complete · Tab fills "),
            ),
            area,
        );
    }

    pub(crate) fn draw_settings(&self, frame: &mut Frame<'_>, area: Rect, selected: usize) {
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
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Settings ")),
            rect,
        );
    }

    pub(crate) fn draw_palette(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        items: &[command::PaletteItem],
    ) {
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
                    .title(" commands · type to filter "),
            ),
            area,
        );
    }

    pub(crate) fn draw_sidebar(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.last_sidebar_rect = area;
        let width = area.width.saturating_sub(2) as usize;
        let viewport = area.height.saturating_sub(2) as usize;
        let rows = self.sidebar_rows().len();
        self.sidebar_selected = self.sidebar_selected.min(rows.saturating_sub(1));
        let (lines, map) = self.sidebar_display(width);
        // Scroll the selected row's line into view (headers shift lines
        // away from row indexes, so scroll in line space).
        let selected_line = map
            .iter()
            .position(|entry| *entry == Some(self.sidebar_selected))
            .unwrap_or(0);
        self.sidebar_scroll =
            command::ensure_visible(selected_line, self.sidebar_scroll, viewport, lines.len());
        let visible: Vec<Line<'_>> = lines
            .into_iter()
            .skip(self.sidebar_scroll)
            .take(viewport)
            .collect();
        let title = if self.sidebar_filter.is_empty() {
            if self.sidebar_focus {
                " sessions · Esc ".to_string()
            } else {
                " sessions ".to_string()
            }
        } else {
            format!(" sessions · /{} ", self.sidebar_filter)
        };
        let border = if self.sidebar_focus {
            Color::White
        } else {
            Color::DarkGray
        };
        frame.render_widget(
            Paragraph::new(visible).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(title),
            ),
            area,
        );
    }

    /// Composer with a visible cell cursor and shell-aware colors.
    /// No box: a dim rule row carries the shell affordance, and the
    /// input sits directly under it.
    pub(crate) fn draw_composer(&self, frame: &mut Frame<'_>, area: Rect) {
        let shell = self.shell_line();
        let affordance = if self.hist_depth.is_some() {
            "History · Esc restores".to_string()
        } else if self.busy {
            format!("Working · {} queued · Esc stops", self.queue.len())
        } else if shell.is_some() {
            "Shell · Enter runs".to_string()
        } else {
            "Message".to_string()
        };
        let width = area.width as usize;
        let label_width = affordance.chars().count() + 1;
        let rule_len = width.saturating_sub(label_width);
        let label_style = if shell.is_some() {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mut rule = vec![Span::styled(
            "─".repeat(rule_len),
            Style::default().fg(Color::DarkGray),
        )];
        if rule_len > 0 {
            rule.push(Span::raw(" "));
        }
        rule.push(Span::styled(affordance, label_style));
        let mut composer: Vec<Span<'_>> = vec![Span::raw("> ")];
        match &shell {
            Some((marker_len, _)) if *marker_len > 0 && self.cursor >= *marker_len => {
                composer.push(Span::styled(
                    self.input[..*marker_len].to_string(),
                    Style::default().fg(Color::Yellow),
                ));
                composer.extend(composer_spans(
                    &self.input[*marker_len..],
                    self.cursor - *marker_len,
                    true,
                ));
            }
            _ => composer.extend(composer_spans(&self.input, self.cursor, shell.is_some())),
        }
        if self.cursor == self.input.len()
            && let Some(ghost) = &self.ghost_text
        {
            composer.push(Span::styled(
                ghost.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if self.input.is_empty() && self.ghost_text.is_none() {
            // Display-only teaching hint: never part of the input.
            let hint = if self.busy {
                "Working — Enter queues · Esc stops"
            } else {
                "Ask anything · shell runs · Tab completes · # describes · / commands"
            };
            composer.push(Span::styled(
                hint.to_string(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(
            Paragraph::new(vec![Line::from(rule), Line::from(composer)]).wrap(Wrap { trim: false }),
            area,
        );
    }

    pub(crate) fn draw_footer(&self, frame: &mut Frame<'_>, area: Rect) {
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
        } else if self.session_cached > 0 {
            format!(
                "↑{} ↓{} ⛁{}",
                compact_number(self.session_in),
                compact_number(self.session_out),
                compact_number(self.session_cached)
            )
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
        let tasks = if self.state.todos.is_empty() {
            String::new()
        } else {
            use crate::model::TodoStatus;

            let done = self
                .state
                .todos
                .iter()
                .filter(|item| item.status == TodoStatus::Completed)
                .count();
            format!("  tasks {done}/{}", self.state.todos.len())
        };
        let position = if self.follow_transcript {
            String::new()
        } else {
            " · End for latest".to_string()
        };
        let first = Line::from(vec![
            Span::styled(
                format!(" {} ", self.status),
                Style::default().fg(self.status_tone.color()),
            ),
            Span::raw("  "),
            Span::styled(session_tokens, Style::default().fg(Color::DarkGray)),
            Span::styled(branch, Style::default().fg(Color::DarkGray)),
            Span::styled(tasks, Style::default().fg(Color::DarkGray)),
            Span::styled(position, Style::default().fg(Color::Yellow)),
        ]);
        let second = Line::from(vec![
            Span::styled(
                format!(" {}", self.state.workspace.display()),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!(" · {}  ", self.sandbox.selected_name()),
                Style::default().fg(Color::DarkGray),
            ),
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
            Span::styled(
                "Ctrl+P palette · Ctrl+R history · Ctrl+B sessions · Ctrl+Shift+T tab · Ctrl+Shift+D/E split",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(vec![first, second]), area);
    }

    pub(crate) fn draw_overlay(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let theme = self.state.theme.clone();
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
                    " Connect ",
                    &items,
                    selected,
                    scroll,
                    selection_style(&theme),
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
                    " Models ",
                    &display,
                    selected,
                    scroll,
                    selection_style(active),
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
                        if *name == theme {
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
                    " Theme ",
                    &items,
                    selected,
                    &mut scroll,
                    selection_style(preview),
                );
            }
            Overlay::Settings { .. } => {}
            Overlay::Approval => {
                let (name, summary, preview, remaining) = self
                    .approval_card()
                    .unwrap_or_else(|| ("tool".into(), String::new(), None, 0));
                let mut lines = vec![
                    Line::from(format!("Approve `{name}`?")),
                    Line::from(""),
                    Line::from(format!("  {summary}")),
                ];
                if let Some(preview) = preview.filter(|text| !text.is_empty()) {
                    for line in preview.lines().take(6) {
                        lines.push(Line::from(Span::styled(
                            format!("  {line}"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "y approve once · a always allow this run · n deny",
                ));
                let mut height = 8 + lines.len().saturating_sub(5).min(6) as u16;
                if remaining > 0 {
                    lines.push(Line::from(format!("+{remaining} more awaiting decision")));
                    height += 1;
                }
                let rect = centered(area, 78, height);
                frame.render_widget(Clear, rect);
                frame.render_widget(
                    Paragraph::new(lines)
                        .block(Block::default().borders(Borders::ALL).title(" Approval ")),
                    rect,
                );
            }
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

/// One classified piece of a shell line for composer colors.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShellPiece {
    Plain,
    Command,
    Flag,
    Str,
    Op,
}

fn piece_style(piece: ShellPiece) -> Style {
    match piece {
        ShellPiece::Plain => Style::default(),
        ShellPiece::Command => Style::default().fg(Color::Cyan),
        ShellPiece::Flag => Style::default().fg(Color::Yellow),
        ShellPiece::Str => Style::default().fg(Color::Green),
        ShellPiece::Op => Style::default().fg(Color::Magenta),
    }
}

/// Composer spans with a visible cell cursor: a reversed block sits on
/// the character under the cursor (or just past the end), and shell
/// lines get shell-aware colors. Concatenating the spans reproduces the
/// input exactly, plus the cursor cell when the cursor is at the end.
fn composer_spans(input: &str, cursor: usize, highlight: bool) -> Vec<Span<'static>> {
    let cursor = cursor.min(input.len());
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut cursor_drawn = false;
    if highlight {
        for (range, piece) in shell_pieces(input) {
            push_piece(&mut spans, input, range, piece, cursor, &mut cursor_drawn);
        }
    } else {
        push_piece(
            &mut spans,
            input,
            0..input.len(),
            ShellPiece::Plain,
            cursor,
            &mut cursor_drawn,
        );
    }
    if !cursor_drawn {
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    spans
}

/// One classified range, splitting at the cursor so the cell under it
/// can carry the reversed style.
fn push_piece(
    spans: &mut Vec<Span<'static>>,
    input: &str,
    range: std::ops::Range<usize>,
    piece: ShellPiece,
    cursor: usize,
    cursor_drawn: &mut bool,
) {
    let style = piece_style(piece);
    let text = &input[range.clone()];
    if !*cursor_drawn
        && input.is_char_boundary(cursor)
        && cursor >= range.start
        && cursor < range.end
    {
        let before = &input[range.start..cursor];
        if !before.is_empty() {
            spans.push(Span::styled(before.to_string(), style));
        }
        let after = &input[cursor..range.end];
        let mut chars = after.chars();
        if let Some(ch) = chars.next() {
            spans.push(Span::styled(
                ch.to_string(),
                style.add_modifier(Modifier::REVERSED),
            ));
            let rest = chars.as_str();
            if !rest.is_empty() {
                spans.push(Span::styled(rest.to_string(), style));
            }
        }
        *cursor_drawn = true;
    } else if !text.is_empty() {
        spans.push(Span::styled(text.to_string(), style));
    }
}

/// Classify a shell line into colored ranges: command words (including
/// behind wrappers) cyan, flags yellow, quoted strings green, operators
/// magenta, everything else plain. Never changes the text.
fn shell_pieces(input: &str) -> Vec<(std::ops::Range<usize>, ShellPiece)> {
    const WRAPPERS: &[&str] = &["sudo", "doas", "env", "nice", "time", "nohup"];
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut piece = ShellPiece::Plain;
    let mut in_word = false;
    let mut expect_command = true;
    let mut last_word = String::new();
    let flush = |pieces: &mut Vec<(std::ops::Range<usize>, ShellPiece)>,
                 start: usize,
                 end: usize,
                 piece: ShellPiece| {
        if end > start {
            pieces.push((start..end, piece));
        }
    };
    let mut chars = input.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\'' | '"' => {
                flush(&mut pieces, start, index, piece);
                start = index;
                piece = ShellPiece::Str;
                for (_, next) in chars.by_ref() {
                    if next == ch {
                        break;
                    }
                }
                in_word = true;
                expect_command = false;
            }
            '|' | '&' | ';' | '>' | '<' => {
                flush(&mut pieces, start, index, piece);
                start = index;
                piece = ShellPiece::Op;
                if chars.peek().is_some_and(|(_, next)| *next == ch) {
                    chars.next();
                }
                in_word = false;
                // A pipe or sequence starts a new command; a redirect is
                // followed by a file name, not a command.
                expect_command = matches!(ch, '|' | '&' | ';');
            }
            c if c.is_whitespace() => {
                flush(&mut pieces, start, index, piece);
                start = index;
                piece = ShellPiece::Plain;
                in_word = false;
            }
            _ => {
                if !in_word {
                    flush(&mut pieces, start, index, piece);
                    start = index;
                    let wrapper = WRAPPERS.contains(&last_word.as_str());
                    if expect_command || wrapper {
                        piece = ShellPiece::Command;
                        expect_command = wrapper;
                    } else if ch == '-' {
                        piece = ShellPiece::Flag;
                    } else {
                        piece = ShellPiece::Plain;
                    }
                    in_word = true;
                    last_word.clear();
                }
                last_word.push(ch);
            }
        }
    }
    flush(&mut pieces, start, input.len(), piece);
    pieces
}

/// Named theme palette: accent drives the header + selection highlight,
/// role colors drive transcript labels. High-contrast maximizes separation.
pub(crate) struct ThemePalette {
    pub(crate) accent: Color,
    pub(crate) user: Color,
    pub(crate) assistant: Color,
    pub(crate) tool: Color,
}

pub(crate) fn theme_palette(theme: &str) -> ThemePalette {
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

pub(crate) fn accent_color(theme: &str) -> Color {
    theme_palette(theme).accent
}

/// Push a conversational body with a stable role glyph on the first line
/// and a quiet continuation rail on following lines. The glyph avoids the
/// old full-width `─ ROLE #N ─` banners while keeping multi-line turns easy
/// to scan.
fn push_prefixed_text(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    prefix_style: Style,
    body_style: Style,
) {
    push_prefixed_body(
        lines,
        text.lines().map(str::to_string),
        first_prefix,
        continuation_prefix,
        prefix_style,
        body_style,
    );
}

fn push_prefixed_body<I>(
    lines: &mut Vec<Line<'static>>,
    body: I,
    first_prefix: &str,
    continuation_prefix: &str,
    prefix_style: Style,
    body_style: Style,
) where
    I: IntoIterator<Item = String>,
{
    let mut body = body.into_iter().peekable();
    if body.peek().is_none() {
        lines.push(Line::from(Span::styled(
            format!("{first_prefix} "),
            prefix_style,
        )));
        return;
    }
    for (index, line) in body.enumerate() {
        let prefix = if index == 0 {
            first_prefix
        } else {
            continuation_prefix
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{prefix} "), prefix_style),
            Span::styled(line, body_style),
        ]));
    }
}

fn push_prefixed_filtered(
    lines: &mut Vec<Line<'static>>,
    filtered: Option<&FilteredLines>,
    raw: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    prefix_style: Style,
    body_style: Style,
) {
    match filtered {
        Some(filtered) => push_prefixed_body(
            lines,
            filtered.shown.iter().cloned(),
            first_prefix,
            continuation_prefix,
            prefix_style,
            body_style,
        ),
        None => push_prefixed_body(
            lines,
            raw.lines().map(str::to_string),
            first_prefix,
            continuation_prefix,
            prefix_style,
            body_style,
        ),
    }
}

fn push_indented_filtered(
    lines: &mut Vec<Line<'static>>,
    filtered: Option<&FilteredLines>,
    raw: &str,
    prefix: &str,
    style: Style,
) {
    match filtered {
        Some(filtered) => push_indented_owned(lines, filtered.shown.iter().cloned(), prefix, style),
        None => push_indented_owned(lines, raw.lines().map(str::to_string), prefix, style),
    }
}

fn push_indented_owned<I>(lines: &mut Vec<Line<'static>>, body: I, prefix: &str, style: Style)
where
    I: IntoIterator<Item = String>,
{
    for line in body {
        lines.push(Line::from(vec![
            Span::styled(
                format!("    {prefix} "),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(line, style),
        ]));
    }
}

/// Split the wire-format tool result (`[name]\noutput`) into a compact
/// label and output body. Older sessions may not contain the bracketed
/// prefix, so those remain readable as an anonymous tool result.
fn tool_payload(content: &str) -> (String, String) {
    if let Some(rest) = content.strip_prefix('[')
        && let Some(end) = rest.find("]\n")
    {
        return (rest[..end].to_string(), rest[end + 2..].to_string());
    }
    ("tool".to_string(), content.to_string())
}

/// Dim trailer under a filtered block: hidden count plus the undo path.
fn push_filter_trailer(lines: &mut Vec<Line<'static>>, hidden: usize, block: usize) {
    lines.push(Line::from(Span::styled(
        format!("  ⋯ {hidden} line(s) hidden · /filter {block} --clear"),
        Style::default().fg(Color::DarkGray),
    )));
}

pub(crate) fn selection_style(theme: &str) -> Style {
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

pub(crate) fn render_picker(
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

pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}
