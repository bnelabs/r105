//! UiApp ghost text: synchronous completion-cascade scheduling.
//!
//! The tick owns scheduling: input changes invalidate the shown ghost,
//! qualifying input debounces into an instant history/path lookup (no
//! flights, no generations — both layers resolve in microseconds).
//! Dismissal sticks until the next edit; acceptance chains naturally
//! because the longer input re-triggers the tick.

use super::*;
use crate::suggest::ShellHistory;

impl UiApp {
    fn history_path(&self) -> PathBuf {
        self.paths.config_dir.join("shell_history.json")
    }

    /// 45ms-tick scheduler: invalidate on edit, debounce, resolve.
    /// Menus own the composer while open: the tick clears any ghost and
    /// resolves nothing (single-owner rule, spec 0018).
    pub(crate) fn tick_ghost(&mut self) {
        if !self.completion_on {
            return;
        }
        if self.input != self.ghost_seen_input {
            self.ghost_seen_input = self.input.clone();
            self.ghost_changed_at = Instant::now();
            self.ghost_text = None;
            self.ghost_dismissed = None;
        }
        if self.menu_wants_input() || self.hist_depth.is_some() {
            self.ghost_text = None;
            return;
        }
        if self.ghost_text.is_some() || !matches!(self.overlay, Overlay::None) || self.busy {
            return;
        }
        if self.ghost_changed_at.elapsed() < self.ghost_debounce {
            return;
        }
        if self.ghost_dismissed.as_deref() == Some(self.input.as_str()) {
            return;
        }
        self.refresh_bin_cache();
        let cwd = self.state.workspace.clone();
        self.ghost_text = crate::suggest::suggest(
            &self.input,
            &self.shell_history,
            &cwd,
            &self.bin_cache,
            super::complete::score_file_candidate,
        );
    }

    /// PATH executables feed the command-name ghost layer. The scan is
    /// cheap but not per-keystroke free, so it refreshes on a TTL.
    fn refresh_bin_cache(&mut self) {
        const TTL: Duration = Duration::from_secs(30);
        if self.bin_cache_at.is_none_or(|at| at.elapsed() >= TTL) {
            self.bin_cache = crate::suggest::scan_path_bins();
            self.bin_cache_at = Some(Instant::now());
        }
    }

    /// Accept the visible ghost when the cursor is at the end of input.
    /// The longer input re-triggers the tick, chaining continuations.
    pub(crate) fn accept_ghost(&mut self) -> bool {
        if self.cursor != self.input.len() {
            return false;
        }
        if let Some(ghost) = self.ghost_text.take() {
            self.input.push_str(&ghost);
            self.cursor = self.input.len();
            self.ghost_seen_input = self.input.clone();
            self.ghost_dismissed = None;
            return true;
        }
        false
    }

    /// Dismiss without accepting; the tick will not resurrect it until
    /// the input changes.
    pub(crate) fn dismiss_ghost(&mut self) -> bool {
        if self.ghost_text.take().is_none() {
            return false;
        }
        self.ghost_dismissed = Some(self.input.clone());
        true
    }

    /// Record an executed shell line for the frequency layer; the file
    /// write fails silent — history is a convenience, not a promise.
    pub(crate) fn record_shell(&mut self, cmd: &str) {
        let cwd = self.state.workspace.to_string_lossy().to_string();
        self.shell_history.record(cmd, &cwd);
        let path = self.history_path();
        let _ = self.shell_history.save(&path);
    }

    pub(crate) async fn command_completion(&mut self, args: &[String]) {
        match args.first().map(String::as_str).unwrap_or("status") {
            "clear" => {
                self.shell_history = ShellHistory::new(self.history_max);
                self.ghost_text = None;
                let path = self.history_path();
                let _ = self.shell_history.save(&path);
                self.set_status("Ghost history cleared".into());
            }
            "status" => {
                self.set_status(if self.completion_on {
                    format!(
                        "Ghost on · {} shell command(s) remembered · debounce {}ms · Tab accepts · Esc dismisses",
                        self.shell_history.len(),
                        self.ghost_debounce.as_millis(),
                    )
                } else {
                    "Ghost completion disabled (completion_enabled=false)".into()
                });
            }
            other => self.set_error(format!("Usage: /completion [status|clear] (got {other})")),
        }
    }
}
