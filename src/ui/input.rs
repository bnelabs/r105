//! UiApp input: Keyboard/mouse input, composer editing, and submit paths.

use super::*;

impl UiApp {
    pub(crate) async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.busy {
                self.cancel_work("Stopping…");
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if self.action_key("cancel", &key) {
            self.cancel_work("Stopping…");
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P'))
        {
            self.open_palette();
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            // An open history search owns Esc: cancel and restore.
            if self.hist_open() {
                self.close_hist_search(false);
                return Ok(());
            }
            // A focused pane owns Esc first: focus returns to the
            // composer, the pane stays visible.
            if self.sidebar_focus {
                self.unfocus_sidebar();
                return Ok(());
            }
            // An open shell menu owns Esc the way the palette owns
            // typing: close it, leave the composer alone.
            if self.sh_menu_invoked {
                self.close_sh_menu();
                return Ok(());
            }
            // Esc stands down every offer, including a pending fix.
            self.pending_correction = None;
            if self.hist_depth.is_some() {
                // A history walk owns Esc: restore the draft, cancel the
                // walk, and leave any ghost/cancel handling alone.
                self.end_history_walk(false);
                self.set_status("Restored".into());
            } else if matches!(self.overlay, Overlay::Approval) {
                // Dismissing the card must decide it, or the paused
                // round would hang busy forever.
                self.resolve_approval(ApprovalVerdict::Deny);
            } else if matches!(self.overlay, Overlay::None) {
                if !self.busy && self.dismiss_ghost() {
                    return Ok(());
                }
                self.cancel_work("Stopped");
            } else {
                self.overlay = Overlay::None;
                self.input.clear();
                self.cursor = 0;
            }
            return Ok(());
        }
        if !matches!(self.overlay, Overlay::None) {
            self.handle_overlay_key(key).await?;
            return Ok(());
        }
        // Reverse history search owns the keyboard while open.
        if self.hist_open() {
            self.handle_hist_key(key);
            return Ok(());
        }
        // Tabs (Warp-style): Ctrl+Shift+T new, Ctrl+Shift+W close,
        // Ctrl+Tab / Ctrl+Shift+Tab cycle, Alt+1..9 select.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        {
            self.tab_new();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W'))
        {
            self.tab_close();
            return Ok(());
        }
        if key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.tab_next(!key.modifiers.contains(KeyModifiers::SHIFT));
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::ALT)
            && let KeyCode::Char(digit) = key.code
            && let Some(index) = digit.to_digit(10)
            && index >= 1
        {
            self.tab_switch(index as usize - 1);
            return Ok(());
        }
        if self.action_key("sidebar", &key) {
            self.toggle_sidebar();
            return Ok(());
        }
        // A focused pane owns every other key: arrows move, Enter
        // opens, letters filter, Esc (above) leaves.
        if self.sidebar_focus {
            self.handle_sidebar_key(key);
            return Ok(());
        }
        if self.action_key("details", &key) {
            self.show_details = !self.show_details;
            self.set_status(if self.show_details {
                "Details on".into()
            } else {
                "Details off".into()
            });
            return Ok(());
        }
        if self.action_key("tasks", &key) {
            let text = if self.busy {
                format!(
                    "active request; {} queued prompt(s); tool round {}",
                    self.queue.len(),
                    self.tool_round
                )
            } else {
                format!("idle; {} queued prompt(s)", self.queue.len())
            };
            self.push_system(&text);
            return Ok(());
        }
        if self.action_key("history", &key) {
            self.open_hist_search();
            return Ok(());
        }
        if self.action_key("redraw", &key) {
            self.transcript_scroll = 0;
            self.follow_transcript = true;
            self.set_status("Top".into());
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('a') | KeyCode::Char('A'))
            && matches!(self.overlay, Overlay::None)
        {
            self.cursor = 0;
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('e') | KeyCode::Char('E'))
        {
            self.cursor = self.input.len();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('K'))
        {
            self.delete_to_end();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('u') | KeyCode::Char('U'))
        {
            self.delete_to_start();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('w') | KeyCode::Char('W'))
        {
            self.delete_prev_word();
            return Ok(());
        }
        if key.code == KeyCode::Tab && self.at_menu_open() {
            self.accept_at_complete();
            return Ok(());
        }
        if key.code == KeyCode::Tab && self.arg_menu_open() {
            self.accept_arg_complete();
            return Ok(());
        }
        if key.code == KeyCode::Tab && self.sh_menu_open() {
            self.accept_sh_complete();
            return Ok(());
        }
        // Shell Tab decides: certain applies, ambiguous opens the
        // menu, empty falls through to ghost/mode cycling below.
        if key.code == KeyCode::Tab && self.sh_tab() {
            return Ok(());
        }
        // A visible ghost wins over the mode cycle: Tab already means
        // "complete" everywhere else in the composer.
        if key.code == KeyCode::Tab && self.cursor == self.input.len() && self.ghost_text.is_some()
        {
            self.accept_ghost();
            return Ok(());
        }
        if key.code == KeyCode::Tab {
            let next = match self.mode {
                Mode::Build => Mode::Plan,
                Mode::Plan => Mode::Ask,
                Mode::Ask => Mode::Build,
            };
            self.set_mode(next);
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Up | KeyCode::Down)
        {
            let step = page_step(self.transcript_height).clamp(4, 10);
            if key.code == KeyCode::Up {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(step);
            } else {
                self.transcript_scroll = self.transcript_scroll.saturating_add(step);
            }
            self.follow_transcript = false;
            return Ok(());
        }
        if self.at_menu_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.at_cache_items.len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.at_selected = self.at_selected.saturating_sub(1);
                } else {
                    self.at_selected = (self.at_selected + 1).min(count - 1);
                }
            }
            return Ok(());
        }
        if self.arg_menu_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.arg_cache_items.len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.arg_selected = self.arg_selected.saturating_sub(1);
                } else {
                    self.arg_selected = (self.arg_selected + 1).min(count - 1);
                }
            }
            return Ok(());
        }
        if self.sh_menu_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.sh_cache_items.len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.sh_selected = self.sh_selected.saturating_sub(1);
                } else {
                    self.sh_selected = (self.sh_selected + 1).min(count - 1);
                }
            }
            return Ok(());
        }
        if self.palette_active() && matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let count = self.palette_items().len();
            if count > 0 {
                if key.code == KeyCode::Up {
                    self.palette_selected = self.palette_selected.saturating_sub(1);
                } else {
                    self.palette_selected = (self.palette_selected + 1).min(count - 1);
                }
                self.palette_scroll =
                    command::ensure_visible(self.palette_selected, self.palette_scroll, 7, count);
            }
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Home) {
            self.jump_top();
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::End) {
            self.jump_bottom();
            return Ok(());
        }
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.insert_text("\n");
            }
            KeyCode::Enter => self.submit().await?,
            KeyCode::Char(character) => {
                if key.modifiers.contains(KeyModifiers::ALT) {
                    match character {
                        'f' | 'F' => {
                            self.cursor = next_word_boundary(&self.input, self.cursor);
                            return Ok(());
                        }
                        'b' | 'B' => {
                            self.cursor = prev_word_boundary(&self.input, self.cursor);
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                self.insert_paired(character);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.cursor = prev_word_boundary(&self.input, self.cursor);
                } else {
                    self.cursor = previous_boundary(&self.input, self.cursor);
                }
            }
            KeyCode::Right => {
                // A pending fix owns an empty composer: one keystroke
                // takes it, whatever the modifiers.
                if self.input.trim().is_empty() && self.apply_correction() {
                    return Ok(());
                }
                // At the end of input a visible ghost owns `→`: plain
                // takes it all, Ctrl takes one word; Alt stays a word
                // jump and mid-line stays cursor movement.
                let at_end = self.cursor == self.input.len();
                if at_end && self.ghost_text.is_some() && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        self.accept_ghost_word();
                    } else {
                        self.accept_ghost();
                    }
                } else if key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.cursor = next_word_boundary(&self.input, self.cursor);
                } else {
                    self.cursor = next_boundary(&self.input, self.cursor);
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::PageUp => self.page_up(),
            KeyCode::PageDown => self.page_down(),
            KeyCode::Up => self.history_previous(),
            KeyCode::Down => self.history_next(),
            _ => {}
        }
        Ok(())
    }

    pub(crate) async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        let current = std::mem::replace(&mut self.overlay, Overlay::None);
        match current {
            Overlay::Providers {
                mut selected,
                mut scroll,
            } => {
                let count = provider::PRESETS.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(6),
                    KeyCode::PageDown => selected = (selected + 6).min(count.saturating_sub(1)),
                    KeyCode::Enter => {
                        let preset = provider::PRESETS[selected];
                        let id = preset.id.to_string();
                        if preset.asks_for_url {
                            self.open_url_overlay(id);
                            keep = false;
                        } else if preset.api_key_required
                            && preset
                                .api_key_env
                                .and_then(|name| std::env::var(name).ok())
                                .is_none_or(|value| value.trim().is_empty())
                        {
                            self.input.clear();
                            self.cursor = 0;
                            self.overlay = Overlay::ApiKey { provider: id };
                            keep = false;
                        } else {
                            self.start_provider(id, None);
                            keep = false;
                        }
                    }
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    scroll = command::ensure_visible(selected, scroll, 8, count);
                    self.overlay = Overlay::Providers { selected, scroll };
                }
            }
            Overlay::Models {
                items,
                mut selected,
                mut scroll,
                active,
            } => {
                let count = items.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(count.saturating_sub(1)),
                    KeyCode::PageUp => selected = selected.saturating_sub(8),
                    KeyCode::PageDown => selected = (selected + 8).min(count.saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some(model) = items.get(selected) {
                            self.state.model = model.id.clone();
                            let model = model.id.clone();
                            self.persist_connection(Some(&model));
                            self.set_ok(format!("Model: {model}"));
                        }
                        keep = false;
                    }
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    scroll = command::ensure_visible(selected, scroll, 10, count);
                    self.overlay = Overlay::Models {
                        items,
                        selected,
                        scroll,
                        active,
                    };
                }
            }
            Overlay::ApiKey { provider } => match key.code {
                KeyCode::Enter => {
                    let value = self.input.trim().to_string();
                    self.input.clear();
                    self.cursor = 0;
                    if value.is_empty() {
                        self.set_error("API key was not entered".into());
                    } else {
                        self.start_provider(provider, Some(value));
                    }
                }
                KeyCode::Char(character) => {
                    self.insert_text(&character.to_string());
                    self.overlay = Overlay::ApiKey { provider };
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.overlay = Overlay::ApiKey { provider };
                }
                _ => self.overlay = Overlay::ApiKey { provider },
            },
            Overlay::CustomUrl { provider } => match key.code {
                KeyCode::Enter => {
                    let value = if self.input.trim().is_empty() {
                        provider::preset(&provider)
                            .and_then(|preset| preset.base_url)
                            .unwrap_or_default()
                            .to_string()
                    } else {
                        self.input.trim().to_string()
                    };
                    self.input.clear();
                    self.cursor = 0;
                    if provider::valid_url(&value) {
                        self.start_provider(provider, Some(value));
                    } else {
                        self.set_status(
                            "Enter an http:// or https:// URL without credentials".into(),
                        );
                        self.overlay = Overlay::CustomUrl { provider };
                    }
                }
                KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input.clear();
                    self.cursor = 0;
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Char(character) => {
                    self.insert_text(&character.to_string());
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Backspace => {
                    self.backspace();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Delete => {
                    self.delete();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Left => {
                    self.cursor = previous_boundary(&self.input, self.cursor);
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Right => {
                    self.cursor = next_boundary(&self.input, self.cursor);
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::Home => {
                    self.cursor = 0;
                    self.overlay = Overlay::CustomUrl { provider };
                }
                KeyCode::End => {
                    self.cursor = self.input.len();
                    self.overlay = Overlay::CustomUrl { provider };
                }
                _ => self.overlay = Overlay::CustomUrl { provider },
            },
            Overlay::Theme {
                mut selected,
                original,
            } => {
                let count = THEMES.len();
                let mut keep = true;
                match key.code {
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        self.state.theme = THEMES[selected].to_string();
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(count.saturating_sub(1));
                        self.state.theme = THEMES[selected].to_string();
                    }
                    KeyCode::Enter => {
                        let theme = THEMES[selected].to_string();
                        self.state.theme = theme.clone();
                        self.persist_config(|config| config.theme = theme.clone());
                        self.set_ok(format!("Theme: {theme}"));
                        keep = false;
                    }
                    KeyCode::Esc => {
                        // Revert the live preview.
                        self.state.theme = original.clone();
                        keep = false;
                    }
                    _ => {}
                }
                if keep {
                    self.overlay = Overlay::Theme { selected, original };
                }
            }
            Overlay::Settings { mut selected } => {
                const COUNT: usize = 7;
                let mut keep = true;
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(COUNT - 1),
                    KeyCode::Left => self.cycle_setting(selected, -1),
                    KeyCode::Right | KeyCode::Enter => self.cycle_setting(selected, 1),
                    KeyCode::Esc => keep = false,
                    _ => {}
                }
                if keep {
                    self.overlay = Overlay::Settings { selected };
                }
            }
            Overlay::Approval => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.resolve_approval(ApprovalVerdict::Once);
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    self.resolve_approval(ApprovalVerdict::Always);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.resolve_approval(ApprovalVerdict::Deny);
                }
                // Any other key keeps the card: an undecided round must
                // not leak back into the composer.
                _ => self.overlay = Overlay::Approval,
            },
            Overlay::None => {}
        }
        Ok(())
    }

    pub(crate) async fn submit(&mut self) -> Result<()> {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        self.pending_correction = None;
        self.close_sh_menu();
        self.end_history_walk(true);
        // `#` is natural language for the shell (Warp's command search):
        // ask the model for one command and leave it in the composer for
        // review. Nothing runs until the next Enter.
        if let Some(goal) = value.strip_prefix('#') {
            let goal = goal.trim().to_string();
            self.input.clear();
            self.cursor = 0;
            self.at_cache_key.clear();
            self.arg_cache_key.clear();
            if goal.is_empty() {
                self.set_status("Usage: # describe what you want to do".into());
                return Ok(());
            }
            self.command_shell_draft(&[goal]);
            return Ok(());
        }
        if self.palette_active()
            && command::command(&value).is_none()
            && !self.is_custom_command(&value)
        {
            if let Some(item) = self.palette_items().get(self.palette_selected) {
                self.input = format!("{} ", item.name);
                self.cursor = self.input.len();
            }
            return Ok(());
        }
        self.input.clear();
        self.cursor = 0;
        self.at_cache_key.clear();
        self.arg_cache_key.clear();
        if let Some(shell) = value.strip_prefix('!') {
            let shell = shell.trim().to_string();
            if shell.is_empty() {
                self.set_status("Usage: !<command>".into());
                return Ok(());
            }
            self.submit_shell(value, shell);
            return Ok(());
        }
        // Warp-style Enter: a line that reads as a shell command runs
        // it. The detection is the same one behind completion, so prose
        // and questions still reach the model; `#` forces the router
        // and a trailing `?` stays a prompt.
        let cwd = self.state.workspace.clone();
        self.ctx_cache.refresh_for(&value, &cwd);
        if crate::suggest::looks_like_shell(&value, &self.ctx_cache.aliases) {
            self.submit_shell(value.clone(), value);
            return Ok(());
        }
        if let Some(parsed) = command::parse(&value) {
            self.handle_command(parsed).await
        } else {
            self.submit_prompt(value);
            Ok(())
        }
    }

    /// Shared shell entry for `!` lines and Warp-style bare commands:
    /// `cd <dir>` moves the workspace (a sandboxed subshell cannot keep
    /// a directory change), everything else runs in the sandbox and its
    /// output joins the conversation.
    pub(crate) fn submit_shell(&mut self, display: String, command: String) {
        if self.try_cd(&command) {
            return;
        }
        self.redo_stack.clear();
        self.state.history.push(Message::user(display));
        self.follow_transcript = true;
        // Frequency layer: every run teaches the ghost.
        self.record_shell(&command);
        self.run_shell_command(command);
    }

    /// Bare `cd <dir>` retargets the workspace, terminal-style. Returns
    /// false for compound lines and non-`cd` commands, which run in the
    /// shell as usual.
    fn try_cd(&mut self, command: &str) -> bool {
        let mut words = command.split_whitespace();
        if words.next() != Some("cd") {
            return false;
        }
        let Some(target) = words.next() else {
            return false;
        };
        if words.next().is_some() {
            return false;
        }
        let path = PathBuf::from(target).expanduser();
        let resolved = if path.is_absolute() {
            path
        } else {
            self.state.workspace.join(path)
        };
        let Ok(canonical) = std::fs::canonicalize(&resolved) else {
            self.set_error(format!("cd: no such directory: {target}"));
            return true;
        };
        if !canonical.is_dir() {
            self.set_error(format!("cd: not a directory: {target}"));
            return true;
        }
        self.state.workspace = canonical;
        self.refresh_git_branch();
        self.refresh_custom_commands();
        let workspace = self.state.workspace.clone();
        self.note_workspace(&workspace);
        self.refresh_sidebar();
        self.set_ok(format!("Workspace: {}", self.state.workspace.display()));
        true
    }

    /// Shared prompt entry: resolves `@file` references into attached
    /// context, then steers the active request or starts a new one.
    pub(crate) fn submit_prompt(&mut self, value: String) {
        self.redo_stack.clear();
        let mut blocks = Vec::new();
        let mut unknown = Vec::new();
        for path in extract_file_refs(&value) {
            match self.resolve_file_ref(&path) {
                Some(block) => blocks.push(block),
                None => unknown.push(path),
            }
        }
        if !unknown.is_empty() {
            self.set_error(format!("Unknown @refs: {}", unknown.join(", ")));
        }
        let context = if blocks.is_empty() {
            None
        } else {
            Some(blocks.join("\n"))
        };
        if self.busy {
            // Steer: cancel the in-flight request and jump the queue. The
            // cancelled task errors out, drops its restore via
            // `drop_next_restore`, and the steered prompt runs next.
            if let Some(token) = &self.cancellation {
                token.cancel();
            }
            self.drop_next_restore = true;
            self.queue.push_front((value, context));
            self.set_status(format!("Steering… ({} behind)", self.queue.len() - 1));
            return;
        }
        self.start_prompt(value, context);
    }

    /// Resolve one `@path` token against the workspace. Files are inlined
    /// (bounded), directories become listings; anything else is `None` so
    /// the caller can warn and leave the token literal.
    pub(crate) fn resolve_file_ref(&self, path: &str) -> Option<String> {
        let resolved = safe_path(&self.state.workspace, path).ok()?;
        if resolved.is_file() {
            let content = std::fs::read_to_string(&resolved).ok()?;
            const LIMIT: usize = 12_000;
            let over = content.chars().count() > LIMIT;
            let body: String = content.chars().take(LIMIT).collect();
            Some(format!(
                "<file path=\"{path}\">\n{body}{}\n</file>",
                if over { "\n⋯ truncated" } else { "" }
            ))
        } else if resolved.is_dir() {
            let mut entries: Vec<String> = std::fs::read_dir(&resolved)
                .ok()?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    entry.file_name().into_string().ok().map(|name| {
                        format!("{}{}", name, if entry.path().is_dir() { "/" } else { "" })
                    })
                })
                .take(40)
                .collect();
            entries.sort();
            Some(format!(
                "<directory path=\"{path}\">\n{}\n</directory>",
                entries.join("\n")
            ))
        } else {
            None
        }
    }

    /// Run a `!` shell line inside the sandbox boundary (timeouts,
    /// cancellation, workspace confinement) and attach its output to the
    /// conversation so the next turn can use it.
    pub(crate) fn run_shell_command(&mut self, command: String) {
        if self.state.permission_posture == "off" {
            self.set_error("Shell disabled (permissions off)".into());
            return;
        }
        let workspace = self.state.workspace.clone();
        let sandbox = self.sandbox.clone();
        let allow_network =
            self.state.permission_posture != "restricted" && self.state.permission_posture != "off";
        let cancellation = self.cancellation.clone().unwrap_or_default();
        let sender = self.tx.clone();
        // The correction worker needs the same context the ghost layers
        // use: the raw line, the workspace, and the PATH binaries.
        let bins = self.bin_cache.clone();
        let correction_cwd = workspace.clone();
        let preview: String = command.chars().take(60).collect();
        self.set_status(format!("Running: {preview}"));
        tokio::spawn(async move {
            let (program, args) = if cfg!(windows) {
                ("cmd", vec!["/C".to_string(), command.clone()])
            } else {
                ("sh", vec!["-c".to_string(), command.clone()])
            };
            let notice = match sandbox
                .run(program, &args, &workspace, allow_network, &cancellation)
                .await
            {
                Ok(output) => {
                    // A non-zero exit with rule matches becomes a
                    // one-keystroke offer for the best fix; alternates
                    // ride along in the notice and the status.
                    let mut fixes = Vec::new();
                    if output.status.is_some_and(|code| code != 0) {
                        fixes = crate::suggest::suggest_corrections(
                            &command,
                            &output.stderr,
                            &correction_cwd,
                            &bins,
                        );
                    }
                    let mut body = String::new();
                    let stdout = output.stdout.trim_end();
                    if !stdout.is_empty() {
                        body.push_str(stdout);
                    }
                    let stderr = output.stderr.trim_end();
                    if !stderr.is_empty() {
                        if !body.is_empty() {
                            body.push('\n');
                        }
                        body.push_str("stderr:\n");
                        body.push_str(stderr);
                    }
                    if body.is_empty() {
                        body.push_str("(no output)");
                    }
                    const LIMIT: usize = 8_000;
                    let over = body.chars().count() > LIMIT;
                    let shown: String = body.chars().take(LIMIT).collect();
                    let mut notice = format!(
                        "$ {command}\n{shown}{}",
                        if over { "\n⋯ output truncated" } else { "" }
                    );
                    if let Some((best, rest)) = fixes.split_first() {
                        let mut hint = format!("Did you mean `!{best}`? → applies");
                        for alt in rest {
                            hint.push_str(&format!(" · `!{alt}`"));
                        }
                        notice.push('\n');
                        notice.push_str(&hint);
                        let _ = sender.send(crate::ui::events::UiEvent::ShellCorrection {
                            failed: command.clone(),
                            fixed: best.clone(),
                            more: rest.to_vec(),
                        });
                    }
                    notice
                }
                Err(error) => format!("$ {command}\nfailed: {error:#}"),
            };
            let _ = sender.send(crate::ui::events::UiEvent::Notice(notice));
        });
    }

    /// Suspend the TUI, run `$EDITOR` on the current draft, and resume with
    /// whatever it left behind.
    pub(crate) async fn run_editor(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> Result<()> {
        let editor = std::env::var("EDITOR").unwrap_or_default();
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            anyhow::bail!("set $EDITOR to compose prompts externally (e.g. EDITOR=nvim)");
        };
        let program = program.to_string();
        let rest: Vec<String> = parts.map(str::to_string).collect();
        let path = std::env::temp_dir().join(format!("r105-editor-{}.md", std::process::id()));
        std::fs::write(&path, &self.input).context("writing editor draft")?;
        restore_terminal(terminal).context("suspending TUI for editor")?;
        let edited = tokio::process::Command::new(&program)
            .args(&rest)
            .arg(&path)
            .status()
            .await;
        let setup = setup_terminal(self.mouse_enabled);
        *terminal = setup.context("restoring TUI after editor")?;
        edited.context("running editor")?;
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let _ = std::fs::remove_file(&path);
        self.input = content.trim_end_matches('\n').to_string();
        self.cursor = self.input.len();
        self.at_cache_key.clear();
        self.set_status(format!(
            "Draft from {program} ({} chars)",
            self.input.chars().count()
        ));
        Ok(())
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        let over_sidebar = self.sidebar_hit(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.hist_open() {
                    self.hist_wheel(false);
                    return;
                }
                if over_sidebar {
                    // The draw step pins scroll to the selection, so
                    // wheel over the pane moves the selection itself.
                    self.sidebar_selected = self.sidebar_selected.saturating_sub(3);
                    self.clamp_sidebar_selected();
                    return;
                }
                let mut step = 4usize;
                if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    step = 12;
                }
                self.transcript_scroll = self.transcript_scroll.saturating_sub(step);
                self.follow_transcript = false;
            }
            MouseEventKind::ScrollDown => {
                if self.hist_open() {
                    self.hist_wheel(true);
                    return;
                }
                if over_sidebar {
                    self.sidebar_selected = self.sidebar_selected.saturating_add(3);
                    self.clamp_sidebar_selected();
                    return;
                }
                let mut step = 4usize;
                if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                    step = 12;
                }
                self.transcript_scroll = self.transcript_scroll.saturating_add(step);
                self.follow_transcript = false;
            }
            MouseEventKind::Down(_) => {
                if !matches!(self.overlay, Overlay::None) {
                    return;
                }
                let col = mouse.column;
                let row = mouse.row;
                if self.hist_open() {
                    self.click_hist_search(col, row);
                    return;
                }
                if self.tab_plus_hit(col, row) {
                    self.tab_new();
                    return;
                }
                if let Some(index) = self.tab_hit(col, row) {
                    self.tab_switch(index);
                    return;
                }
                if self.sidebar_hit(col, row) {
                    self.sidebar_click(col, row);
                    return;
                }
                if self.click_palette(col, row) {
                    return;
                }
                if self.click_sh_complete(col, row) {
                    return;
                }
                if self.click_composer(col, row) {
                    return;
                }
                self.click_transcript(col, row);
            }
            _ => {}
        }
    }

    fn click_palette(&mut self, col: u16, row: u16) -> bool {
        let Some(rect) = self.last_palette_rect else {
            return false;
        };
        if col < rect.x
            || col >= rect.x + rect.width
            || row < rect.y
            || row >= rect.y + rect.height
            || self.last_palette_count == 0
        {
            return false;
        }
        let viewport = rect.height.saturating_sub(2) as usize;
        let offset = (row.saturating_sub(rect.y).saturating_sub(1)) as usize;
        if offset >= viewport {
            return false;
        }
        let index = self.palette_scroll + offset;
        if index >= self.last_palette_count {
            return false;
        }
        if self.palette_selected == index {
            let items = self.palette_items();
            if let Some(item) = items.get(index) {
                self.input = format!("{} ", item.name);
                self.cursor = self.input.len();
                self.palette_selected = 0;
                self.palette_scroll = 0;
            }
        } else {
            self.palette_selected = index;
        }
        true
    }

    /// Shell menu clicks mirror the palette: first click selects,
    /// second click fills. Viewport math matches `draw_sh_complete`.
    fn click_sh_complete(&mut self, col: u16, row: u16) -> bool {
        let Some(rect) = self.last_sh_rect else {
            return false;
        };
        if col < rect.x
            || col >= rect.x + rect.width
            || row < rect.y
            || row >= rect.y + rect.height
            || self.last_sh_count == 0
        {
            return false;
        }
        let viewport = rect.height.saturating_sub(2) as usize;
        let offset = (row.saturating_sub(rect.y).saturating_sub(1)) as usize;
        if offset >= viewport {
            return false;
        }
        // Same scroll math as `draw_sh_complete`, recomputed: the draw
        // pins the window to the selection from the top.
        let scroll = command::ensure_visible(self.sh_selected, 0, viewport, self.last_sh_count);
        let index = (scroll + offset).min(self.last_sh_count - 1);
        if self.sh_selected == index {
            self.accept_sh_complete();
            self.sh_selected = 0;
        } else {
            self.sh_selected = index;
        }
        true
    }

    fn click_composer(&mut self, col: u16, row: u16) -> bool {
        let rect = self.last_composer_rect;
        if rect.width == 0
            || col < rect.x
            || col >= rect.x + rect.width
            || row < rect.y
            || row >= rect.y + rect.height
        {
            return false;
        }
        // Borderless composer: row 0 is the rule, so the text starts at
        // column `x` on rows `y + 1` onward.
        let inner_w = rect.width.max(1) as usize;
        let rx = col.saturating_sub(rect.x) as usize;
        let ry = row.saturating_sub(rect.y).saturating_sub(1) as usize;
        // Visual offset of the click inside the wrapped "> input" text.
        let target = ry.saturating_mul(inner_w).saturating_add(rx);
        // Walk the rendered text ("> " prefix + input with wrapping and
        // newlines) and stop at the byte index closest to the click.
        let rendered = format!("> {}", self.input);
        let mut visual = 0usize;
        let mut byte = 0usize;
        for (offset, ch) in rendered.char_indices() {
            if visual >= target {
                byte = offset;
                break;
            }
            byte = offset + ch.len_utf8();
            if ch == '\n' {
                visual = (visual / inner_w + 1) * inner_w;
            } else {
                visual += 1;
            }
        }
        // Strip the "> " prefix; clamp to a char boundary.
        let index = byte.saturating_sub(2).min(self.input.len());
        let mut snapped = index;
        while snapped > 0 && !self.input.is_char_boundary(snapped) {
            snapped -= 1;
        }
        self.cursor = snapped;
        self.end_history_walk(true);
        true
    }

    fn click_transcript(&mut self, col: u16, row: u16) {
        let rect = self.last_transcript_rect;
        if rect.width == 0
            || col < rect.x
            || col >= rect.x + rect.width
            || row < rect.y
            || row >= rect.y + rect.height
        {
            return;
        }
        let line = self.last_transcript_scroll + (row.saturating_sub(rect.y)) as usize;
        let id = self
            .transcript_header_rows
            .get(line)
            .and_then(|entry| entry.clone());
        let Some(id) = id else {
            return;
        };
        let default = self
            .section_order
            .iter()
            .find(|(sid, _)| *sid == id)
            .map(|(_, default)| *default)
            .unwrap_or(false);
        if let Some(next) = self.toggle_section(&id, default, "Section") {
            self.set_status(format!(
                "Section {}",
                if next { "expanded" } else { "collapsed" }
            ));
        }
    }

    /// Remappable Ctrl actions. `keybindings` maps action names (`cancel`,
    /// `details`, `tasks`, `history`, `redraw`) to `ctrl+<letter>`; anything
    /// else falls back to the built-in default from `KEY_ACTIONS`.
    pub(crate) fn action_key(&self, action: &str, key: &KeyEvent) -> bool {
        let default = KEY_ACTIONS
            .iter()
            .find(|(name, _)| *name == action)
            .map(|(_, shortcut)| *shortcut);
        match (self.state.keybindings.get(action), default) {
            (Some(spec), _) => match_ctrl_spec(spec, key),
            (None, Some(shortcut)) => {
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(shortcut)
            }
            (None, None) => false,
        }
    }

    pub(crate) fn insert_text(&mut self, value: &str) {
        self.end_history_walk(true);
        self.pending_correction = None;
        self.input.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    /// Take the pending shell fix into an empty composer. False when
    /// there is nothing to take.
    pub(crate) fn apply_correction(&mut self) -> bool {
        let Some(correction) = self.pending_correction.take() else {
            return false;
        };
        self.end_history_walk(true);
        self.input = format!("!{}", correction.fixed);
        self.cursor = self.input.len();
        self.set_status(format!("From `{}` — Enter runs", correction.failed));
        true
    }

    /// IDE-style paired input: typing an opener inserts its closer and
    /// steps inside; typing a closer over itself steps over it.
    pub(crate) fn insert_paired(&mut self, character: char) {
        self.pending_correction = None;
        let closer = match character {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = closer {
            let next = self.input[self.cursor..].chars().next();
            if self.cursor == self.input.len()
                || next
                    .is_some_and(|c| c.is_whitespace() || matches!(c, ')' | ']' | '}' | '"' | '\''))
            {
                self.end_history_walk(true);
                self.input.insert(self.cursor, character);
                self.input.insert(self.cursor + 1, close);
                self.cursor += character.len_utf8();
                return;
            }
        }
        if matches!(character, ')' | ']' | '}' | '"' | '\'')
            && self.input[self.cursor..].starts_with(character)
        {
            self.end_history_walk(true);
            self.cursor += character.len_utf8();
            return;
        }
        if matches!(character, '"' | '\'') {
            let next = self.input[self.cursor..].chars().next();
            if self.cursor == self.input.len()
                || next.is_some_and(|c| c.is_whitespace() || matches!(c, ')' | ']' | '}'))
            {
                self.end_history_walk(true);
                self.input.insert(self.cursor, character);
                self.input.insert(self.cursor + 1, character);
                self.cursor += character.len_utf8();
                return;
            }
        }
        self.insert_text(&character.to_string());
    }

    pub(crate) fn delete_prev_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.pending_correction = None;
        self.end_history_walk(true);
        let start = prev_word_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn delete_to_start(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.pending_correction = None;
        self.end_history_walk(true);
        self.input.drain(..self.cursor);
        self.cursor = 0;
    }

    pub(crate) fn delete_to_end(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        self.pending_correction = None;
        self.end_history_walk(true);
        self.input.truncate(self.cursor);
    }

    pub(crate) fn open_palette(&mut self) {
        self.pending_correction = None;
        if self.input.starts_with('/')
            && self.input.len() <= 24
            && !self.input.contains([' ', '\n'])
        {
            self.input.clear();
            self.cursor = 0;
            self.palette_selected = 0;
        } else {
            self.input = "/".to_string();
            self.cursor = 1;
            self.palette_selected = 0;
            self.palette_scroll = 0;
            self.set_status("Palette — type to filter · Enter opens".into());
        }
    }

    pub(crate) fn page_up(&mut self) {
        let step = page_step(self.transcript_height);
        self.transcript_scroll = self.transcript_scroll.saturating_sub(step);
        self.follow_transcript = false;
    }

    pub(crate) fn page_down(&mut self) {
        let step = page_step(self.transcript_height);
        self.transcript_scroll = self.transcript_scroll.saturating_add(step);
        self.follow_transcript = false;
    }

    pub(crate) fn jump_top(&mut self) {
        self.transcript_scroll = 0;
        self.follow_transcript = false;
        self.set_status("Top".into());
    }

    pub(crate) fn jump_bottom(&mut self) {
        self.follow_transcript = true;
        self.set_status("Latest".into());
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.pending_correction = None;
        self.end_history_walk(true);
        let start = previous_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        self.pending_correction = None;
        self.end_history_walk(true);
        let end = next_boundary(&self.input, self.cursor);
        self.input.drain(self.cursor..end);
    }

    pub(crate) fn history_previous(&mut self) {
        let history = self.user_history();
        if history.is_empty() {
            return;
        }
        self.pending_correction = None;
        let depth = match self.hist_depth {
            None => {
                // First ↑ stashes the live draft: ↓ past the newest
                // restores it, like a shell.
                self.draft_stash = self.input.clone();
                1
            }
            Some(depth) => (depth + 1).min(history.len()),
        };
        self.hist_depth = Some(depth);
        self.input = history[history.len() - depth].clone();
        self.cursor = self.input.len();
        self.ghost_text = None;
    }

    pub(crate) fn history_next(&mut self) {
        let Some(depth) = self.hist_depth else {
            return;
        };
        self.pending_correction = None;
        let history = self.user_history();
        if depth > 1 {
            let next = depth - 1;
            self.hist_depth = Some(next);
            self.input = history[history.len() - next].clone();
        } else {
            self.hist_depth = None;
            self.input = std::mem::take(&mut self.draft_stash);
        }
        self.cursor = self.input.len();
        self.ghost_text = None;
    }

    /// End the ↑/↓ walk. `adopt` keeps the previewed text as the new
    /// draft (any real edit, submit, or ghost accept means the user took
    /// it); `false` restores the stash (Esc cancels the walk).
    pub(crate) fn end_history_walk(&mut self, adopt: bool) {
        if self.hist_depth.take().is_none() {
            return;
        }
        if adopt {
            self.draft_stash.clear();
        } else {
            self.input = std::mem::take(&mut self.draft_stash);
            self.cursor = self.input.len();
        }
    }

    fn user_history(&self) -> Vec<String> {
        self.state
            .history
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.content.clone())
            .collect()
    }

    /// Replace the `@token` before the cursor with the selected pick.
    /// Directories keep a trailing `/` so completion can continue; files
    /// take a trailing space so typing resumes naturally.
    pub(crate) fn accept_at_complete(&mut self) -> bool {
        let items = self.at_cache_items.clone();
        let Some(pick) = items.get(self.at_selected).cloned() else {
            return false;
        };
        self.pending_correction = None;
        let end = self.cursor.min(self.input.len());
        let before = self.input[..end].to_string();
        let Some(at) = before.rfind('@') else {
            return false;
        };
        let suffix = if self.state.workspace.join(&pick).is_dir() {
            "/"
        } else {
            " "
        };
        let replacement = format!("@{pick}{suffix}");
        self.input.replace_range(at..end, &replacement);
        self.cursor = at + replacement.len();
        self.at_cache_key.clear();
        self.at_cache_items.clear();
        self.at_selected = 0;
        true
    }

    /// Replace the partial argument after the last space with the selected
    /// pick plus a trailing space so typing resumes naturally.
    pub(crate) fn accept_arg_complete(&mut self) -> bool {
        self.pending_correction = None;
        let items = self.arg_cache_items.clone();
        let Some(pick) = items.get(self.arg_selected).cloned() else {
            return false;
        };
        let Some(space) = self.input.rfind(' ') else {
            return false;
        };
        self.input.replace_range(space + 1.., &format!("{pick} "));
        self.cursor = self.input.len();
        self.arg_cache_key.clear();
        self.arg_cache_items.clear();
        self.arg_selected = 0;
        true
    }
}

