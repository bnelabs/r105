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
    /// resolves nothing (single-owner rule, spec 0018). Marker-free
    /// shell lines resolve exactly like `!` lines — no prefix needed.
    pub(crate) fn tick_ghost(&mut self) {
        if !self.completion_on {
            return;
        }
        if self.input != self.ghost_seen_input {
            self.ghost_seen_input = self.input.clone();
            self.ghost_changed_at = Instant::now();
            self.ghost_text = None;
            self.ghost_dismissed = None;
            self.ai_ghost_pending = None;
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
        // Warm the alias table before detection: `g st` only reads as
        // shell once the rc-file alias is known.
        let cwd = self.state.workspace.clone();
        self.ctx_cache.refresh_for(&self.input, &cwd);
        let Some((_, prefix)) = self.shell_line() else {
            return;
        };
        self.ctx_cache.refresh_for(&prefix, &cwd);
        self.kick_live_refresh(&prefix);
        self.ghost_text = crate::suggest::suggest_shell(
            &prefix,
            &self.shell_history,
            &cwd,
            &self.bin_cache,
            &self.ctx_cache,
            super::complete::score_file_candidate,
        );
        if self.ghost_text.is_none() {
            self.kick_ai_ghost(&prefix);
        }
    }

    /// Background daemon queries behind the live value kinds: short
    /// commands under a 2s timeout, at most one flight per key per TTL.
    /// Results (and failures) return as `UiEvent::LiveValues`; the
    /// keystroke path only ever reads the cache, never waits.
    pub(crate) fn kick_live_refresh(&mut self, line: &str) {
        const QUERIES: &[(&str, &str, &[&str])] = &[
            (
                "k8s-ns",
                "kubectl",
                &["get", "namespaces", "-o=jsonpath={.items[*].metadata.name}"],
            ),
            (
                "k8s-pods",
                "kubectl",
                &["get", "pods", "-o=jsonpath={.items[*].metadata.name}"],
            ),
            (
                "docker-containers",
                "docker",
                &["ps", "--format", "{{.Names}}"],
            ),
            (
                "docker-images",
                "docker",
                &["images", "--format", "{{.Repository}}:{{.Tag}}"],
            ),
        ];
        let command = {
            let cache = &self.ctx_cache;
            crate::suggest::shell_context(line, &cache.aliases, &cache.git_alias_list)
                .map(|context| context.command)
                .unwrap_or_default()
        };
        let now = Instant::now();
        for (key, program, args) in QUERIES {
            let watched = match *key {
                "k8s-ns" | "k8s-pods" => command == "kubectl",
                "docker-containers" | "docker-images" => command == "docker",
                _ => false,
            };
            if !watched {
                continue;
            }
            let live = &mut self.ctx_cache.live;
            if live.inflight.contains(*key) {
                continue;
            }
            if live
                .at
                .get(*key)
                .is_some_and(|at| now - *at < crate::suggest::LIVE_TTL)
            {
                continue;
            }
            if live
                .failed_at
                .get(*key)
                .is_some_and(|at| now - *at < crate::suggest::LIVE_FAIL_QUIET)
            {
                continue;
            }
            live.inflight.insert(key.to_string());
            let sender = self.tx.clone();
            let key = key.to_string();
            let program = program.to_string();
            let args: Vec<String> = args.iter().map(ToString::to_string).collect();
            tokio::spawn(async move {
                let mut command = tokio::process::Command::new(&program);
                command.args(&args).kill_on_drop(true);
                let output =
                    tokio::time::timeout(std::time::Duration::from_secs(2), command.output()).await;
                let (ok, values) = match output {
                    Ok(Ok(out)) if out.status.success() => {
                        let text = String::from_utf8_lossy(&out.stdout);
                        let mut values: Vec<String> =
                            text.split_whitespace().map(str::to_string).collect();
                        values.truncate(200);
                        (true, values)
                    }
                    _ => (false, Vec::new()),
                };
                let _ = sender.send(crate::ui::events::UiEvent::LiveValues { key, values, ok });
            });
        }
    }

    /// Model ghost for shell lines the local cascade cannot extend:
    /// debounced well past the local layers, idle-only, one flight per
    /// input generation. The model returns a full line; only a true
    /// extension of the typed text becomes a ghost. Failures are
    /// silent — no model, no ghost, no nagging.
    pub(crate) fn kick_ai_ghost(&mut self, prefix: &str) {
        const AI_DEBOUNCE: Duration = Duration::from_millis(900);
        if !self.ai_suggest_on || prefix.trim().len() < 3 {
            return;
        }
        // No async runtime (unit tests): the local cascade stands alone.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        // A complete command (`git status`) has nothing to extend.
        if crate::suggest::line_looks_complete(prefix, &self.ctx_cache) {
            return;
        }
        if self.ghost_changed_at.elapsed() < AI_DEBOUNCE {
            return;
        }
        if self.ai_ghost_pending.as_deref() == Some(prefix) {
            return;
        }
        self.ai_ghost_seq += 1;
        let seq = self.ai_ghost_seq;
        self.ai_ghost_pending = Some(prefix.to_string());
        let backend = self.backend.clone();
        let mut state = self.state.clone();
        state.history.clear();
        let sender = self.tx.clone();
        let prefix = prefix.to_string();
        let for_input = self.input.clone();
        let prompt = format!(
            "The user typed this partial shell command:\n{prefix}\n\
             Output ONLY the completed full command on one line: no fences, \
             no explanation. If it is complete already or is not a shell \
             command, repeat it unchanged."
        );
        tokio::spawn(async move {
            let suffix = match backend.chat(&state, &prompt, &[]).await {
                Ok(result) => clean_shell_draft(&result.content)
                    .and_then(|full| full.strip_prefix(prefix.as_str()).map(str::to_string))
                    .filter(|rest| !rest.trim().is_empty() && !rest.contains('\n'))
                    .map(|rest| rest.chars().take(120).collect::<String>()),
                Err(_) => None,
            };
            let _ = sender.send(crate::ui::events::UiEvent::AiGhost {
                seq,
                for_input,
                suffix,
            });
        });
    }

    /// PATH executables feed the command-name ghost layer. The scan is
    /// cheap but not per-keystroke free, so it refreshes on a TTL.
    pub(crate) fn refresh_bin_cache(&mut self) {
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

    /// Accept one word of the visible ghost (leading whitespace plus
    /// the next word run), leaving the rest for the tick to re-offer —
    /// the partial-accept half of history-style autosuggestion.
    pub(crate) fn accept_ghost_word(&mut self) -> bool {
        if self.cursor != self.input.len() {
            return false;
        }
        let Some(ghost) = self.ghost_text.take() else {
            return false;
        };
        let mut end = 0;
        let mut chars = ghost.char_indices().peekable();
        while let Some((index, char)) = chars.peek() {
            if !char.is_whitespace() {
                break;
            }
            end = index + char.len_utf8();
            chars.next();
        }
        for (index, char) in chars {
            if char.is_whitespace() {
                break;
            }
            end = index + char.len_utf8();
        }
        if end == 0 {
            return false;
        }
        self.input.push_str(&ghost[..end]);
        self.cursor = self.input.len();
        self.ghost_seen_input = self.input.clone();
        self.ghost_dismissed = None;
        true
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
                self.set_status("Suggestions cleared".into());
            }
            "ai" => match args.get(1).map(String::as_str).unwrap_or("status") {
                "on" => {
                    self.ai_suggest_on = true;
                    self.set_status("Model ghost on".into());
                }
                "off" => {
                    self.ai_suggest_on = false;
                    self.set_status("Model ghost off".into());
                }
                "status" => self.set_status(if self.ai_suggest_on {
                    "Model ghost on · local cascade first".into()
                } else {
                    "Model ghost off".into()
                }),
                other => self.set_error(format!("Usage: /completion ai [on|off] (got {other})")),
            },
            "status" => {
                self.set_status(if self.completion_on {
                    format!(
                        "Suggestions on · {} remembered · model ghost {} · Tab accepts",
                        self.shell_history.len(),
                        if self.ai_suggest_on { "on" } else { "off" },
                    )
                } else {
                    "Suggestions off".into()
                });
            }
            other => self.set_error(format!(
                "Usage: /completion [status|clear|ai on|ai off] (got {other})"
            )),
        }
    }
}
