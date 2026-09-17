//! UiApp complete: Palette, `@` file menu, and argument-value completion.

use super::*;

impl UiApp {
    pub(crate) fn palette_active(&self) -> bool {
        matches!(self.overlay, Overlay::None)
            && self.input.starts_with('/')
            && !self.input.chars().any(char::is_whitespace)
    }

    /// Single-owner rule (spec 0018): while a menu would own the next
    /// Tab/↑/↓, the ghost stays hidden. True for an open overlay, the
    /// slash palette, a live `@path` token, or a slash-command argument
    /// position with candidates. Cheap: no filesystem work.
    pub(crate) fn menu_wants_input(&mut self) -> bool {
        if !matches!(self.overlay, Overlay::None) {
            return true;
        }
        // A focused pane owns the next keystroke, so the ghost stands
        // down the same way it does for open menus.
        if self.sidebar_focus {
            self.ghost_text = None;
            return true;
        }
        if self.hist_open() {
            self.ghost_text = None;
            return true;
        }
        if self.palette_active() {
            return true;
        }
        if self.at_token().is_some() {
            return true;
        }
        if !self.arg_menu_items().is_empty() {
            return true;
        }
        !self.sh_menu_items().is_empty()
    }

    pub(crate) fn palette_items(&self) -> Vec<command::PaletteItem> {
        let mut items = command::palette_items(self.input.trim(), &self.custom_commands);
        // Recency is the primary sort key (stable: fuzzy order survives
        // within a tier), mirroring the palette's (priority, match kind,
        // score) tiers. The fuzzy tiers in `command::palette_items`
        // already encode exact > prefix > substring, so only recency is
        // added here.
        items.sort_by_key(|item| {
            let name = item.name.to_ascii_lowercase();
            self.recent_commands
                .iter()
                .position(|recent| *recent == name)
                .unwrap_or(usize::MAX)
        });
        for item in &mut items {
            self.decorate_palette_item(item);
        }
        items
    }

    /// Remember a dispatched slash command for palette recency boosting.
    /// Unknown names (typos) are never recorded — only real dispatches.
    pub(crate) fn note_recent(&mut self, name: &str) {
        let name = name.to_ascii_lowercase();
        self.recent_commands.retain(|item| *item != name);
        self.recent_commands.push_front(name);
        self.recent_commands.truncate(8);
    }

    /// Append the live value to stateful palette rows (`/theme` shows
    /// `· now dracula`). The active mode command gets `· active` instead.
    /// Rows without readable state keep their plain description.
    pub(crate) fn decorate_palette_item(&self, item: &mut command::PaletteItem) {
        let mode = self.mode.as_str();
        let badge = match item.name.as_str() {
            "/theme" => Some(format!("now {}", self.state.theme)),
            "/model" => Some(format!("now {}", self.state.model)),
            "/quality" => Some(format!(
                "now {}",
                self.state.quality.as_deref().unwrap_or("auto")
            )),
            "/profile" => Some(format!(
                "now {}",
                self.state.profile.as_deref().unwrap_or("auto")
            )),
            "/reasoning" => Some(format!("now {}", self.state.reasoning_effort)),
            "/plan" if mode == "plan" => Some("active".to_string()),
            "/build" if mode == "build" => Some("active".to_string()),
            "/ask" if mode == "ask" => Some("active".to_string()),
            _ => None,
        };
        if let Some(badge) = badge {
            item.description = format!("{} · {badge}", item.description);
        }
    }

    /// Exact `/name` match against loaded custom commands (input is
    /// already lowercased by the parser; names are stored lowercased).
    pub(crate) fn is_custom_command(&self, value: &str) -> bool {
        self.custom_commands
            .iter()
            .any(|command| format!("/{}", command.name) == value.to_ascii_lowercase())
    }

    pub(crate) fn find_custom_command(&self, name: &str) -> Option<CustomCommand> {
        let name = name.strip_prefix('/').unwrap_or(name).to_ascii_lowercase();
        self.custom_commands
            .iter()
            .find(|command| command.name == name)
            .cloned()
    }

