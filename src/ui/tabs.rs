//! UiApp tabs: session-backed tabs for the top bar.
//!
//! Each tab holds one or more pane stubs. Settling a tab stashes its
//! live panes (autosaving each session) into the stubs; activating one
//! materializes stubs back into live panes, loading their sessions. A
//! fresh tab starts one unnamed pane. The bar persists to `tabs.json`
//! and restores on startup, loading the active tab so the visible state
//! always matches the bar.

use super::render::accent_color;
use super::*;

/// One pane stub inside a tab: the session file to restore, plus the
/// label shown while the tab is inactive. The live transcript and
/// request state do not persist; sessions carry the conversation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PaneStub {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    #[serde(default)]
    pub(crate) title: String,
}

/// One tab: its pane stubs and which pane had focus. The active tab's
/// labels track the live panes; inactive tabs keep the names they had
/// when the user left them.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Tab {
    #[serde(default)]
    pub(crate) panes: Vec<PaneStub>,
    #[serde(default)]
    pub(crate) focus: usize,
    /// Legacy single-pane fields; migrated into `panes` on load and
    /// dropped on the next save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) title: String,
}

impl Tab {
    pub(crate) fn new(title: &str) -> Self {
        Self {
            panes: vec![PaneStub {
                session: None,
                title: title.to_string(),
            }],
            focus: 0,
            session: None,
            title: String::new(),
        }
    }

    /// Fold a legacy `{session, title}` entry into the pane list.
    pub(crate) fn normalize(mut self) -> Self {
        if self.panes.is_empty() {
            let title = if self.title.is_empty() {
                "session".to_string()
            } else {
                self.title.clone()
            };
            self.panes.push(PaneStub {
                session: self.session.clone(),
                title,
            });
        }
        for (index, pane) in self.panes.iter_mut().enumerate() {
            if pane.title.is_empty() {
                pane.title = format!("session {}", index + 1);
            }
        }
        self.session = None;
        self.title.clear();
        if self.focus >= self.panes.len() {
            self.focus = 0;
        }
        self
    }

    /// The label for pane `index`, preferring the live session name.
    pub(crate) fn label(&self, index: usize, live: Option<&str>) -> String {
        live.map(str::to_string)
            .or_else(|| self.panes.get(index).map(|pane| pane.title.clone()))
            .unwrap_or_else(|| format!("session {}", index + 1))
    }
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

