use super::*;

pub(super) fn pointer_cell(state: &WindowState) -> (u16, u16) {
    let (rows, cols) = state.session.size();
    (
        (((state.pointer.1 - PAD_Y).max(0.0) / CELL_H) as u16).min(rows - 1),
        (((state.pointer.0 - PAD_X).max(0.0) / CELL_W) as u16).min(cols - 1),
    )
}

fn ordered(selection: ((u16, u16), (u16, u16))) -> ((u16, u16), (u16, u16)) {
    if selection.0 <= selection.1 {
        selection
    } else {
        (selection.1, selection.0)
    }
}

fn selected_text(screen: &vt100::Screen, selection: ((u16, u16), (u16, u16))) -> String {
    let (start, end) = ordered(selection);
    let mut lines = Vec::new();
    for row in start.0..=end.0 {
        let from = if row == start.0 { start.1 } else { 0 };
        let to = if row == end.0 {
            end.1
        } else {
            screen.size().1 - 1
        };
        let mut line = String::new();
        for col in from..=to {
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                let content = cell.contents();
                line.push_str(if content.is_empty() { " " } else { &content });
            }
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

pub(super) fn selection_rects(state: &WindowState) -> Vec<ColoredRect> {
    let Some(selection) = state.selection else {
        return Vec::new();
    };
    let (start, end) = ordered(selection);
    let cols = state.session.size().1;
    (start.0..=end.0)
        .map(|row| {
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 { end.1 + 1 } else { cols };
            (
                BoxRect::new(
                    PAD_X + f32::from(from) * CELL_W,
                    PAD_Y + f32::from(row) * CELL_H,
                    f32::from(to - from) * CELL_W,
                    CELL_H,
                ),
                px((40, 65, 95)),
            )
        })
        .collect()
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let clean: String = normalized
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    if bracketed {
        format!("\x1b[200~{clean}\x1b[201~").into_bytes()
    } else {
        clean.replace('\n', "\r").into_bytes()
    }
}

pub(super) fn clipboard_action(state: &mut WindowState, paste: bool) {
    if state.approval.is_some() {
        state.status_note = "Resolve the approval before pasting or copying".into();
        return;
    }
    let outcome = (|| -> Result<()> {
        if state.clipboard.is_none() {
            state.clipboard = Some(arboard::Clipboard::new()?);
        }
        if paste {
            let text = state.clipboard.as_mut().unwrap().get_text()?;
            anyhow::ensure!(text.len() <= 1024 * 1024, "clipboard text exceeds 1 MiB");
            if state.focus == Focus::Composer {
                state.composer.insert_text(&text);
            } else if state.focus == Focus::Terminal {
                let bytes = paste_bytes(&text, state.session.screen().bracketed_paste());
                state.session.scroll_to_bottom();
                state.selection = None;
                state.session.write(&bytes)?;
            }
        } else {
            let text = if state.focus == Focus::Composer {
                state.composer.text().to_string()
            } else if state.focus == Focus::AiPanel {
                state
                    .ai
                    .list()
                    .get(state.panel_sel)
                    .map(|block| block.response.clone())
                    .unwrap_or_default()
            } else {
                state
                    .selection
                    .map(|selection| selected_text(state.session.screen(), selection))
                    .unwrap_or_default()
            };
            if !text.is_empty() {
                state.clipboard.as_mut().unwrap().set_text(text)?;
            }
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        state.status_note = format!("Clipboard: {error}");
    }
    state.window.request_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_normalizes_newlines_and_cannot_inject_bracket_terminators() {
        assert_eq!(
            paste_bytes("one\r\ntwo\x1b[201~", true),
            b"\x1b[200~one\ntwo[201~\x1b[201~"
        );
        assert_eq!(paste_bytes("one\ntwo", false), b"one\rtwo");
    }
    #[test]
    fn selection_copies_wide_unicode_and_reversed_ranges() {
        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process("ab界cd\r\nnext".as_bytes());
        assert_eq!(selected_text(parser.screen(), ((0, 1), (0, 5))), "b界cd");
        assert_eq!(
            selected_text(parser.screen(), ((1, 3), (0, 0))),
            "ab界cd\nnext"
        );
    }
}