    /// The `@path` token immediately before the cursor, if any. The `@`
    /// must start the line or follow whitespace so `user@host` never counts.
    pub(crate) fn at_token(&self) -> Option<String> {
        let end = self.cursor.min(self.input.len());
        let before = self.input.get(..end)?;
        let at = before.rfind('@')?;
        if at > 0
            && !before
                .get(..at)
                .is_some_and(|head| head.ends_with(char::is_whitespace))
        {
            return None;
        }
        let token = before.get(at + 1..)?;
        if token.is_empty() || !token.chars().all(is_path_char) {
            return None;
        }
        Some(token.to_string())
    }

    pub(crate) fn at_menu_items(&mut self) -> Vec<String> {
        if !matches!(self.overlay, Overlay::None) || self.palette_active() {
            return Vec::new();
        }
        let Some(query) = self.at_token() else {
            return Vec::new();
        };
        let key = format!("{}:{}", self.input, self.cursor);
        if key == self.at_cache_key {
            return self.at_cache_items.clone();
        }
        let items = complete_files(&self.state.workspace, &query);
        self.at_cache_key = key;
        self.at_selected = 0;
        self.at_cache_items = items.clone();
        items
    }

    pub(crate) fn at_menu_open(&mut self) -> bool {
        !self.at_menu_items().is_empty()
    }

    pub(crate) fn at_menu_active(&mut self) -> bool {
        !self.at_cache_key.is_empty()
            && self.at_token().is_some()
            && !self.at_cache_items.is_empty()
            && format!("{}:{}", self.input, self.cursor) == self.at_cache_key
    }

    /// Value completion for a command's argument (`/theme dr<Tab>`). Only
    /// the argument right after the command (or one subcommand deeper for
    /// `/skill` and `/session`) completes; anything more complex stays
    /// manual. Requires the cursor at the end of a single-line input so
    /// replacement is a plain suffix swap.
    pub(crate) fn arg_menu_items(&mut self) -> Vec<String> {
        if !matches!(self.overlay, Overlay::None)
            || self.palette_active()
            || self.cursor != self.input.len()
            || self.input.contains('\n')
            || self.at_token().is_some()
        {
            return Vec::new();
        }
        let trailing_space = self.input.ends_with(char::is_whitespace);
        let mut words: Vec<&str> = self.input.split_whitespace().collect();
        if words.is_empty() || !words[0].starts_with('/') {
            return Vec::new();
        }
        // `/cmd ` (trailing space) means an empty token is being completed.
        if trailing_space {
            words.push("");
        }
        if words.len() != 2 && words.len() != 3 {
            return Vec::new();
        }
        let key = format!("{}:{}", self.input, self.cursor);
        if key == self.arg_cache_key {
            return self.arg_cache_items.clone();
        }
        let token = words.last().unwrap_or(&"").to_ascii_lowercase();
        let mut candidates = self.arg_candidates(words[0], words.get(1));
        candidates.retain(|candidate| candidate.to_ascii_lowercase().starts_with(&token));
        candidates.truncate(12);
        self.arg_cache_key = key;
        self.arg_selected = 0;
        self.arg_cache_items = candidates.clone();
        candidates
    }