/// Remappable composer actions and their built-in defaults. Only
/// `ctrl+<letter>` specs are accepted: predictable to parse, hard to typo
/// into something destructive. Ctrl+C, Esc, Tab, and Enter stay fixed.
pub(crate) const KEY_ACTIONS: [(&str, char); 6] = [
    ("cancel", 'x'),
    ("details", 'o'),
    ("tasks", 't'),
    ("history", 'r'),
    ("redraw", 'l'),
    ("sidebar", 'b'),
];

pub(crate) fn is_known_key_action(action: &str) -> bool {
    KEY_ACTIONS.iter().any(|(name, _)| *name == action)
}

pub(crate) fn ctrl_spec_letter(spec: &str) -> Option<char> {
    let rest = spec.trim().to_ascii_lowercase();
    let letter = rest.strip_prefix("ctrl+")?;
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(first), None) if first.is_ascii_alphabetic() => Some(first),
        _ => None,
    }
}

pub(crate) fn valid_ctrl_spec(spec: &str) -> bool {
    ctrl_spec_letter(spec).is_some()
}

pub(crate) fn match_ctrl_spec(spec: &str, key: &KeyEvent) -> bool {
    let Some(letter) = ctrl_spec_letter(spec) else {
        return false;
    };
    key.modifiers == KeyModifiers::CONTROL
        && matches!(key.code, KeyCode::Char(found) if found.to_ascii_lowercase() == letter)
}

