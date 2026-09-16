//! UiApp sidebar: a toggleable left pane for opening and managing
//! sessions (one per working transcript) and recent workspaces.
//! Sessions are file-backed and switching never discards work: the
//! live transcript autosaves first, to its own name or a timestamped
//! `autosave-<epoch>` file.

use super::*;

pub(crate) const SIDEBAR_WIDTH: u16 = 30;
const MAX_RECENT_WORKSPACES: usize = 8;

/// One selectable sidebar row. Section headers render between groups
/// but are never rows, so keyboard and click math stay index-clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarRow {
    New,
    Session { name: String, messages: usize },
    Workspace { path: String },
}

impl UiApp {
    /// Show the pane (focused) if hidden, hide it if focused, focus it
    /// if visible-but-idle. Ctrl+B cycles all three.
    pub(crate) fn toggle_sidebar(&mut self) {
        if !self.sidebar_visible {
            self.sidebar_visible = true;
            self.sidebar_focus = true;
            self.sidebar_selected = 0;
            self.refresh_sidebar();
            self.set_status(
                "Sessions · Enter opens · n new · d delete · type filters · Esc".into(),
            );
        } else if self.sidebar_focus {
            self.sidebar_visible = false;
            self.sidebar_focus = false;
            self.sidebar_filter.clear();
        } else {
            self.sidebar_focus = true;
            self.sidebar_selected = 0;
        }
    }

    /// Leave keyboard focus in the composer; the pane stays visible.
    pub(crate) fn unfocus_sidebar(&mut self) {
        self.sidebar_focus = false;
        self.sidebar_filter.clear();
    }

    /// Reload the saved-session list; the selection clamps on next use.
    pub(crate) fn refresh_sidebar(&mut self) {
        self.sidebar_sessions = session::list(&self.paths);
        self.clamp_sidebar_selected();
    }

    pub(crate) fn clamp_sidebar_selected(&mut self) {
        let rows = self.sidebar_rows().len();
        self.sidebar_selected = self.sidebar_selected.min(rows.saturating_sub(1));
    }

    /// Selectable rows: `+ New` (only unfiltered), saved sessions in
    /// list order, then workspaces with the live one pinned first. The
    /// live pin is display-only, so startup never writes the recents
    /// file — only explicit switches persist.
    pub(crate) fn sidebar_rows(&self) -> Vec<SidebarRow> {
        let mut rows = Vec::new();
        if self.sidebar_filter.is_empty() {
            rows.push(SidebarRow::New);
        }
        let query = self.sidebar_filter.to_ascii_lowercase();
        for info in &self.sidebar_sessions {
            if !query.is_empty() && !info.name.to_ascii_lowercase().contains(&query) {
                continue;
            }
            rows.push(SidebarRow::Session {
                name: info.name.clone(),
                messages: info.message_count,
            });
        }
        let live = self.state.workspace.to_string_lossy().to_string();
        for path in std::iter::once(&live)
            .chain(self.recent_workspaces.iter().filter(|item| *item != &live))
        {
            if !query.is_empty() {
                let lowered = path.to_ascii_lowercase();
                let base = workspace_basename(path).to_ascii_lowercase();
                if !lowered.contains(&query) && !base.contains(&query) {
                    continue;
                }
            }
            rows.push(SidebarRow::Workspace { path: path.clone() });
        }
        rows
    }