    /// Candidate values for the argument under the cursor. `first` is the
    /// already-typed first argument when completing the second position.
    pub(crate) fn arg_candidates(&self, command: &str, first: Option<&&str>) -> Vec<String> {
        let command = command.to_ascii_lowercase();
        // Second position: names of skills and sessions behind their
        // subcommands.
        if let Some(first) = first.filter(|_| self.arg_position() == 2) {
            match (command.as_str(), first.to_ascii_lowercase().as_str()) {
                ("/skill", "use" | "show" | "drop") => return self.skill_names(),
                ("/session", "load" | "delete" | "diff") => {
                    return session::list(&self.paths)
                        .iter()
                        .map(|item| item.name.clone())
                        .collect();
                }
                _ => return Vec::new(),
            }
        }
        match command.as_str() {
            "/model" => self.known_models.clone(),
            "/preview" => complete_files(&self.state.workspace, &self.arg_token()),
            "/connect" => {
                let mut values: Vec<String> = command::static_arg_values("/connect")
                    .unwrap_or_default()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                values.extend(provider::PRESETS.iter().map(|preset| preset.id.to_string()));
                values.sort();
                values.dedup();
                values
            }
            _ => command::static_arg_values(command.as_str())
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }

    /// Which argument position the cursor completes: 1 for `/cmd <tok>`,
    /// 2 for `/cmd <fixed> <tok>`.
    pub(crate) fn arg_position(&self) -> usize {
        let words = self.input.split_whitespace().count();
        if self.input.ends_with(char::is_whitespace) {
            words
        } else {
            words.saturating_sub(1)
        }
    }

    /// The partial token after the last space (empty when the input ends
    /// with a space).
    pub(crate) fn arg_token(&self) -> String {
        if self.input.ends_with(char::is_whitespace) {
            return String::new();
        }
        self.input
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .to_string()
    }

    /// Sorted skill stems (`review` for `review.md`), mirroring `/skills`.
    pub(crate) fn skill_names(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(&self.state.skills_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                (entry.path().extension().and_then(|value| value.to_str()) == Some("md")).then(
                    || {
                        entry
                            .path()
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default()
                            .to_string()
                    },
                )
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    pub(crate) fn arg_menu_open(&mut self) -> bool {
        !self.arg_menu_items().is_empty()
    }

    pub(crate) fn arg_menu_active(&mut self) -> bool {
        !self.arg_cache_key.is_empty()
            && !self.arg_cache_items.is_empty()
            && format!("{}:{}", self.input, self.cursor) == self.arg_cache_key
    }

    /// Resolve the composer to bare shell text plus its marker width:
    /// explicit `!` lines first, then marker-free lines that read as
    /// shell (`git status` completes like `!git status`). `/` commands,
    /// `#` drafts, `@` refs, and multiline input never qualify — their
    /// own menus own those keystrokes.
    pub(crate) fn shell_line(&self) -> Option<(usize, String)> {
        if self.input.contains('\n') || self.at_token().is_some() {
            return None;
        }
        if let Some(rest) = self.input.strip_prefix('!') {
            return (!rest.trim().is_empty()).then(|| (1, rest.to_string()));
        }
        if self.input.starts_with(['/', '#', '@']) {
            return None;
        }
        if crate::suggest::looks_like_shell(&self.input, &self.ctx_cache.aliases) {
            return Some((0, self.input.clone()));
        }
        None
    }

    /// Shell-line Tab menu rows: history plus the context-aware
    /// candidates behind the ghost (subcommands, flags, branches,
    /// scripts, files). Tab-invoked only — the ghost is the as-you-type
    /// layer, the menu opens when Tab finds ambiguity. Input-keyed
    /// cache like the `@` and arg menus.
    pub(crate) fn sh_menu_items(&mut self) -> Vec<crate::suggest::ShellCandidate> {
        if !self.sh_menu_invoked {
            self.sh_cache_key.clear();
            self.sh_cache_items.clear();
            return Vec::new();
        }
        let key = format!("{}:{}", self.input, self.cursor);
        if key == self.sh_cache_key {
            return self.sh_cache_items.clone();
        }
        let items = self.sh_candidates_uncached();
        self.sh_cache_key = key;
        self.sh_selected = 0;
        self.sh_cache_items = items.clone();
        items
    }

    /// Raw candidate computation, independent of invocation: Tab uses
    /// it to decide between direct apply (1 row), opening (2+), and
    /// falling through to the ghost (none).
    pub(crate) fn sh_candidates_uncached(&mut self) -> Vec<crate::suggest::ShellCandidate> {
        if !matches!(self.overlay, Overlay::None)
            || self.palette_active()
            || self.cursor != self.input.len()
        {
            return Vec::new();
        }
        // Markers, then marker-free shell lines; every other line
        // belongs to the palette, the arg menu, or the model. The alias
        // table warms first so `g st` is detected as shell.
        let cwd = self.state.workspace.clone();
        let input = self.input.clone();
        self.ctx_cache.refresh_for(&input, &cwd);
        let Some((_, prefix)) = self.shell_line() else {
            return Vec::new();
        };
        self.ctx_cache.refresh_for(&prefix, &cwd);
        self.kick_live_refresh(&prefix);
        crate::suggest::shell_candidates(
            &prefix,
            &self.shell_history,
            &cwd,
            &self.ctx_cache,
            &self.bin_cache,
        )
    }

    pub(crate) fn sh_menu_open(&mut self) -> bool {
        self.sh_menu_invoked && !self.sh_menu_items().is_empty()
    }

    pub(crate) fn sh_menu_active(&mut self) -> bool {
        self.sh_menu_invoked
            && !self.sh_cache_key.is_empty()
            && !self.sh_cache_items.is_empty()
            && format!("{}:{}", self.input, self.cursor) == self.sh_cache_key
    }

    /// Tab on a shell line: certain (1 row) applies at once, ambiguous
    /// (2+) opens the menu, empty falls through to ghost/mode. False
    /// when Tab is not ours to take.
    pub(crate) fn sh_tab(&mut self) -> bool {
        if self.sh_menu_invoked {
            return false;
        }
        let items = self.sh_candidates_uncached();
        if items.len() == 1 {
            let pick = items.into_iter().next().expect("single row");
            self.apply_sh_pick(&pick);
            self.sh_selected = 0;
            return true;
        }
        if items.len() > 1 {
            self.sh_cache_key = format!("{}:{}", self.input, self.cursor);
            self.sh_selected = 0;
            self.sh_cache_items = items;
            self.sh_menu_invoked = true;
            return true;
        }
        false
    }

    /// Dismiss the invoked menu without accepting.
    pub(crate) fn close_sh_menu(&mut self) {
        self.sh_menu_invoked = false;
        self.sh_cache_key.clear();
        self.sh_cache_items.clear();
        self.sh_selected = 0;
    }

    /// Accept the selected shell row: history rows swap the whole line,
    /// token rows swap the token under the cursor. Either way typing
    /// resumes at the end, and an empty pick list is a no-op.
    pub(crate) fn accept_sh_complete(&mut self) -> bool {
        let items = self.sh_cache_items.clone();
        let Some(pick) = items.get(self.sh_selected).cloned() else {
            return false;
        };
        if !self.apply_sh_pick(&pick) {
            return false;
        }
        self.close_sh_menu();
        true
    }

    /// Swap one candidate into the composer, keeping any `!` marker
    /// (marker-free lines swap in place). False when the input stopped
    /// being a shell line.
    pub(crate) fn apply_sh_pick(&mut self, pick: &crate::suggest::ShellCandidate) -> bool {
        let Some((marker_len, prefix)) = self.shell_line() else {
            return false;
        };
        let swapped = crate::suggest::apply_shell_candidate(&prefix, pick);
        self.input = format!("{}{}", &self.input[..marker_len], swapped);
        self.cursor = self.input.len();
        self.pending_correction = None;
        true
    }
}

pub(crate) fn score_file_candidate(relative: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = relative.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(&query) {
        return Some(800 - relative.len().min(700) as i32);
    }
    let file = candidate.rsplit('/').next().unwrap_or(&candidate);
    if file.starts_with(&query) {
        return Some(700 - relative.len().min(600) as i32);
    }
    if candidate.contains(&query) {
        return Some(500 - relative.len().min(400) as i32);
    }
    // Ordered-subsequence fallback so `mrs` still finds `main.rs`.
    let mut rest = candidate.chars();
    for needle in query.chars() {
        rest.find(|value| *value == needle)?;
    }
    Some(100)
}

/// Fuzzy workspace files for `@` completion. Skips hidden entries, `.git`,
/// and build output; capped so a huge tree cannot stall a keystroke.
pub(crate) fn complete_files(workspace: &Path, query: &str) -> Vec<String> {
    let mut scored: Vec<(i32, String)> = Vec::new();
    for entry in walkdir::WalkDir::new(workspace)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .take(400)
    {
        let relative = entry.path().strip_prefix(workspace).unwrap_or(entry.path());
        if relative.as_os_str().is_empty() {
            continue;
        }
        let display = relative.display().to_string().replace('\\', "/");
        if display
            .split('/')
            .any(|part| part.starts_with('.') || part == "target" || part == "node_modules")
        {
            continue;
        }
        if let Some(score) = score_file_candidate(&display, query) {
            scored.push((score, display));
        }
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    scored.truncate(8);
    scored.into_iter().map(|(_, path)| path).collect()
}
