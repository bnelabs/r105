//! UiApp input: Keyboard/mouse input, composer editing, and submit paths.

use super::*;

impl UiApp {
    pub(crate) async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.busy {
                self.cancel_work("Cancelling…");
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if self.action_key("cancel", &key) {
            self.cancel_work("Cancelling current work…");
            return Ok(());
        }
        if key.code == KeyCode::Esc {
            if matches!(self.overlay, Overlay::None) {
                self.cancel_work("Cancelled");
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
        if self.action_key("details", &key) {
            self.show_details = !self.show_details;
            self.set_status(if self.show_details {
                "Details expanded".into()
            } else {
                "Details collapsed".into()
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
            self.set_status("History search: type /session search <term>".into());
            return Ok(());
        }
        if self.action_key("redraw", &key) {
            self.transcript_scroll = 0;
            self.follow_transcript = true;
            self.set_status("Redrawn".into());
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
        if key.code == KeyCode::Tab {
            self.mode = match self.mode {
                Mode::Build => Mode::Plan,
                Mode::Plan => Mode::Ask,
                Mode::Ask => Mode::Build,
            };
            self.set_ok(format!("Mode: {}", self.mode.as_str()));
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
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.insert_text("\n");
            }
            KeyCode::Enter => self.submit().await?,
            KeyCode::Char(character) => self.insert_text(&character.to_string()),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.cursor = previous_boundary(&self.input, self.cursor),
            KeyCode::Right => self.cursor = next_boundary(&self.input, self.cursor),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::PageUp => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(8);
                self.follow_transcript = false;
            }
            KeyCode::PageDown => {
                self.transcript_scroll = self.transcript_scroll.saturating_add(8);
                self.follow_transcript = false;
            }
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
                            self.set_ok(format!("Model selected: {model}"));
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
                const COUNT: usize = 6;
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
            Overlay::None => {}
        }
        Ok(())
    }

    pub(crate) async fn submit(&mut self) -> Result<()> {
        let value = self.input.trim().to_string();
        if value.is_empty() {
            return Ok(());
        }
        // `#` asks for cheap local routing before any model call: shell
        // shapes prefill `!`, agent shapes send, the rest stays editable.
        // `submit_classified` owns the composer from here on.
        if let Some(classified) = value.strip_prefix('#') {
            self.submit_classified(classified.trim().to_string());
            self.at_cache_key.clear();
            self.arg_cache_key.clear();
            self.cursor = self.input.len();
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
                self.set_status("Usage: !<shell command>".into());
                return Ok(());
            }
            self.redo_stack.clear();
            self.state.history.push(Message::user(value));
            self.follow_transcript = true;
            self.run_shell_command(shell);
            return Ok(());
        }
        if let Some(parsed) = command::parse(&value) {
            self.handle_command(parsed).await
        } else {
            self.submit_prompt(value);
            Ok(())
        }
    }

    /// `#`-prefixed input: cheap local routing before any model call.
    /// Shell-shaped input prefills `!` for one-keystroke confirmation
    /// (never auto-runs); agent-shaped input submits directly; ambiguous
    /// input stays in the composer with a routing hint. Owns the composer:
    /// every arm leaves `input` in its final state.
    pub(crate) fn submit_classified(&mut self, text: String) {
        if text.is_empty() {
            self.input.clear();
            self.set_status("Usage: # <text to classify as shell or prompt>".into());
            return;
        }
        match command::classify_input(&text) {
            command::Route::Shell => {
                self.input = format!("!{text}");
                self.set_status(
                    "Looks like shell — Enter to run, delete ! to send as a prompt".into(),
                );
            }
            command::Route::Agent => {
                self.input.clear();
                self.submit_prompt(text);
            }
            command::Route::Ambiguous => {
                self.input = text;
                self.set_status(
                    "Ambiguous — Enter sends to the agent, prefix ! to run shell".into(),
                );
            }
        }
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
            self.set_error(format!(
                "Unknown @ref(s) sent literally: {}",
                unknown.join(", ")
            ));
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
            self.set_status(format!(
                "Steering… ({} queued behind)",
                self.queue.len() - 1
            ));
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
            self.set_error("Shell (!) is disabled by permission posture 'off'".into());
            return;
        }
        let workspace = self.state.workspace.clone();
        let sandbox = self.sandbox.clone();
        let allow_network =
            self.state.permission_posture != "restricted" && self.state.permission_posture != "off";
        let cancellation = self.cancellation.clone().unwrap_or_default();
        let sender = self.tx.clone();
        let preview: String = command.chars().take(60).collect();
        self.set_status(format!("Running shell: {preview}"));
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
                    format!(
                        "$ {command}\n{shown}{}",
                        if over { "\n⋯ output truncated" } else { "" }
                    )
                }
                Err(error) => format!("$ {command}\nfailed: {error:#}"),
            };
            let _ = sender.send(UiEvent::Notice(notice));
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
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.transcript_scroll = self.transcript_scroll.saturating_sub(3);
                self.follow_transcript = false;
            }
            MouseEventKind::ScrollDown => {
                self.transcript_scroll = self.transcript_scroll.saturating_add(3);
            }
            _ => {}
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
        self.input.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = previous_boundary(&self.input, self.cursor);
        self.input.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let end = next_boundary(&self.input, self.cursor);
        self.input.drain(self.cursor..end);
    }

    pub(crate) fn history_previous(&mut self) {
        if self.state.history.is_empty() {
            return;
        }
        let value = self
            .state
            .history
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(|m| m.content.clone());
        if let Some(value) = value {
            self.input = value;
            self.cursor = self.input.len();
        }
    }

    pub(crate) fn history_next(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    /// Replace the `@token` before the cursor with the selected pick.
    /// Directories keep a trailing `/` so completion can continue; files
    /// take a trailing space so typing resumes naturally.
    pub(crate) fn accept_at_complete(&mut self) -> bool {
        let items = self.at_cache_items.clone();
        let Some(pick) = items.get(self.at_selected).cloned() else {
            return false;
        };
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
pub(crate) const KEY_ACTIONS: [(&str, char); 5] = [
    ("cancel", 'x'),
    ("details", 'o'),
    ("tasks", 't'),
    ("history", 'r'),
    ("redraw", 'l'),
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
