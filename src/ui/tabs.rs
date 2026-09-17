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

/// Direction of a split inside a tab. `Right` keeps the two children
/// side-by-side; `Down` stacks them vertically. The tree is intentionally
/// small and serializable so a restored tab keeps the exact arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum SplitDirection {
    Right,
    Down,
}

/// Persistent pane layout. Leaves refer to the pane's position in the
/// tab's pane list; insert/remove operations keep those indexes in sync.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum LayoutNode {
    Pane(usize),
    Split {
        direction: SplitDirection,
        #[serde(default = "default_split_ratio")]
        ratio: u16,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

fn default_split_ratio() -> u16 {
    500
}

impl LayoutNode {
    /// Build a balanced left-to-right layout for old tab files that had no
    /// layout field, or when a hand-edited file is invalid.
    pub(crate) fn flat(count: usize) -> Option<Self> {
        fn build(start: usize, end: usize) -> LayoutNode {
            if end - start == 1 {
                return LayoutNode::Pane(start);
            }
            let mid = start + (end - start) / 2;
            let left = mid - start;
            let total = end - start;
            let ratio = ((left as u32 * 1000) / total as u32).clamp(100, 900) as u16;
            LayoutNode::Split {
                direction: SplitDirection::Right,
                ratio,
                first: Box::new(build(start, mid)),
                second: Box::new(build(mid, end)),
            }
        }

        (count > 0).then(|| build(0, count))
    }

    pub(crate) fn leaves(&self, output: &mut Vec<usize>) {
        match self {
            LayoutNode::Pane(index) => output.push(*index),
            LayoutNode::Split { first, second, .. } => {
                first.leaves(output);
                second.leaves(output);
            }
        }
    }

    pub(crate) fn valid_for(&self, count: usize) -> bool {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        leaves.len() == count
            && leaves.iter().copied().eq(0..count)
            && leaves.iter().all(|index| *index < count)
    }

    /// Replace one leaf with a two-child split. The caller inserts the new
    /// pane into the pane vector and reindexes existing leaves first.
    pub(crate) fn split_leaf(
        &mut self,
        target: usize,
        new_index: usize,
        direction: SplitDirection,
    ) -> bool {
        match self {
            LayoutNode::Pane(index) if *index == target => {
                *self = LayoutNode::Split {
                    direction,
                    ratio: default_split_ratio(),
                    first: Box::new(LayoutNode::Pane(target)),
                    second: Box::new(LayoutNode::Pane(new_index)),
                };
                true
            }
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                first.split_leaf(target, new_index, direction)
                    || second.split_leaf(target, new_index, direction)
            }
        }
    }

    pub(crate) fn reindex_insert(&mut self, index: usize) {
        match self {
            LayoutNode::Pane(value) => {
                if *value >= index {
                    *value += 1;
                }
            }
            LayoutNode::Split { first, second, .. } => {
                first.reindex_insert(index);
                second.reindex_insert(index);
            }
        }
    }

    pub(crate) fn reindex_remove(&mut self, index: usize) {
        match self {
            LayoutNode::Pane(value) => {
                if *value > index {
                    *value -= 1;
                }
            }
            LayoutNode::Split { first, second, .. } => {
                first.reindex_remove(index);
                second.reindex_remove(index);
            }
        }
    }

    /// Remove a leaf and collapse its parent. A root leaf is never removed
    /// by the UI because the last pane is represented by the tab itself.
    pub(crate) fn remove_leaf(&mut self, target: usize) -> bool {
        match self {
            LayoutNode::Pane(_) => false,
            LayoutNode::Split { first, second, .. } => {
                if matches!(first.as_ref(), LayoutNode::Pane(index) if *index == target) {
                    *self = (**second).clone();
                    true
                } else if matches!(second.as_ref(), LayoutNode::Pane(index) if *index == target) {
                    *self = (**first).clone();
                    true
                } else {
                    first.remove_leaf(target) || second.remove_leaf(target)
                }
            }
        }
    }
}

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
    /// Nested split arrangement for this tab. `None` is accepted for old
    /// files and normalized to a flat layout on load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) layout: Option<LayoutNode>,
    /// Temporarily show only the focused pane while keeping the split intact.
    #[serde(default)]
    pub(crate) zoomed: bool,
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
            layout: Some(LayoutNode::Pane(0)),
            zoomed: false,
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
        if self
            .layout
            .as_ref()
            .is_none_or(|layout| !layout.valid_for(self.panes.len()))
        {
            self.layout = LayoutNode::flat(self.panes.len());
        }
        if self.panes.len() <= 1 {
            self.zoomed = false;
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
    /// Ensure the active tab has a valid layout before drawing or mutating
    /// panes. This also repairs old or manually edited tabs safely.
    pub(crate) fn ensure_active_layout(&mut self) {
        let count = self.panes.len();
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        if tab
            .layout
            .as_ref()
            .is_none_or(|layout| !layout.valid_for(count))
        {
            tab.layout = LayoutNode::flat(count);
        }
        if count <= 1 {
            tab.zoomed = false;
        }
    }

    pub(crate) fn active_layout(&self) -> Option<&LayoutNode> {
        self.tabs
            .get(self.active_tab)
            .and_then(|tab| tab.layout.as_ref())
    }

    pub(crate) fn pane_zoomed(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .map(|tab| tab.zoomed)
            .unwrap_or(false)
    }

    pub(crate) fn toggle_pane_zoom(&mut self) {
        if self.panes.len() <= 1 {
            self.set_status("Only pane already fills the tab".into());
            return;
        }
        self.ensure_active_layout();
        let zoomed = if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.zoomed = !tab.zoomed;
            tab.zoomed
        } else {
            return;
        };
        self.set_status(if zoomed {
            "Pane maximized · Ctrl+Shift+Enter restores".into()
        } else {
            "Pane layout restored".into()
        });
        self.save_tabs();
    }

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
        self.ensure_active_layout();
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
            // The layout refers to the same pane order and remains intact.
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
        self.ensure_active_layout();
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
        self.ensure_active_layout();
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

    /// Move an inactive tab one position without touching its persisted
    /// sessions. The active live tab stays materialized in place.
    pub(crate) fn tab_move(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        let target = if forward {
            if self.active_tab + 1 >= count {
                return;
            }
            self.active_tab + 1
        } else {
            if self.active_tab == 0 {
                return;
            }
            self.active_tab - 1
        };
        self.tabs.swap(self.active_tab, target);
        self.active_tab = target;
        self.save_tabs();
        self.set_status(format!("Tab moved · {} of {}", target + 1, count));
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

    /// Close a tab selected by its bar close affordance. An inactive tab has
    /// no live panes, so it can be removed immediately.
    pub(crate) fn tab_close_at(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if index == self.active_tab {
            self.tab_close();
            return;
        }
        if self.tabs.len() <= 1 {
            self.set_status("Last tab stays open".into());
            return;
        }
        self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.save_tabs();
        self.set_status(format!("Closed tab · {} remaining", self.tabs.len()));
    }

    /// The tab-bar row: one chip per tab, a close affordance on the active
    /// chip, a `+` affordance, then the right-aligned connection summary.
    /// Clicks resolve against `last_tab_rect`.
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
            let text = self.tab_chip_text(index, tab, &sessions);
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
            spans.push(Span::styled(right, Style::default().fg(Color::DarkGray)));
        } else if used < width {
            spans.push(Span::raw(" ".repeat(width - used)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn tab_chip_text(&self, index: usize, tab: &Tab, sessions: &[String]) -> String {
        let active = index == self.active_tab;
        let label = if active {
            match sessions.len() {
                0 => tab.title.clone(),
                1 => sessions[0].clone(),
                count => format!("{} · {} panes", sessions[0], count),
            }
        } else if tab.panes.len() > 1 {
            format!("{} · {} panes", tab.label(0, None), tab.panes.len())
        } else {
            tab.label(0, None)
        };
        let label = compact_tab_label(&label, 28);
        if active {
            format!(" {} {} × ", index + 1, label)
        } else {
            format!(" {} {} ", index + 1, label)
        }
    }

    fn tab_regions(&self) -> Vec<(usize, usize, usize)> {
        let area = self.last_tab_rect;
        let sessions = self.pane_sessions();
        let mut offset = area.x as usize + 6;
        let limit = area.x as usize + area.width as usize;
        let mut regions = Vec::with_capacity(self.tabs.len());
        for (index, tab) in self.tabs.iter().enumerate() {
            let text = self.tab_chip_text(index, tab, &sessions);
            let start = offset;
            let end = start.saturating_add(text.chars().count());
            if start >= limit {
                break;
            }
            regions.push((index, start, end.min(limit)));
            offset = end.saturating_add(1);
        }
        regions
    }

    /// Whether a click lands on the `+` affordance after the chips.
    pub(crate) fn tab_plus_hit(&self, column: u16, row: u16) -> bool {
        let area = self.last_tab_rect;
        if area.width == 0 || row != area.y || column < area.x || column >= area.x + area.width {
            return false;
        }
        let offset = self
            .tab_regions()
            .last()
            .map(|(_, _, end)| end + 1)
            .unwrap_or(area.x as usize + 6);
        let column = column as usize;
        column >= offset && column < offset + 2
    }

    /// The active chip's `×` cell, used for direct mouse closure.
    pub(crate) fn tab_close_hit(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_tab_rect;
        if area.width == 0 || row != area.y {
            return None;
        }
        self.tab_regions().into_iter().find_map(|(index, _, end)| {
            if index == self.active_tab && column as usize + 2 == end {
                Some(index)
            } else {
                None
            }
        })
    }

    /// Tab index under a click in the bar, or `None` outside the chips.
    pub(crate) fn tab_hit(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.last_tab_rect;
        if area.width == 0 || row != area.y || column < area.x || column >= area.x + area.width {
            return None;
        }
        let column = column as usize;
        for (index, start, end) in self.tab_regions() {
            if column >= start && column < end {
                return Some(index);
            }
        }
        None
    }
}

fn compact_tab_label(label: &str, max_chars: usize) -> String {
    let count = label.chars().count();
    if count <= max_chars {
        return label.to_string();
    }
    let keep = max_chars.saturating_sub(1).max(1);
    let mut compact: String = label.chars().take(keep).collect();
    compact.push('…');
    compact
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

    #[test]
    fn layout_tree_inserts_and_removes_without_losing_order() {
        let mut layout = LayoutNode::flat(2).expect("two leaves");
        layout.reindex_insert(1);
        assert!(layout.split_leaf(0, 1, SplitDirection::Down));
        assert!(layout.valid_for(3));
        assert!(layout.remove_leaf(1));
        layout.reindex_remove(1);
        assert!(layout.valid_for(2));
    }

    #[test]
    fn invalid_layout_repairs_to_flat() {
        let mut tab = Tab::new("session");
        tab.panes.push(PaneStub {
            session: None,
            title: "session 2".into(),
        });
        tab.layout = Some(LayoutNode::Pane(99));
        let tab = tab.normalize();
        assert!(
            tab.layout
                .as_ref()
                .is_some_and(|layout| layout.valid_for(2))
        );
    }
}