pub(crate) fn is_path_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '_' | '/' | '.' | '-' | '~' | '\\' | '+')
}

/// `@path` tokens in a prompt, in order and deduplicated. The `@` must open
/// the line or follow whitespace so `user@host` never counts as a file.
pub(crate) fn extract_file_refs(input: &str) -> Vec<String> {
    let bytes = input.as_bytes();
    let mut refs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'@' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) {
            let mut end = index + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric()
                    || matches!(bytes[end], b'_' | b'/' | b'.' | b'-' | b'~' | b'\\' | b'+'))
            {
                end += 1;
            }
            if end > index + 1 {
                let token = &input[index + 1..end];
                if !refs.iter().any(|item| item == token) {
                    refs.push(token.to_string());
                }
            }
            index = end;
        } else {
            index += 1;
        }
    }
    refs
}

pub(crate) fn previous_boundary(input: &str, cursor: usize) -> usize {
    input[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(crate) fn next_boundary(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .chars()
        .next()
        .map(|value| cursor + value.len_utf8())
        .unwrap_or(cursor)
}

fn is_word_char(value: char) -> bool {
    value.is_alphanumeric() || value == '_'
}

pub(crate) fn prev_word_boundary(input: &str, cursor: usize) -> usize {
    let bytes = input.as_bytes();
    let mut index = cursor.min(bytes.len());
    while index > 0 && !is_word_char(input[..index].chars().next_back().unwrap_or(' ')) {
        index = previous_boundary(input, index);
    }
    while index > 0 && is_word_char(input[..index].chars().next_back().unwrap_or(' ')) {
        index = previous_boundary(input, index);
    }
    index
}

pub(crate) fn next_word_boundary(input: &str, cursor: usize) -> usize {
    let len = input.len();
    let mut index = cursor.min(len);
    while index < len && !is_word_char(input[index..].chars().next().unwrap_or(' ')) {
        index = next_boundary(input, index);
    }
    while index < len && is_word_char(input[index..].chars().next().unwrap_or(' ')) {
        index = next_boundary(input, index);
    }
    index
}

pub(crate) fn page_step(height: u16) -> usize {
    (height as usize).saturating_sub(2).max(3)
}
