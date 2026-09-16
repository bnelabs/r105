//! UiApp tabs: session-backed tabs for the top bar.
//!
//! Each tab names a saved session. Switching autosaves the live session
//! first, so a tab never drops work; a fresh tab starts an unnamed
//! session that becomes the tab's name on its first save. The bar
//! persists to `tabs.json` and restores on startup, loading the active
//! tab's session so the visible state always matches the bar.

use super::render::accent_color;
use super::*;

/// One tab: a session file plus the label shown in the bar. The active
/// tab's label tracks `current_session` live; inactive tabs keep the
/// name they had when the user left them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Tab {
    #[serde(default)]
    pub(crate) session: Option<String>,
    pub(crate) title: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TabsFile {
    tabs: Vec<Tab>,
    #[serde(default)]
    active: usize,
}

impl UiApp {
    fn tabs_path(&self) -> PathBuf {
        self.paths.config_dir.join("tabs.json")
    }

    /// Restore the persisted bar and load its active session. Missing or
    /// unreadable files leave the single default tab in place.
    pub(crate) fn load_tabs(&mut self) {
        let Ok(raw) = std::fs::read_to_string(self.tabs_path()) else {
            return;
        };
        let Ok(file) = serde_json::from_str::<TabsFile>(&raw) else {
            return;
        };
        if file.tabs.is_empty() {
            return;
        }
        self.tabs = file.tabs;
        self.active_tab = file.active.min(self.tabs.len() - 1);
        if let Some(name) = self.tabs[self.active_tab].session.clone() {
            self.load_session_named(&name);
        }
    }

    fn save_tabs(&self) {
        let file = TabsFile {
            tabs: self.tabs.clone(),
            active: self.active_tab,
        };
        if let Ok(raw) = serde_json::to_string(&file) {
            let _ = std::fs::write(self.tabs_path(), raw);
        }
    }

    /// Store the live session name on the active tab before leaving it.
    fn stash_active_tab(&mut self) {
        let Some(name) = self.current_session.clone() else {
            return;
        };
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.title = name.clone();
            tab.session = Some(name);
        }
    }

    /// Replace the visible session with a fresh unnamed one.
    fn reset_session(&mut self) {
        self.state.history.clear();
        self.redo_stack.clear();
        self.reseed_msg_ids();
        self.prune_sections();
        self.current_session = None;
        self.follow_transcript = true;
    }

    pub(crate) fn tab_new(&mut self) {
        if self.busy {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        self.autosave_current();
        self.stash_active_tab();
        self.reset_session();
        self.tabs.push(Tab {
            session: None,
            title: "new".into(),
        });
        self.active_tab = self.tabs.len() - 1;
        self.refresh_sidebar();
        self.save_tabs();
        self.set_status(format!("Tab {} · new session", self.active_tab + 1));
    }

    pub(crate) fn tab_switch(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        if self.busy {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        self.autosave_current();
        self.stash_active_tab();
        self.active_tab = index;
        self.activate_tab();
        self.save_tabs();
    }

    pub(crate) fn tab_next(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        let index = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.tab_switch(index);
    }

    pub(crate) fn tab_close(&mut self) {
        if self.tabs.len() <= 1 {
            self.set_status("Last tab stays open".into());
            return;
        }
        if self.busy {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        self.autosave_current();
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.activate_tab();
        self.save_tabs();
        self.set_status(format!("Tab {}", self.active_tab + 1));
    }

    /// Load whatever the active tab points at (or start fresh).
    fn activate_tab(&mut self) {
        let target = self.tabs.get(self.active_tab).cloned();
        let Some(tab) = target else {
            return;
        };
        match tab.session {
            Some(name) => {
                if self.current_session.as_deref() != Some(name.as_str()) {
                    self.load_session_named(&name);
                }
            }
            None => self.reset_session(),
        }
        self.refresh_sidebar();
    }

    /// The tab-bar row: `r105` brand, one chip per tab (`N label`, the
    /// active chip filled), a `+` affordance, then the right-aligned
    /// mode · provider · model. Clicks resolve against `last_tab_rect`.
    pub(crate) fn draw_tab_bar(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.last_tab_rect = area;
        let connection = self.backend.connection();
        let accent = accent_color(&self.state.theme);
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            " r105 ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )];
        let mut used = 6usize;
        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active_tab;
            let label = if active {
                self.current_session
                    .clone()
                    .unwrap_or_else(|| tab.title.clone())
            } else {
                tab.title.clone()
            };
            let text = format!(" {} {} ", index + 1, label);
            used += text.chars().count();
            let style = if active {
                Style::default()
                    .fg(Color::Black)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            spans.push(Span::styled(text, style));
            spans.push(Span::raw(" "));
            used += 1;
        }
        spans.push(Span::styled("+ ", Style::default().fg(Color::DarkGray)));
        used += 2;
        let right = format!(
            "{} · {} · {} ",
            self.mode.as_str(),
            connection.display_name(),
            self.state.model
        );
        let right_width = right.chars().count();
        let width = area.width as usize;
        if used + right_width < width {
            spans.push(Span::raw(" ".repeat(width - used - right_width)));
        }
        spans.push(Span::styled(right, Style::default().fg(Color::DarkGray)));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Whether a click lands on the `+` affordance after the chips.
    pub(crate) fn tab_plus_hit(&self, column: u16, row: u16) -> bool {
        let area = self.last_tab_rect;
        if area.width == 0 || row != area.y || column < area.x || column >= area.x + area.width {
            return false;
        }
        let mut offset = 6usize;
        for (index, tab) in self.tabs.iter().enumerate() {
            let label = if index == self.active_tab {
                self.current_session
                    .clone()
                    .unwrap_or_else(|| tab.title.clone())
            } else {
                tab.title.clone()
            };
            offset += format!(" {} {} ", index + 1, label).chars().count() + 1;
        }
        let column = column as usize;
        column >= offset && column < offset + 2
    }

    /// Tab index under a click in the bar, or `None` outside the chips.
    pub(crate) fn tab_hit(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_tab_rect;
        if area.width == 0 || row != area.y || column < area.x || column >= area.x + area.width {
            return None;
        }
        let mut offset = 6usize;
        for (index, tab) in self.tabs.iter().enumerate() {
            let label = if index == self.active_tab {
                self.current_session
                    .clone()
                    .unwrap_or_else(|| tab.title.clone())
            } else {
                tab.title.clone()
            };
            let text = format!(" {} {} ", index + 1, label);
            let start = offset;
            let end = start + text.chars().count();
            let column = column as usize;
            if column >= start && column < end {
                return Some(index);
            }
            offset = end + 1;
        }
        None
    }
}