    /// Render-ready lines plus, per line, the row index when the line
    /// is selectable (`None` for section headers and empty states).
    /// Pure: click handling recomputes the same mapping, so draw and
    /// hit-testing can never disagree.
    pub(crate) fn sidebar_display(&self, width: usize) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
        let rows = self.sidebar_rows();
        let split = rows
            .iter()
            .position(|row| matches!(row, SidebarRow::Workspace { .. }))
            .unwrap_or(rows.len());
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut map: Vec<Option<usize>> = Vec::new();
        if split > 0 {
            lines.push(section_line(" SESSIONS "));
            map.push(None);
            for (index, row) in rows[..split].iter().enumerate() {
                lines.push(self.sidebar_row_line(row, index, width));
                map.push(Some(index));
            }
        }
        if split < rows.len() {
            lines.push(section_line(" WORKSPACES "));
            map.push(None);
            for (offset, row) in rows[split..].iter().enumerate() {
                lines.push(self.sidebar_row_line(row, split + offset, width));
                map.push(Some(split + offset));
            }
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                " (no match)",
                Style::default().fg(Color::DarkGray),
            )));
            map.push(None);
        }
        (lines, map)
    }

    fn sidebar_row_line(&self, row: &SidebarRow, index: usize, width: usize) -> Line<'static> {
        let selected = index == self.sidebar_selected;
        let style = if selected && self.sidebar_focus {
            super::render::selection_style(&self.state.theme)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if selected { ">" } else { " " };
        let text = match row {
            SidebarRow::New => "+ New session".to_string(),
            SidebarRow::Session { name, messages } => {
                let dot = if self.current_session.as_deref() == Some(name.as_str()) {
                    "●"
                } else {
                    " "
                };
                format!("{dot} {name} · {messages}")
            }
            SidebarRow::Workspace { path } => {
                let dot = if same_path(path, &self.state.workspace) {
                    "●"
                } else {
                    " "
                };
                format!("{dot} {}", workspace_basename(path))
            }
        };
        Line::from(Span::styled(
            truncate_cells(&format!("{marker} {text}"), width),
            style,
        ))
    }

    /// Keyboard entry while the pane owns input. Plain letters filter
    /// (`n` opens a fresh session only when the filter is empty);
    /// Enter opens, Delete/`d` removes a saved session.
    pub(crate) fn handle_sidebar_key(&mut self, key: KeyEvent) {
        let rows = self.sidebar_rows();
        match key.code {
            KeyCode::Up => self.sidebar_selected = self.sidebar_selected.saturating_sub(1),
            KeyCode::Down => {
                self.sidebar_selected =
                    (self.sidebar_selected + 1).min(rows.len().saturating_sub(1));
            }
            KeyCode::Home => self.sidebar_selected = 0,
            KeyCode::End => self.sidebar_selected = rows.len().saturating_sub(1),
            KeyCode::PageUp => {
                self.sidebar_selected = self.sidebar_selected.saturating_sub(sidebar_page(self));
            }
            KeyCode::PageDown => {
                self.sidebar_selected =
                    (self.sidebar_selected + sidebar_page(self)).min(rows.len().saturating_sub(1));
            }
            KeyCode::Enter => self.sidebar_open_selected(),
            KeyCode::Delete => self.sidebar_delete_selected(),
            KeyCode::Backspace => {
                self.sidebar_filter.pop();
                self.sidebar_selected = 0;
            }
            KeyCode::Char('n' | 'N')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.sidebar_filter.is_empty() =>
            {
                self.sidebar_new();
            }
            KeyCode::Char('d' | 'D')
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                    && self.sidebar_filter.is_empty() =>
            {
                self.sidebar_delete_selected();
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.sidebar_filter.push(character);
                self.sidebar_selected = 0;
            }
            _ => {}
        }
        self.clamp_sidebar_selected();
    }

    /// Open the selected row: fresh transcript, saved session, or
    /// workspace switch.
    pub(crate) fn sidebar_open_selected(&mut self) {
        let rows = self.sidebar_rows();
        match rows.get(self.sidebar_selected) {
            Some(SidebarRow::New) => self.sidebar_new(),
            Some(SidebarRow::Session { name, .. }) => {
                let name = name.clone();
                self.sidebar_load(&name);
            }
            Some(SidebarRow::Workspace { path }) => {
                let path = path.clone();
                self.command_workspace(&[path]);
            }
            None => {}
        }
    }

    /// Fresh transcript, autosaving the live one first so starting
    /// over never loses work.
    pub(crate) fn sidebar_new(&mut self) {
        let saved = self.autosave_current();
        self.state.history.clear();
        self.redo_stack.clear();
        self.reseed_msg_ids();
        self.prune_sections();
        self.current_session = None;
        self.follow_transcript = true;
        self.refresh_sidebar();
        self.set_status(match saved {
            Some(name) => format!("New session · previous saved as {name}"),
            None => "New session".into(),
        });
    }

    /// Load a saved session, autosaving the live transcript first
    /// (unless it is the same session being reloaded).
    pub(crate) fn sidebar_load(&mut self, name: &str) {
        if self.current_session.as_deref() != Some(name) {
            self.autosave_current();
        }
        self.load_session_named(name);
        self.refresh_sidebar();
    }

    /// Delete the selected saved session file. The live transcript is
    /// never touched: deleting the loaded session just unnams it.
    pub(crate) fn sidebar_delete_selected(&mut self) {
        let rows = self.sidebar_rows();
        let Some(SidebarRow::Session { name, .. }) = rows.get(self.sidebar_selected) else {
            self.set_status("Sessions · d deletes a saved session".into());
            return;
        };
        let name = name.clone();
        match session::delete(&self.paths, &name) {
            Ok(true) => {
                if self.current_session.as_deref() == Some(name.as_str()) {
                    self.current_session = None;
                }
                self.refresh_sidebar();
                self.set_status(format!("Deleted session {name}"));
            }
            Ok(false) => self.set_error(format!("Session not found: {name}")),
            Err(error) => self.set_error(format!("Session delete failed: {error}")),
        }
    }

    /// Persist the live transcript when it holds anything: back to its
    /// own name, else a timestamped autosave. Returns the name used.
    pub(crate) fn autosave_current(&mut self) -> Option<String> {
        let paths = self.paths.clone();
        self.autosave(&paths)
    }

    /// Whether a click lands inside the visible pane.
    pub(crate) fn sidebar_hit(&self, column: u16, row: u16) -> bool {
        if !self.sidebar_visible || self.last_sidebar_rect == Rect::default() {
            return false;
        }
        let area = self.last_sidebar_rect;
        column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height
    }

    /// Click a sidebar row: first click selects, second opens — the
    /// same rhythm as the palette.
    pub(crate) fn sidebar_click(&mut self, column: u16, row: u16) {
        if !self.sidebar_visible || self.last_sidebar_rect == Rect::default() {
            return;
        }
        let area = self.last_sidebar_rect;
        if column < area.x
            || column >= area.x + area.width
            || row < area.y + 1
            || row >= area.y + area.height.saturating_sub(1)
        {
            return;
        }
        let width = area.width.saturating_sub(2) as usize;
        let (_, map) = self.sidebar_display(width);
        let line = (row - area.y - 1) as usize + self.sidebar_scroll;
        if let Some(Some(index)) = map.get(line) {
            if *index == self.sidebar_selected {
                self.sidebar_focus = true;
                self.sidebar_open_selected();
            } else {
                self.sidebar_selected = *index;
                self.sidebar_focus = true;
            }
        }
    }

    fn recent_path(&self) -> std::path::PathBuf {
        self.paths.config_dir.join("recent_workspaces.json")
    }

    /// Load persisted workspaces (existing directories only). Read-only:
    /// the live workspace pins itself at display time, so this never
    /// writes — only explicit switches persist.
    pub(crate) fn load_recent_workspaces(&mut self) {
        let mut recent: Vec<String> = std::fs::read_to_string(self.recent_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        recent.retain(|path| std::path::Path::new(path).is_dir());
        recent.truncate(MAX_RECENT_WORKSPACES);
        self.recent_workspaces = recent;
    }

    /// Pin a workspace to the front of the recents; persists silently.
    pub(crate) fn note_workspace(&mut self, path: &std::path::Path) {
        self.note_workspace_str(&path.to_string_lossy());
    }

    fn note_workspace_str(&mut self, path: &str) {
        self.recent_workspaces.retain(|item| item != path);
        self.recent_workspaces.insert(0, path.to_string());
        self.recent_workspaces.truncate(MAX_RECENT_WORKSPACES);
        let path = self.recent_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &path,
            serde_json::to_string(&self.recent_workspaces).unwrap_or_default(),
        );
    }
}

fn section_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        title.to_string(),
        Style::default().fg(Color::DarkGray),
    ))
}

/// Last path component, `~`-aware for display only.
pub(crate) fn workspace_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string())
}

/// String-vs-PathBuf equality without allocating the live path.
fn same_path(left: &str, right: &std::path::Path) -> bool {
    std::path::Path::new(left) == right
}

/// Visible content lines for paging: full height minus the border.
fn sidebar_page(app: &UiApp) -> usize {
    (app.last_sidebar_rect.height.saturating_sub(2) as usize).max(1)
}

/// Truncate to a cell width without splitting mid-character.
fn truncate_cells(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    text.chars().take(width).collect()
}