    /// Restore the persisted bar and load its active tab. Missing or
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
        self.tabs = file.tabs.into_iter().map(Tab::normalize).collect();
        self.active_tab = file.active.min(self.tabs.len() - 1);
        // The startup pane may carry a CLI `--session` or model flags:
        // hand it to the first session-less stub instead of dropping it.
        let carried = self.panes.get(self.focus).map(|pane| pane.state.clone());
        self.materialize_active(carried);
    }

    pub(crate) fn save_tabs(&self) {
        let file = TabsFile {
            tabs: self.tabs.clone(),
            active: self.active_tab,
        };
        if let Ok(raw) = serde_json::to_string(&file) {
            let _ = std::fs::write(self.tabs_path(), raw);
        }
    }

    /// Store the live pane names on the active tab before leaving it.
    /// Every pane autosaves, so a tab switch never drops work.
    fn stash_active_tab(&mut self) {
        let paths = self.paths.clone();
        let stubs: Vec<tabs::PaneStub> = self
            .panes
            .iter_mut()
            .enumerate()
            .map(|(index, pane)| {
                let name = pane.autosave(&paths);
                let title = if pane.title.is_empty() {
                    format!("session {}", index + 1)
                } else {
                    pane.title.clone()
                };
                tabs::PaneStub {
                    title,
                    session: name.or_else(|| pane.current_session.clone()),
                }
            })
            .collect();
        let focus = self.focus;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.panes = stubs;
            tab.focus = focus;
        }
    }

    /// Replace the live panes with the active tab's stubs, loading the
    /// sessions they point at (fresh sessions when they point at none).
    /// `carried` is a startup state (possibly CLI-loaded) for the first
    /// session-less stub; later switches pass `None`.
    fn materialize_active(&mut self, mut carried: Option<crate::model::ChatState>) {
        let Some(tab) = self.tabs.get(self.active_tab).cloned() else {
            return;
        };
        let workspace = self
            .panes
            .get(self.focus)
            .map(|pane| pane.state.workspace.clone())
            .unwrap_or_else(|| self.base_state.workspace.clone());
        let mut panes: Vec<Pane> = Vec::with_capacity(tab.panes.len());
        for stub in tab.panes.iter() {
            let state = match &stub.session {
                Some(_) => {
                    let mut state = self.base_state.fresh_like();
                    state.workspace = workspace.clone();
                    state
                }
                None => match carried.take() {
                    Some(state) => state,
                    None => {
                        let mut state = self.base_state.fresh_like();
                        state.workspace = workspace.clone();
                        state
                    }
                },
            };
            let Some(template) = self.panes.first() else {
                return;
            };
            let id = self.next_pane_id;
            self.next_pane_id += 1;
            let mut pane = Pane::new(
                id,
                state,
                template.ghost_debounce,
                template.completion_on,
                template.ai_suggest_on,
            );
            pane.title = stub.title.clone();
            if let Some(name) = &stub.session {
                match session::load(&self.paths, name, &mut pane.state) {
                    Ok(count) => {
                        pane.current_session = Some(name.clone());
                        pane.reseed_msg_ids();
                        pane.prune_sections();
                        pane.sync_mode_from_state();
                        pane.status = format!("Loaded {name} ({count} messages)");
                    }
                    Err(error) => pane.set_error(format!("Session load failed: {error}")),
                }
            }
            panes.push(pane);
        }
        if panes.is_empty() {
            return;
        }
        self.panes = panes;
        self.focus = tab.focus.min(self.panes.len() - 1);
        self.routing = None;
        self.hist_search = None;
        self.refresh_git_branch();
        self.refresh_custom_commands();
        self.refresh_sidebar();
    }

    /// The focused pane's live session name, for labels drawn live.
    pub(crate) fn pane_sessions(&self) -> Vec<String> {
        self.panes
            .iter()
            .enumerate()
            .map(|(index, pane)| {
                pane.current_session.clone().unwrap_or_else(|| {
                    if pane.title.is_empty() {
                        format!("session {}", index + 1)
                    } else {
                        pane.title.clone()
                    }
                })
            })
            .collect()
    }

    /// Any pane still working makes a tab switch unsafe: stashing would
    /// drop its in-flight stream.
    pub(crate) fn panes_busy(&self) -> bool {
        self.panes.iter().any(|pane| pane.busy)
    }

    /// Record the live pane layout on the active tab without saving
    /// anything; names come from each pane's current session.
    pub(crate) fn sync_tab_layout(&mut self) {
        let stubs: Vec<tabs::PaneStub> = self
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| tabs::PaneStub {
                title: if pane.title.is_empty() {
                    format!("session {}", index + 1)
                } else {
                    pane.title.clone()
                },
                session: pane.current_session.clone(),
            })
            .collect();
        let focus = self.focus;
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.panes = stubs;
            tab.focus = focus;
        }
    }

    pub(crate) fn tab_new(&mut self) {
        if self.panes_busy() {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        self.stash_active_tab();
        self.tabs.push(Tab::new("session"));
        self.active_tab = self.tabs.len() - 1;
        self.materialize_active(None);
        self.save_tabs();
        self.set_status(format!("Tab {} · new session", self.active_tab + 1));
    }

    pub(crate) fn tab_switch(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        if self.panes_busy() {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        self.stash_active_tab();
        self.active_tab = index;
        self.materialize_active(None);
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
        if self.panes_busy() {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        let paths = self.paths.clone();
        for pane in &mut self.panes {
            pane.autosave(&paths);
        }
        self.tabs.remove(self.active_tab);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.materialize_active(None);
        self.save_tabs();
        self.set_status(format!("Tab {}", self.active_tab + 1));
    }

    /// The tab-bar row: `r105` brand, one chip per tab (`N label`, the
    /// active chip filled), a `+` affordance, then the right-aligned
    /// mode · provider · model. Clicks resolve against `last_tab_rect`.
    pub(crate) fn draw_tab_bar(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.last_tab_rect = area;
        let connection = self.backend.connection();
        let accent = accent_color(&self.state.theme);
        let sessions = self.pane_sessions();
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            " r105 ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )];
        let mut used = 6usize;
        for (index, tab) in self.tabs.iter().enumerate() {
            let active = index == self.active_tab;
            let label = if active {
                match sessions.len() {
                    0 => tab.title.clone(),
                    1 => sessions[0].clone(),
                    count => format!("{} ({} panes)", sessions[0], count),
                }
            } else {
                tab.label(0, None)
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
                self.pane_sessions().first().cloned().unwrap_or_default()
            } else {
                tab.label(0, None)
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
                self.pane_sessions().first().cloned().unwrap_or_default()
            } else {
                tab.label(0, None)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_tabs_file_migrates_to_pane_stubs() {
        let raw = r#"{"tabs":[{"session":"alpha","title":"alpha"}],"active":0}"#;
        let file: TabsFile = serde_json::from_str(raw).expect("legacy file parses");
        let tabs: Vec<Tab> = file.tabs.into_iter().map(Tab::normalize).collect();
        assert_eq!(tabs[0].panes.len(), 1);
        assert_eq!(tabs[0].panes[0].session.as_deref(), Some("alpha"));
        assert_eq!(tabs[0].panes[0].title, "alpha");
        // The next save writes the pane form; legacy fields are gone.
        let raw = serde_json::to_string(&tabs[0]).unwrap();
        assert!(!raw.contains("\"session\":null"));
        assert!(raw.contains("\"panes\""));
    }

    #[test]
    fn legacy_untitled_tab_defaults_to_session() {
        let raw = r#"{"tabs":[{"session":null,"title":""}],"active":0}"#;
        let file: TabsFile = serde_json::from_str(raw).unwrap();
        let tab = file.tabs.into_iter().next().unwrap().normalize();
        assert_eq!(tab.panes.len(), 1);
        assert_eq!(tab.panes[0].title, "session");
        assert!(tab.panes[0].session.is_none());
    }
}
