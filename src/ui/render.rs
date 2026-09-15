//! UiApp render: Ratatui rendering: transcript, palette, composer, overlays.

use super::*;

impl UiApp {
    pub(crate) fn draw(&mut self, frame: &mut ratatui::Frame<'_>) {
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

    pub(crate) fn draw_header(&self, frame: &mut Frame<'_>, area: Rect) {
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

    pub(crate) fn draw_transcript(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let mut lines = Vec::new();
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
        for message in &self.state.history {
            let color = match message.role.as_str() {
                "user" => palette.user,
                "assistant" => palette.assistant,
                "tool" => palette.tool,
                _ => Color::Magenta,
            };
            let label = message.role.to_ascii_uppercase();
            let thinking = if message.role == "assistant" {
                thinking_body(&message.content)
            } else {
                None
            };
            let is_tool = message.role == "tool";
            let failed = is_tool && message.content.contains("tool error:");
            // A message is one section: tool output, or the thinking part
            // of an assistant message (shown only when thinking is on).
            let is_section = is_tool || (thinking.is_some() && show_thinking);
            let gutter = if is_section {
                let default = if is_tool {
                    details_default
                } else {
                    thinking_default
                };
                order.push((message.id.clone(), default));
                format!(" [{}]", order.len())
            } else {
                String::new()
            };
            lines.push(Line::from(Span::styled(
                format!(" {label}{gutter} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            if message.role == "assistant"
                && let Some(body) = thinking
            {
                let expanded = if is_section {
                    let (id, default) = order.last().cloned().unwrap_or_default();
                    self.section_expanded(&id, default)
                } else {
                    thinking_default
                };
                push_thinking_lines(&mut lines, body, show_thinking, expanded);
            } else if is_tool {
                let expanded = failed || self.section_expanded(&message.id, details_default);
                if !expanded {
                    let first = message.content.lines().next().unwrap_or_default();
                    let number = order.len();
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  ▸[{number}] {}…",
                            first.chars().take(96).collect::<String>()
                        ),
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    for line in message.content.lines() {
                        lines.push(Line::from(format!("  {line}")));
                    }
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
        self.section_order = order;
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
                    .title(" files · Tab accept · ↑↓ choose "),
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
                    .title(" values · Tab accept · ↑↓ choose "),
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
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" settings · ↑↓ move · ←/→ change · Esc close "),
            ),
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
                    .title(" commands · ↑↓ choose · Enter accept · * custom "),
            ),
            area,
        );
    }

    pub(crate) fn draw_composer(&self, frame: &mut Frame<'_>, area: Rect) {
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
                Style::default().fg(self.status_tone.color()),
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

    pub(crate) fn draw_overlay(&mut self, frame: &mut Frame<'_>, area: Rect) {
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
