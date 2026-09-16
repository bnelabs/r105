//! UiApp reverse history search (Ctrl+R).
//!
//! The search is a live filter over the shell history: typing narrows,
//! the highlighted match previews in the composer, Enter accepts it,
//! and Esc restores the draft that was there before the search opened.

use super::*;

/// Live reverse-search state: the query, the highlighted match, and the
/// draft to restore when the search is cancelled.
pub(crate) struct HistSearch {
    pub(crate) query: String,
    pub(crate) selected: usize,
    pub(crate) restore: String,
    pub(crate) restore_cursor: usize,
}

/// Match cap: enough to scroll, small enough to draw cheaply.
const SEARCH_CAP: usize = 40;

impl UiApp {
    pub(crate) fn hist_open(&self) -> bool {
        self.hist_search.is_some()
    }

    /// Current matches, newest first and deduped.
    pub(crate) fn hist_matches(&self) -> Vec<String> {
        let Some(search) = &self.hist_search else {
            return Vec::new();
        };
        self.shell_history.search(&search.query, SEARCH_CAP)
    }

    /// Open the search. A single-line draft seeds the query so
    /// `git` + Ctrl+R starts with git commands; the draft is kept for
    /// Esc.
    pub(crate) fn open_hist_search(&mut self) {
        if self.hist_search.is_some() {
            return;
        }
        let seed = if self.input.contains('\n') {
            String::new()
        } else {
            self.input.trim().to_string()
        };
        self.hist_search = Some(HistSearch {
            query: seed,
            selected: 0,
            restore: self.input.clone(),
            restore_cursor: self.cursor,
        });
        self.preview_hist_selection();
    }

    /// Enter accepts the highlighted match into the composer; Esc
    /// restores the pre-search draft. Either way the search closes.
    pub(crate) fn close_hist_search(&mut self, accept: bool) {
        let Some(search) = self.hist_search.take() else {
            return;
        };
        let matches = self.shell_history.search(&search.query, SEARCH_CAP);
        if accept && let Some(pick) = matches.get(search.selected) {
            self.input = pick.clone();
            self.cursor = self.input.len();
            self.ghost_seen_input = self.input.clone();
            self.ghost_text = None;
            self.ghost_changed_at = Instant::now();
            self.set_status("History".into());
            return;
        }
        self.input = search.restore;
        self.cursor = search.restore_cursor.min(self.input.len());
        if accept {
            self.set_status("No history match".into());
        }
    }

    /// Move the highlight, clamped to the match list.
    pub(crate) fn hist_move(&mut self, forward: bool) {
        let count = self.hist_matches().len();
        if count == 0 {
            return;
        }
        let Some(search) = self.hist_search.as_mut() else {
            return;
        };
        search.selected = if forward {
            (search.selected + 1).min(count - 1)
        } else {
            search.selected.saturating_sub(1)
        };
        self.preview_hist_selection();
    }

    /// Composer preview of the highlighted match while the search is
    /// open; an empty match list leaves the previous draft in place.
    fn preview_hist_selection(&mut self) {
        let matches = self.hist_matches();
        let selected = self
            .hist_search
            .as_ref()
            .map(|search| search.selected)
            .unwrap_or(0);
        if let Some(pick) = matches.get(selected) {
            self.input = pick.clone();
            self.cursor = self.input.len();
        }
    }

    /// Type the next query character (or backspace/cut from the caller).
    pub(crate) fn hist_query_push(&mut self, ch: char) {
        if let Some(search) = self.hist_search.as_mut() {
            search.query.push(ch);
            search.selected = 0;
        }
        self.preview_hist_selection();
    }

    pub(crate) fn hist_query_backspace(&mut self) {
        if let Some(search) = self.hist_search.as_mut() {
            search.query.pop();
            search.selected = 0;
        }
        self.preview_hist_selection();
    }

    pub(crate) fn hist_query_clear(&mut self) {
        if let Some(search) = self.hist_search.as_mut() {
            search.query.clear();
            search.selected = 0;
        }
        self.preview_hist_selection();
    }

    /// Keys while the search is open: type to narrow, ↑↓/Ctrl+R cycle,
    /// Ctrl+U/W edit the query, Enter accepts, Esc cancels.
    pub(crate) fn handle_hist_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.close_hist_search(false),
            KeyCode::Enter => self.close_hist_search(true),
            KeyCode::Up => self.hist_move(false),
            KeyCode::Down => self.hist_move(true),
            KeyCode::Backspace => self.hist_query_backspace(),
            KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.hist_move(true);
            }
            KeyCode::Char('u' | 'U') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.hist_query_clear();
            }
            KeyCode::Char('w' | 'W') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.hist_query_drop_word();
            }
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.hist_query_push(ch);
            }
            _ => {}
        }
    }

    /// Drop the query's last word (Ctrl+W), keeping interior spaces.
    pub(crate) fn hist_query_drop_word(&mut self) {
        if let Some(search) = self.hist_search.as_mut() {
            let trimmed = search.query.trim_end();
            let cut = trimmed
                .rfind(char::is_whitespace)
                .map_or(0, |index| index + 1);
            search.query = trimmed[..cut].trim_end().to_string();
            search.selected = 0;
        }
        self.preview_hist_selection();
    }

    /// Panel rows: `(text, selected)`.
    pub(crate) fn hist_rows(&self) -> Vec<(String, bool)> {
        let selected = self
            .hist_search
            .as_ref()
            .map(|search| search.selected)
            .unwrap_or(0);
        self.hist_matches()
            .into_iter()
            .enumerate()
            .map(|(index, text)| (text, index == selected))
            .collect()
    }

    /// Scroll wheel over the panel moves the highlight.
    pub(crate) fn hist_wheel(&mut self, down: bool) {
        for _ in 0..3 {
            self.hist_move(down);
        }
    }

    /// Click on the panel: select the row, or accept it when the
    /// clicked row is already highlighted (mirroring the other menus).
    pub(crate) fn click_hist_search(&mut self, column: u16, row: u16) -> bool {
        let Some(rect) = self.last_hist_rect else {
            return false;
        };
        if rect.width == 0
            || column < rect.x
            || column >= rect.x + rect.width
            || row < rect.y
            || row >= rect.y + rect.height
            || self.last_hist_count == 0
        {
            return false;
        }
        let viewport = rect.height.saturating_sub(2) as usize;
        let offset = (row.saturating_sub(rect.y).saturating_sub(1)) as usize;
        if offset >= viewport {
            return false;
        }
        let index = self.hist_scroll() + offset;
        if index >= self.last_hist_count {
            return false;
        }
        let already = self
            .hist_search
            .as_ref()
            .is_some_and(|search| search.selected == index);
        if already {
            self.close_hist_search(true);
        } else if let Some(search) = self.hist_search.as_mut() {
            search.selected = index;
            self.preview_hist_selection();
        }
        true
    }

    /// Scroll offset for the drawn panel, mirroring the draw math.
    pub(crate) fn hist_scroll(&self) -> usize {
        let viewport = self
            .last_hist_rect
            .map(|rect| rect.height.saturating_sub(2) as usize)
            .unwrap_or(8)
            .max(1);
        let selected = self
            .hist_search
            .as_ref()
            .map(|search| search.selected)
            .unwrap_or(0);
        command::ensure_visible(selected, 0, viewport, self.last_hist_count)
    }
}
