//! UiApp commands: Slash-command dispatch and handlers.

use super::*;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
};
use std::io;

impl UiApp {
    pub(crate) async fn handle_command(&mut self, parsed: ParsedCommand) -> Result<()> {
        // Recency feeds the palette boost; typos are excluded so a
        // one-off misspelling never floats above real commands.
        if command::command(&parsed.name).is_some() || self.is_custom_command(&parsed.name) {
            self.note_recent(&parsed.name);
        }
        match parsed.name.as_str() {
            "/" | "/help" => self.command_help(&parsed.args),
            "/state" => self.command_state(),
            "/connect" | "/provider" => self.command_connect(&parsed.args),
            "/models" => self.start_model_list(),
            "/model" => {
                if let Some(model) = parsed.args.first() {
                    self.state.model = model.clone();
                    let model = model.clone();
                    self.persist_connection(Some(&model));
                    self.set_ok(format!("Model: {model}"));
                } else {
                    self.start_model_list();
                }
            }
            "/health" => self.start_health(),
            "/completion" => self.command_completion(&parsed.args).await,
            "/profiles" => self.start_profiles(),
            "/profile" => self.command_profile(&parsed.args),
            "/history" => self.command_history(),
            "/plan" => {
                self.set_mode(Mode::Plan);
            }
            "/build" => {
                self.set_mode(Mode::Build);
            }
            "/ask" => {
                self.set_mode(Mode::Ask);
            }
            "/skills" => self.command_skills(),
            "/skill" => self.command_skill(&parsed.args),
            "/compact" => self.command_compact(),
            "/tokens" => {
                let usage = self.state.token_usage();
                let cache = match (
                    self.state.last_usage.prompt_cache_hit_tokens,
                    self.state.last_usage.prompt_cache_miss_tokens,
                ) {
                    (Some(hit), Some(miss)) => format!(" · cache hit {hit} / miss {miss}"),
                    (Some(hit), None) => format!(" · cache hit {hit}"),
                    _ => String::new(),
                };
                self.push_system(&format!(
                    "Context: {} / {} tokens ({:.1}%, {} confidence){cache}",
                    usage.used_tokens,
                    usage.context_tokens,
                    usage.percent(),
                    usage.source
                ));
            }
            "/quality" => self.command_quality(&parsed.args),
            "/json" => self.command_json(&parsed.args),
            "/max" => self.command_max(&parsed.args),
            "/cache-prompt" => {
                self.state.cache_prompt = parsed
                    .args
                    .first()
                    .map(|value| value != "off")
                    .unwrap_or(!self.state.cache_prompt);
                let enabled = self.state.cache_prompt;
                self.set_ok(format!(
                    "Prompt cache {}",
                    if enabled { "on" } else { "off" }
                ));
                self.persist_config(|config| config.cache_prompt = enabled);
            }
            "/config" => self.command_config(&parsed.args).await,
            "/clear" => {
                // Backup first: a cleared transcript is otherwise gone.
                // A failed backup aborts the clear so nothing is lost.
                let backup = if self.state.history.is_empty() {
                    None
                } else {
                    match session::save_checkpoint(
                        &self.paths,
                        &self.state,
                        "clear",
                        self.current_session.as_deref(),
                    ) {
                        Ok(name) => Some(name),
                        Err(error) => {
                            self.set_error(format!("Clear backup failed, session kept: {error}"));
                            return Ok(());
                        }
                    }
                };
                self.state.history.clear();
                self.streaming.clear();
                self.streaming_reasoning.clear();
                self.redo_stack.clear();
                self.prune_sections();
                self.set_ok(match backup {
                    Some(name) => format!("Cleared · backup {name}"),
                    None => "Cleared".to_string(),
                });
            }
            "/workspace" => self.command_workspace(&parsed.args),
            "/session" => self.command_session(&parsed.args),
            "/export" => self.command_export(&parsed.args),
            "/mcp" => self.command_mcp(&parsed.args)?,
            "/plugin" => self.push_system(&serde_json::to_string_pretty(
                &crate::plugin::status_from(&self.plugins_dir),
            )?),
            "/theme" => self.command_theme(&parsed.args),
            "/autocompact" => self.command_autocompact(&parsed.args),
            "/reasoning" => self.command_reasoning(&parsed.args),
            "/permissions" => self.command_permissions(&parsed.args),
            "/preview" => self.command_preview(&parsed.args),
            "/map" => self.push_system(&self.workspace_map()),
            "/diff" => self.push_system(&self.workspace_diff()),
            "/copy" => {
                self.command_copy(&parsed.args);
            }
            "/tasks" => self.push_system(&format!(
                "busy={} queued={} tool_round={}",
                self.busy,
                self.queue.len(),
                self.tool_round
            )),
            "/retry" => self.command_retry(),
            "/undo" => self.command_undo(),
            "/redo" => self.command_redo(),
            "/rewind" => self.command_rewind(&parsed.args),
            "/filter" => self.command_filter(&parsed.args),
            "/block" => self.command_block(&parsed.args),
            "/rerun" => self.command_rerun(&parsed.args).await?,
            "/expand" => self.command_expand(&parsed.args),
            "/editor" => self.command_editor(),
            "/settings" => {
                self.overlay = Overlay::Settings { selected: 0 };
                self.set_status("Settings".into());
            }
            "/thinking" => self.command_thinking(&parsed.args),
            "/attention" => self.command_attention(&parsed.args),
            "/mouse" => self.command_mouse(&parsed.args),
            "/commands" => self.command_custom_commands(&parsed.args),
            "/workflows" => self.command_workflows(&parsed.args),
            "/exit" => self.quit = true,
            _ => {
                if let Some(custom) = self.find_custom_command(&parsed.name) {
                    let expanded = custom::substitute_args(&custom.content, &parsed.args);
                    self.submit_prompt(expanded);
                    // `submit_prompt` sets Sending/Steering/Queued status;
                    // keep it and prefix the expansion attribution.
                    let outcome = std::mem::take(&mut self.status);
                    self.set_status(format!(
                        "Expanded /{} ({}) · {}",
                        custom.name, custom.source, outcome
                    ));
                } else if let Some(hit) = command::suggest(&parsed.name, &self.custom_commands) {
                    self.set_error(format!(
                        "Unknown command {}; did you mean {hit}?",
                        parsed.name
                    ));
                } else {
                    self.set_error(format!("Unknown command {}; type /help", parsed.name));
                }
            }
        }
        Ok(())
    }

    /// `/help [command]`: full dump by default, or one entry's usage,
    /// description, and (for customs) argument hint plus source file.
    pub(crate) fn command_help(&mut self, args: &[String]) {
        let Some(topic) = args.first() else {
            self.push_system(&command::help_text_with(&self.custom_commands));
            return;
        };
        if let Some(item) = command::command(topic) {
            self.push_system(&format!("{}\n  {}", item.usage, item.description));
            return;
        }
        if let Some(custom) = self.find_custom_command(topic) {
            let mut output = format!("/{}\n  {}", custom.name, custom.description);
            if let Some(hint) = &custom.argument_hint {
                output.push_str(&format!("\n  arguments: {hint}"));
            }
            output.push_str(&format!(
                "\n  source: {} ({})",
                custom.source,
                custom.path.display()
            ));
            self.push_system(&output);
            return;
        }
        self.push_system(&command::help_text_with(&self.custom_commands));
    }

    /// `/commands [reload]`: list loaded Markdown commands with scope, or
    /// reload both scopes first. Shadowed files (built-in wins) are
    /// reported so nothing is silently ignored.
    pub(crate) fn command_custom_commands(&mut self, args: &[String]) {
        if args.first().is_some_and(|action| action == "reload") {
            let shadowed = self.refresh_custom_commands();
            self.set_ok(if shadowed == 0 {
                format!("Reloaded {} workflow(s)", self.custom_commands.len())
            } else {
                format!(
                    "Reloaded {} workflow(s); {shadowed} shadowed by built-ins",
                    self.custom_commands.len()
                )
            });
        }
        if self.custom_commands.is_empty() {
            self.push_system(&format!(
                "No saved workflows. Add name.md files in\n  {}\n  {}",
                commands_dir(&self.paths).display(),
                project_commands_dir(&self.state.workspace).display()
            ));
            return;
        }
        let mut lines = vec!["Saved workflows".to_string(), String::new()];
        for command in &self.custom_commands {
            let shadow = if command::command(&format!("/{}", command.name)).is_some() {
                " (shadowed by built-in)"
            } else {
                ""
            };
            lines.push(format!(
                "  /{:<18} {} ({}){shadow}",
                command.name, command.description, command.source
            ));
        }
        self.push_system(&lines.join("\n"));
    }

    pub(crate) fn command_state(&mut self) {
        let connection = self.backend.connection();
        self.push_system(&format!(
            "Mode: {} · Model: {} ({})\nBackend: {} · {}\nPermissions: {} · Sandbox: {} · Approvals: {}\nThinking: {} · Reasoning: {} · Bell: {}\nQuality: {} · Profile: {}",
            self.mode.as_str(),
            self.state.model,
            connection.display_name(),
            connection.backend,
            connection.base_url,
            self.state.permission_posture,
            self.sandbox.selected_name(),
            self.policy.summary(),
            if self.state.show_thinking {
                "shown"
            } else {
                "hidden"
            },
            self.state.reasoning_effort,
            if self.attention_bell { "on" } else { "off" },
            self.state.quality.as_deref().unwrap_or("auto"),
            self.state.profile.as_deref().unwrap_or("auto"),
        ));
    }

    /// `#<plain words>`: ask the model for one shell command and put it
    /// in the composer for review. Nothing runs without an explicit
    /// Enter, and the composer's shell routing plus the sandbox apply
    /// unchanged. The draft is a cheap history-free one-shot and never
    /// touches the session or the busy/queue machinery.
    pub(crate) fn command_shell_draft(&mut self, args: &[String]) {
        let request = args.join(" ");
        if request.is_empty() {
            self.set_status("Usage: # describe the shell command".into());
            return;
        }
        if self.state.permission_posture == "off" {
            self.set_error("Shell disabled (permissions off)".into());
            return;
        }
        if self.busy {
            self.set_status("Busy — try when idle".into());
            return;
        }
        let shell = if cfg!(windows) {
            "Windows cmd.exe (run via `cmd /C`)"
        } else {
            "POSIX sh (run via `sh -c`)"
        };
        let prompt = format!(
            "Translate the request into ONE shell command for {} on {}.\n\
             Output ONLY the command: no fences, no surrounding quotes, no explanation, single line.\n\
             Request: {request}",
            shell,
            std::env::consts::OS,
        );
        let backend = self.backend.clone();
        let mut state = self.state.clone();
        state.history.clear();
        let sender = self.tx.clone();
        let pane = self.id;
        self.set_status("Drafting…".into());
        tokio::spawn(async move {
            let outcome = match backend.chat(&state, &prompt, &[]).await {
                Ok(result) => match clean_shell_draft(&result.content) {
                    Some(command) => Ok(command),
                    None => Err("The model returned no usable command".to_string()),
                },
                Err(error) => Err(format!("Shell draft failed: {error:#}")),
            };
            let _ = sender.send(crate::ui::events::UiEvent::ShellDraft(outcome).at(pane));
        });
    }

    pub(crate) fn command_history(&mut self) {
        if self.state.history.is_empty() {
            self.push_system("Session is empty");
            return;
        }
        let start = self.state.history.len().saturating_sub(12);
        let mut lines = vec![format!(
            "Last {} of {} messages:",
            self.state.history.len() - start,
            self.state.history.len()
        )];
        for message in &self.state.history[start..] {
            let preview = message
                .content
                .chars()
                .take(160)
                .collect::<String>()
                .replace('\n', " ");
            lines.push(format!("[{}] {}", message.role, preview));
        }
        self.push_system(&lines.join("\n"));
    }

    pub(crate) fn command_quality(&mut self, args: &[String]) {
        const VALID: [&str; 3] = ["fast", "balanced", "best"];
        let Some(value) = args.first() else {
            self.push_system(&format!(
                "quality={}",
                self.state.quality.as_deref().unwrap_or("auto")
            ));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.set_error(format!("Unknown quality; choose {}", VALID.join(", ")));
            return;
        }
        self.state.quality = Some(value.clone());
        self.persist_config(|config| config.quality = Some(value.clone()));
        self.set_ok(format!("Quality: {value}"));
    }

    pub(crate) fn command_profile(&mut self, args: &[String]) {
        const VALID: [&str; 7] = [
            "simple",
            "strict_json",
            "coding",
            "complex_reasoning",
            "long_context_qa",
            "tool_agent",
            "creative",
        ];
        let Some(value) = args.first() else {
            self.state.profile = None;
            self.persist_config(|config| config.profile = None);
            self.set_ok("Router profile: auto".into());
            return;
        };
        let value = value.to_ascii_lowercase();
        if value == "auto" {
            self.state.profile = None;
            self.persist_config(|config| config.profile = None);
            self.set_ok("Router profile: auto".into());
        } else if VALID.contains(&value.as_str()) {
            self.state.profile = Some(value.clone());
            self.persist_config(|config| config.profile = Some(value.clone()));
            self.set_ok(format!("Router profile: {value}"));
        } else {
            self.set_error(format!(
                "Unknown profile; choose auto, {}",
                VALID.join(", ")
            ));
        }
    }

    pub(crate) fn command_json(&mut self, args: &[String]) {
        self.state.json_mode = toggle_value(args.first(), self.state.json_mode);
        self.set_ok(format!(
            "JSON response mode: {}",
            if self.state.json_mode { "on" } else { "off" }
        ));
    }

    pub(crate) fn command_max(&mut self, args: &[String]) {
        let Some(value) = args.first() else {
            self.state.max_tokens = None;
            self.set_ok("Maximum completion tokens: auto".into());
            return;
        };
        match value.parse::<u32>() {
            Ok(value) if value > 0 => {
                self.state.max_tokens = Some(value);
                self.set_ok(format!("Maximum completion tokens: {value}"));
            }
            _ => self.set_status("Usage: /max <positive token count>".into()),
        }
    }

    pub(crate) async fn command_config(&mut self, args: &[String]) {
        let action = args.first().map(String::as_str).unwrap_or("reload");
        let config = match Config::load(&self.paths) {
            Ok(config) => config,
            Err(error) => {
                self.set_error(format!("Config read failed: {error:#}"));
                return;
            }
        };
        if action == "show" {
            match serde_json::to_string_pretty(&config) {
                Ok(value) => self.push_system(&value),
                Err(error) => self.set_error(format!("Config formatting failed: {error}")),
            }
            return;
        }
        if action != "reload" {
            self.set_status("Usage: /config show|reload".into());
            return;
        }

        let previous = self.backend.connection().clone();
        let mut connection_changed = false;
        if config.backend.is_some() || config.url.is_some() || config.provider.is_some() {
            let mut connection = provider::resolve_connection(
                config.provider.as_deref(),
                config.backend.as_deref(),
                config.url.as_deref(),
            );
            // A key entered in the guided TUI lives only in memory. Retain it
            // across a reload when the endpoint remains the same.
            if connection.api_key.is_none()
                && connection.base_url == previous.base_url
                && connection.provider_id == previous.provider_id
            {
                connection.api_key = previous.api_key.clone();
            }
            if connection.backend != previous.backend
                || connection.base_url != previous.base_url
                || connection.provider_id != previous.provider_id
            {
                match self.backend.with_connection(connection) {
                    Ok(candidate) => {
                        self.backend = candidate;
                        connection_changed = true;
                    }
                    Err(error) => {
                        self.set_error(format!("Config connection rejected: {error}"));
                        return;
                    }
                }
            }
        }

        self.state.theme = config.theme.clone();
        self.state.quality = config.quality.clone();
        self.state.profile = config.profile.clone();
        if let Some(model) = config.model.clone() {
            self.state.model = model;
        }
        self.state.auto_compact = config.auto_compact;
        self.state.cache_prompt = config.cache_prompt;
        self.state.reasoning_effort = config.reasoning_effort.clone();
        self.state.show_thinking = config.show_thinking;
        self.state.thinking_default_expanded = config.thinking_default_expanded;
        self.state.permission_posture = config.permission_posture.clone();
        self.state.keybindings = config.keybindings.clone();
        self.state.skills_dir = config.skills_dir.clone();
        self.state.model_contexts = config.model_contexts.clone();
        self.attention_bell = config.attention_bell;
        let mouse_note = if config.mouse != self.mouse_enabled {
            self.mouse_enabled = config.mouse;
            if config.mouse {
                let _ = execute!(io::stdout(), EnableMouseCapture);
            } else {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
            " · mouse updated"
        } else {
            ""
        };
        if let Some(tokens) = config.context_tokens {
            self.state.context_tokens = tokens;
        }
        self.plugins_dir = config.plugins_dir.clone();
        self.sandbox = Sandbox::detect(
            &config.sandbox_backend,
            config.docker_image.clone(),
            config.timeout_seconds,
        );
        let connection_note = if connection_changed {
            ", connection updated"
        } else {
            ""
        };
        self.refresh_skills();
        self.refresh_custom_commands();
        self.refresh_git_branch();
        self.set_ok(format!("Config reloaded{connection_note}{mouse_note}"));
    }

    pub(crate) fn command_autocompact(&mut self, args: &[String]) {
        self.state.auto_compact = toggle_value(args.first(), self.state.auto_compact);
        let enabled = self.state.auto_compact;
        self.persist_config(|config| config.auto_compact = enabled);
        self.set_ok(format!(
            "Autocompact {}",
            if enabled { "on" } else { "off" }
        ));
    }

    pub(crate) fn command_reasoning(&mut self, args: &[String]) {
        const VALID: [&str; 9] = [
            "auto", "off", "none", "disabled", "low", "medium", "high", "max", "xhigh",
        ];
        let Some(value) = args.first() else {
            self.push_system(&format!("reasoning_effort={}", self.state.reasoning_effort));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.set_error(format!(
                "Unknown reasoning effort; choose {}",
                VALID.join(", ")
            ));
            return;
        }
        self.state.reasoning_effort = value.clone();
        self.persist_config(|config| config.reasoning_effort = value.clone());
        self.set_ok(format!("Reasoning: {value}"));
    }

    pub(crate) fn command_permissions(&mut self, args: &[String]) {
        const VALID: [&str; 4] = ["full-access", "restricted", "sandboxed", "off"];
        let Some(value) = args.first() else {
            self.push_system(&format!(
                "permission_posture={} (valid: {})",
                self.state.permission_posture,
                VALID.join(", ")
            ));
            return;
        };
        let value = value.to_ascii_lowercase();
        if !VALID.contains(&value.as_str()) {
            self.set_error(format!(
                "Unknown permission posture; choose {}",
                VALID.join(", ")
            ));
            return;
        }
        self.state.permission_posture = value.clone();
        self.persist_config(|config| config.permission_posture = value.clone());
        self.set_ok(format!("Permissions: {value}"));
    }

    pub(crate) fn command_preview(&mut self, args: &[String]) {
        let Some(requested) = args.first() else {
            self.set_status("Usage: /preview <filename>".into());
            return;
        };
        let path = match safe_path(&self.state.workspace, requested) {
            Ok(path) => path,
            Err(error) => {
                self.set_error(format!("Preview path rejected: {error}"));
                return;
            }
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => self.push_system(&format!(
                "--- {requested} ---\n{}",
                content.chars().take(2_000).collect::<String>()
            )),
            Err(error) => self.set_error(format!("Preview failed: {error}")),
        }
    }

    pub(crate) fn command_copy(&mut self, args: &[String]) {
        // `/copy out [n]`: whole block verbatim (or its filtered view
        // when a /filter is active), spec 0017.
        if args.first().is_some_and(|first| first == "out") {
            self.copy_block(args.get(1));
            return;
        }
        if self.last_response.is_empty() {
            self.set_error("There is no response to copy".into());
            return;
        }
        let Some(requested) = args.first() else {
            if copy_to_clipboard(&self.last_response) {
                self.set_ok(format!(
                    "Copied {} characters",
                    self.last_response.chars().count()
                ));
            } else {
                self.set_error(
                    "Clipboard unavailable (try pbcopy, wl-copy, xclip, or clip)".into(),
                );
            }
            return;
        };
        let index: usize = match requested.parse() {
            Ok(number) if number >= 1 => number,
            _ => {
                self.set_status("Usage: /copy [n] (nth fenced code block)".into());
                return;
            }
        };
        let blocks = code_blocks(&self.last_response);
        match blocks.get(index - 1) {
            Some(block) => {
                if copy_to_clipboard(block) {
                    self.set_ok(format!(
                        "Copied code block {index} ({} characters)",
                        block.chars().count()
                    ));
                } else {
                    self.set_error(
                        "Clipboard unavailable (try pbcopy, wl-copy, xclip, or clip)".into(),
                    );
                }
            }
            None => {
                self.set_error(format!(
                    "Code block {index} not found ({} fenced block(s) in last response)",
                    blocks.len()
                ));
            }
        }
    }

    /// Copy one block to the clipboard: the filtered view when the
    /// block carries a `/filter`, otherwise the full content. Defaults
    /// to the last message.
    fn copy_block(&mut self, arg: Option<&String>) {
        let history_len = self.state.history.len();
        let number = match arg {
            Some(text) => match parse_block_number(text) {
                Some(number) => number,
                None => {
                    self.set_status("Usage: /copy out [n]".into());
                    return;
                }
            },
            None => history_len,
        };
        if number < 1 || number > history_len {
            self.set_error(format!("Block {number} out of range (1-{history_len})"));
            return;
        }
        let index = number - 1;
        let id = self.section_id(index);
        let message = &self.state.history[index];
        let content = block_copy_content(&message.content, self.block_filters.get(&id));
        if copy_to_clipboard(&content) {
            self.set_ok(format!(
                "Copied block {number} ({} characters)",
                content.chars().count()
            ));
        } else {
            self.set_error("Clipboard unavailable (try pbcopy, wl-copy, xclip, or clip)".into());
        }
    }

    pub(crate) fn persist_config<F>(&mut self, update: F)
    where
        F: FnOnce(&mut Config),
    {
        match Config::load(&self.paths) {
            Ok(mut config) => {
                update(&mut config);
                if let Err(error) = config.save(&self.paths) {
                    self.set_error(format!("Setting changed, but config save failed: {error}"));
                }
            }
            Err(error) => {
                self.set_error(format!("Setting changed, but config read failed: {error}"))
            }
        }
    }

    pub(crate) fn command_compact(&mut self) {
        if self.busy {
            self.set_error("Finish the active request before compacting".into());
            return;
        }
        if self.state.history.len() < 4 {
            self.set_error("There is not enough conversation to compact yet".into());
            return;
        }
        self.start_compaction(false);
    }

    pub(crate) fn command_retry(&mut self) {
        if self.busy {
            self.set_error("Finish the active request before retrying".into());
            return;
        }
        let Some(prompt) = self.last_failed_prompt.clone().or_else(|| {
            if self.input.trim().is_empty() {
                None
            } else {
                Some(self.input.trim().to_string())
            }
        }) else {
            self.set_error("Nothing to retry".into());
            return;
        };
        self.last_failed_prompt = None;
        self.input.clear();
        self.cursor = 0;
        self.start_prompt(prompt, None);
    }

    pub(crate) fn start_compaction(&mut self, automatic: bool) {
        if self.state.history.len() < 4 {
            self.set_error("There is not enough conversation to compact yet".into());
            return;
        }
        // Backup first: the summary replaces the transcript, so a bad
        // compaction must stay recoverable via `/session load`.
        let backup = match session::save_checkpoint(
            &self.paths,
            &self.state,
            "compact",
            self.current_session.as_deref(),
        ) {
            Ok(name) => name,
            Err(error) => {
                self.set_error(format!("Compact backup failed, session kept: {error}"));
                return;
            }
        };
        let (older, recent) = split_compact(&self.state.history);
        if older.is_empty() {
            self.set_error("There is not enough conversation to compact yet".into());
            return;
        }
        let older = older.to_vec();
        let recent = recent.to_vec();
        let transcript = older
            .iter()
            .map(|message| format!("{}: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n\n");
        let prompt = format!(
            "Summarize the following conversation for a future coding harness turn. \
             Preserve decisions, constraints, file paths, unresolved work, and user preferences. \
             Be concise and factual.\n\n{transcript}"
        );
        let backend = self.backend.clone();
        let mut state = self.state.clone();
        state.history.clear();
        let sender = self.tx.clone();
        let pane = self.id;
        self.busy = true;
        self.request_started = Some(Instant::now());
        self.awaiting_first_token = true;
        self.slow_hint_shown = false;
        self.cancellation = Some(CancellationToken::new());
        self.compact_backup = Some(backup);
        self.set_status(if automatic {
            "Context near its limit; compacting…".into()
        } else {
            "Compacting context…".into()
        });
        tokio::spawn(async move {
            match backend.chat(&state, &prompt, &[]).await {
                Ok(result) => {
                    let _ = sender.send(
                        crate::ui::events::UiEvent::Compacted {
                            summary: result.content,
                            recent,
                        }
                        .at(pane),
                    );
                }
                Err(error) => {
                    let _ = sender
                        .send(crate::ui::events::UiEvent::ChatError(format!("{error:#}")).at(pane));
                }
            }
        });
    }

    /// Apply a compaction summary, replacing the transcript head. Returns
    /// false (history untouched) when the summary is blank — wiping
    /// context for an empty summary is never a valid compaction.
    pub(crate) fn apply_compaction(&mut self, summary: String, recent: Vec<Message>) -> bool {
        if summary.trim().is_empty() {
            self.busy = false;
            self.cancellation = None;
            self.compact_backup = None;
            self.follow_transcript = true;
            self.set_error("Compaction returned an empty summary · /compact to retry".into());
            return false;
        }
        self.state.history = vec![Message::system(format!("Conversation summary:\n{summary}"))];
        self.state.history.extend(recent);
        self.state.last_usage = Usage::default();
        self.prune_sections();
        self.busy = false;
        self.cancellation = None;
        self.follow_transcript = true;
        let backup_note = self
            .compact_backup
            .take()
            .map(|name| format!(" · backup {name}"))
            .unwrap_or_default();
        self.set_ok(format!("Context compacted{backup_note}"));
        true
    }

    pub(crate) fn command_skills(&mut self) {
        let mut names = std::fs::read_dir(&self.state.skills_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                (entry.path().extension().and_then(|value| value.to_str()) == Some("md"))
                    .then(|| entry.file_name().to_string_lossy().to_string())
            })
            .collect::<Vec<_>>();
        names.sort();
        let output = if names.is_empty() {
            "No Markdown skills found".to_string()
        } else {
            format!(
                "Skills in {}:\n{}",
                self.state.skills_dir.display(),
                names.join("\n")
            )
        };
        self.push_system(&output);
    }

    pub(crate) fn command_skill(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            Some("use") => {
                let Some(name) = args.get(1).map(String::as_str) else {
                    self.set_status("Usage: /skill use <name>".into());
                    return;
                };
                let Some(name) = skill_name(name) else {
                    self.set_error("Skill names must be local Markdown filenames".into());
                    return;
                };
                let path = self.state.skills_dir.join(format!("{name}.md"));
                if !path.is_file() {
                    self.set_error(format!("Skill not found: {}", path.display()));
                } else if !self.state.active_skills.contains(&name) {
                    self.state.active_skills.push(name.clone());
                    let params = args
                        .iter()
                        .skip(2)
                        .filter_map(|value| value.split_once('='))
                        .map(|(key, value)| (key.to_string(), value.to_string()))
                        .collect::<std::collections::BTreeMap<_, _>>();
                    if !params.is_empty() {
                        self.state.skill_params.insert(name.clone(), params);
                    }
                    self.set_ok(format!("Skill active: {name}"));
                } else {
                    self.set_status(format!("Skill already active: {name}"));
                }
            }
            Some("show") => {
                let Some(name) = args.get(1).and_then(|value| skill_name(value)) else {
                    self.set_status("Usage: /skill show <name>".into());
                    return;
                };
                let path = self.state.skills_dir.join(format!("{name}.md"));
                match std::fs::read_to_string(&path) {
                    Ok(content) => self.push_system(&format!("Skill: {name}\n\n{content}")),
                    Err(error) => self.set_error(format!("Skill read failed: {error}")),
                }
            }
            Some("drop") => {
                let Some(name) = args.get(1).and_then(|value| skill_name(value)) else {
                    self.set_status("Usage: /skill drop <name>".into());
                    return;
                };
                let before = self.state.active_skills.len();
                self.state.active_skills.retain(|item| item != &name);
                self.state.skill_params.remove(&name);
                if before == self.state.active_skills.len() {
                    self.set_status(format!("Skill was not active: {name}"));
                } else {
                    self.set_ok(format!("Skill inactive: {name}"));
                }
            }
            Some("clear") => {
                self.state.active_skills.clear();
                self.set_ok("All skills cleared".into());
            }
            _ => {
                self.set_status("Usage: /skill use|show|drop|clear <name>".into());
            }
        }
    }

    pub(crate) fn command_connect(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            None => {
                self.overlay = Overlay::Providers {
                    selected: 0,
                    scroll: 0,
                };
                self.set_status("Choose a provider; credentials stay in memory".into());
            }
            Some("status") | Some("show") => {
                let connection = self.backend.connection();
                self.push_system(&format!(
                    "provider={} backend={} url={} model={}",
                    connection.display_name(),
                    connection.backend,
                    connection.base_url,
                    self.state.model
                ));
            }
            Some("url") | Some("custom") => {
                if let Some(url) = args.get(1) {
                    if provider::valid_url(url) {
                        self.start_provider("custom".into(), Some(url.clone()));
                    } else {
                        self.set_error("Invalid custom URL".into());
                    }
                } else {
                    self.open_url_overlay("custom".into());
                }
            }
            Some(provider_id) => {
                let provider_id = provider::aliases(provider_id);
                if let Some(preset) = provider::preset(&provider_id) {
                    if preset.asks_for_url && args.get(1).is_none() {
                        self.open_url_overlay(provider_id);
                    } else {
                        self.start_provider(provider_id, args.get(1).cloned());
                    }
                } else {
                    self.status =
                        format!("Unknown provider '{provider_id}'; use /connect to browse");
                }
            }
        }
    }

    pub(crate) fn open_url_overlay(&mut self, provider: String) {
        self.input.clear();
        self.cursor = 0;
        self.set_status(match provider::preset(&provider) {
            Some(preset) if preset.base_url.is_some() => {
                format!(
                    "Enter {} base URL; Enter uses the local default",
                    preset.label
                )
            }
            _ => "Enter an OpenAI-compatible http(s) base URL".into(),
        });
        self.overlay = Overlay::CustomUrl { provider };
    }

    pub(crate) fn command_mcp(&mut self, args: &[String]) -> Result<()> {
        match args.first().map(String::as_str) {
            Some("reconnect") => {
                let server = args.get(1).cloned();
                self.set_status(match server.as_deref() {
                    Some(name) => format!("Reconnecting MCP server {name}…"),
                    None => "Reconnecting MCP servers…".into(),
                });
                let sender = self.tx.clone();
                let pane = self.id;
                tokio::spawn(async move {
                    let notice = match crate::mcp::reconnect(server.as_deref()).await {
                        Ok(message) => message,
                        Err(error) => format!("MCP reconnect failed: {error:#}"),
                    };
                    let _ = sender.send(crate::ui::events::UiEvent::Notice(notice).at(pane));
                });
            }
            Some("tools") => {
                let tools = crate::mcp::definitions();
                self.push_system(
                    &serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".into()),
                );
            }
            Some("list") | Some("status") | None => {
                self.push_system(&serde_json::to_string_pretty(&crate::mcp::status())?);
            }
            Some(other) => {
                self.set_status(format!(
                    "Usage: /mcp list|tools|reconnect [server] (got {other})"
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn start_provider(&mut self, id: String, entered: Option<String>) {
        let Some(preset) = provider::preset(&id) else {
            self.set_error(format!("Unknown provider '{id}'"));
            return;
        };
        let entered_url = entered
            .as_deref()
            .filter(|value| provider::valid_url(value));
        let mut connection = provider::resolve_connection(Some(preset.id), None, entered_url);
        if preset.api_key_required && entered.is_none() && connection.api_key.is_none() {
            self.overlay = Overlay::ApiKey {
                provider: preset.id.to_string(),
            };
            self.set_status(format!(
                "{} requires {} (the key stays in memory)",
                preset.label,
                preset.api_key_env.unwrap_or("an API key")
            ));
            return;
        }
        if preset.id == "custom" {
            if entered_url.is_none() {
                self.set_status("Custom connections require a valid URL".into());
                return;
            }
        } else if entered_url.is_none()
            && let Some(key) = entered
        {
            connection.api_key = Some(key);
        }
        match self.backend.with_connection(connection.clone()) {
            Ok(candidate) => {
                self.set_status(format!("Checking {} and loading models…", preset.label));
                self.pending_connection = Some(connection);
                let sender = self.tx.clone();
                let pane = self.id;
                tokio::spawn(async move {
                    match candidate.list_models().await {
                        Ok(value) => {
                            let models = extract_models(&value);
                            let runtime_context = candidate.context_limit().await;
                            let _ = sender.send(
                                crate::ui::events::UiEvent::ModelsLoaded {
                                    backend: candidate,
                                    models,
                                    runtime_context,
                                }
                                .global(),
                            );
                        }
                        Err(error) => {
                            let _ = sender.send(
                                crate::ui::events::UiEvent::Notice(format!(
                                    "connection failed: {error:#}"
                                ))
                                .at(pane),
                            );
                        }
                    }
                });
            }
            Err(error) => self.set_error(format!("connection failed: {error}")),
        }
    }

    pub(crate) fn start_model_list(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        let pane = self.id;
        self.set_status("Refreshing model list…".into());
        tokio::spawn(async move {
            match backend.list_models().await {
                Ok(value) => {
                    let models = extract_models(&value);
                    if models.is_empty() {
                        let _ = sender.send(
                            crate::ui::events::UiEvent::Notice(
                                "Provider returned no models".into(),
                            )
                            .at(pane),
                        );
                    } else {
                        let runtime_context = backend.context_limit().await;
                        let _ = sender.send(
                            crate::ui::events::UiEvent::ModelsLoaded {
                                backend,
                                models,
                                runtime_context,
                            }
                            .global(),
                        );
                    }
                }
                Err(error) => {
                    let _ = sender.send(
                        crate::ui::events::UiEvent::Notice(format!("model list failed: {error:#}"))
                            .at(pane),
                    );
                }
            }
        });
    }

    /// Best-effort background probe of the configured backend so the
    /// context budget is right before the first `/models`. Failures are
    /// silent: the footer keeps the configured default.
    pub(crate) fn refresh_context(&self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        let pane = self.id;
        tokio::spawn(async move {
            let runtime_context = backend.context_limit().await;
            let contexts = match backend.list_models().await {
                Ok(value) => extract_models(&value)
                    .into_iter()
                    .filter_map(|model| model.context.map(|context| (model.id, context)))
                    .collect(),
                Err(_) => std::collections::BTreeMap::new(),
            };
            if runtime_context.is_some() || !contexts.is_empty() {
                let _ = sender.send(
                    crate::ui::events::UiEvent::ContextObserved {
                        runtime_context,
                        contexts,
                    }
                    .at(pane),
                );
            }
        });
    }

    pub(crate) fn start_health(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        let pane = self.id;
        self.set_status("Checking backend…".into());
        tokio::spawn(async move {
            let notice = match backend.health().await {
                Ok(value) => {
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                }
                Err(error) => format!("health failed: {error:#}"),
            };
            let _ = sender.send(crate::ui::events::UiEvent::Notice(notice).at(pane));
        });
    }

    pub(crate) fn start_profiles(&mut self) {
        let backend = self.backend.clone();
        let sender = self.tx.clone();
        let pane = self.id;
        self.set_status("Loading router profiles…".into());
        tokio::spawn(async move {
            let notice = match backend.profiles().await {
                Ok(value) => {
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                }
                Err(error) => format!("profiles failed: {error:#}"),
            };
            let _ = sender.send(crate::ui::events::UiEvent::Notice(notice).at(pane));
        });
    }

    pub(crate) fn persist_connection(&mut self, model: Option<&String>) {
        let Ok(mut config) = Config::load(&self.paths) else {
            self.set_error("connected, but could not read config to persist connection".into());
            return;
        };
        let connection = self.backend.connection();
        config.provider = connection.provider_id.clone();
        config.backend = Some(connection.backend.clone());
        config.url = Some(connection.base_url.clone());
        if let Some(model) = model {
            config.model = Some(model.clone());
        }
        if let Err(error) = config.save(&self.paths) {
            self.set_error(format!("connected, but could not save config: {error}"));
        }
    }

    pub(crate) fn command_workspace(&mut self, args: &[String]) {
        if let Some(path) = args.first() {
            let path = PathBuf::from(path).expanduser();
            if let Err(error) = std::fs::create_dir_all(&path) {
                self.set_error(format!("workspace error: {error}"));
            } else {
                self.state.workspace = path;
                self.refresh_git_branch();
                self.refresh_custom_commands();
                let workspace = self.state.workspace.clone();
                self.note_workspace(&workspace);
                self.refresh_sidebar();
                self.set_ok(format!("Workspace: {}", self.state.workspace.display()));
            }
        } else {
            self.push_system(&format!("Workspace: {}", self.state.workspace.display()));
        }
    }

    /// Load a saved session by name, reseeding transcript bookkeeping.
    /// Shared by `/session load` and the sidebar.
    pub(crate) fn load_session_named(&mut self, name: &str) {
        let paths = self.paths.clone();
        match session::load(&paths, name, &mut self.state) {
            Ok(count) => {
                self.redo_stack.clear();
                self.reseed_msg_ids();
                self.prune_sections();
                self.sync_mode_from_state();
                self.current_session = Some(name.to_string());
                self.follow_transcript = true;
                self.set_ok(format!("Loaded {name} ({count} messages)"));
            }
            Err(error) => self.set_error(format!("Session load failed: {error}")),
        }
    }

    pub(crate) fn command_session(&mut self, args: &[String]) {
        match args.first().map(String::as_str) {
            Some("save") => {
                let name = args.get(1).map(String::as_str).unwrap_or("default");
                match session::save(&self.paths, name, &self.state) {
                    Ok(path) => {
                        self.current_session = Some(name.to_string());
                        self.refresh_sidebar();
                        self.set_ok(format!("Saved {}", path.display()));
                    }
                    Err(error) => self.set_error(format!("Session save failed: {error}")),
                }
            }
            Some("load") => {
                let name = args
                    .get(1)
                    .map(String::as_str)
                    .unwrap_or("default")
                    .to_string();
                self.load_session_named(&name);
                self.refresh_sidebar();
            }
            Some("list") => {
                let items = session::list(&self.paths);
                self.push_system(&serde_json::to_string_pretty(&items).unwrap_or_default());
            }
            Some("search") => {
                let query = args.get(1).map(String::as_str).unwrap_or_default();
                self.push_system(
                    &serde_json::to_string_pretty(&session::search(&self.paths, query))
                        .unwrap_or_default(),
                );
            }
            Some("delete") => {
                let name = args.get(1).map(String::as_str).unwrap_or_default();
                match session::delete(&self.paths, name) {
                    Ok(true) => {
                        self.refresh_sidebar();
                        self.set_ok(format!("Deleted session {name}"));
                    }
                    Ok(false) => self.set_error(format!("Session not found: {name}")),
                    Err(error) => self.set_error(format!("Session delete failed: {error}")),
                }
            }
            Some("diff") => {
                let name = args.get(1).map(String::as_str).unwrap_or("default");
                match session::diff(&self.paths, name, &self.state) {
                    Ok(value) => self.push_system(&value),
                    Err(error) => self.set_error(format!("Session diff failed: {error}")),
                }
            }
            Some("fork") => {
                let Some(name) = args.get(1).map(String::as_str) else {
                    self.set_status("Usage: /session fork <name> [turns]".into());
                    return;
                };
                // The fork records where it came from; the working session
                // stays current (you keep working here either way).
                let parent = self.current_session.clone();
                if let Some(turns) = args.get(2) {
                    // Snapping fork: keep the first N user turns only. The
                    // cut lands on a user boundary by construction, and
                    // `repair_prefix` drops anything still stranded.
                    let turns = turns.parse::<usize>().unwrap_or(0);
                    if turns == 0 {
                        self.set_error("Usage: /session fork <name> [turns]".into());
                        return;
                    }
                    let mut seen = 0;
                    let mut end = self.state.history.len();
                    for (position, message) in self.state.history.iter().enumerate() {
                        if message.role == "user" {
                            seen += 1;
                            if seen == turns + 1 {
                                end = position;
                                break;
                            }
                        }
                    }
                    let mut forked = self.state.clone();
                    forked.history = session::repair_prefix(forked.history[..end].to_vec());
                    let kept = forked.history.len();
                    match session::save_with_parent(&self.paths, name, &forked, parent.as_deref()) {
                        Ok(path) => self.set_ok(format!(
                            "Forked first {turns} turn(s) as {name} ({kept} messages, {})",
                            path.display()
                        )),
                        Err(error) => self.set_error(format!("Session fork failed: {error}")),
                    }
                    return;
                }
                match session::save_with_parent(&self.paths, name, &self.state, parent.as_deref()) {
                    Ok(path) => {
                        self.set_ok(format!(
                            "Forked current session as {name} ({}); keep working here or /session load {name}",
                            path.display()
                        ));
                    }
                    Err(error) => self.set_error(format!("Session fork failed: {error}")),
                }
            }
            Some("tree") => {
                self.push_system(&session::tree(&self.paths));
            }
            _ => self
                .set_status("Usage: /session save|load|list|search|delete|diff|fork|tree".into()),
        }
    }

    /// `/rewind [n]`: drop the last n user turns after a checkpoint
    /// backup. Unlike `/undo` the composer is left alone; `/redo`
    /// re-applies the dropped turns and `/session load <backup>`
    /// restores the pre-rewind transcript.
    pub(crate) fn command_rewind(&mut self, args: &[String]) {
        if self.busy {
            self.set_error("Finish the active request before rewinding".into());
            return;
        }
        let turns = args
            .first()
            .map(String::as_str)
            .unwrap_or("1")
            .parse::<usize>()
            .unwrap_or(0);
        if turns == 0 {
            self.set_error("Usage: /rewind [turns]".into());
            return;
        }
        let mut index = None;
        let mut seen = 0;
        for (position, message) in self.state.history.iter().enumerate().rev() {
            if message.role == "user" {
                seen += 1;
                if seen == turns {
                    index = Some(position);
                    break;
                }
            }
        }
        let Some(index) = index else {
            self.set_error(if seen == 0 {
                "Nothing to rewind".into()
            } else {
                format!("Only {seen} user turn(s) in the session")
            });
            return;
        };
        // Backup first: without it a rewind is unrecoverable.
        let backup = match session::save_checkpoint(
            &self.paths,
            &self.state,
            "rewind",
            self.current_session.as_deref(),
        ) {
            Ok(name) => name,
            Err(error) => {
                self.set_error(format!("Rewind backup failed, session kept: {error}"));
                return;
            }
        };
        let removed: Vec<Message> = self.state.history.drain(index..).collect();
        let count = removed.len();
        self.redo_stack
            .push(crate::ui::events::UndoEntry { messages: removed });
        self.prune_sections();
        self.follow_transcript = true;
        self.push_system(&format!(
            "Rewound {count} message(s) · backup {backup} · /redo to re-apply, /session load {backup} to restore"
        ));
        self.set_ok(format!("Rewound {count} message(s); backup {backup}"));
    }

    pub(crate) fn command_undo(&mut self) {
        if self.busy {
            self.set_error("Finish the active request before undoing".into());
            return;
        }
        let Some(index) = self
            .state
            .history
            .iter()
            .rposition(|message| message.role == "user")
        else {
            self.set_error("Nothing to undo".into());
            return;
        };
        let removed: Vec<Message> = self.state.history.drain(index..).collect();
        let prompt = removed.first().map(|item| item.content.clone());
        let Some(prompt) = prompt else {
            self.set_error("Nothing to undo".into());
            return;
        };
        let count = removed.len();
        self.redo_stack
            .push(crate::ui::events::UndoEntry { messages: removed });
        self.prune_sections();
        self.input = prompt;
        self.cursor = self.input.len();
        self.at_cache_key.clear();
        self.follow_transcript = true;
        self.set_ok(format!(
            "Undid {count} message(s); prompt restored · /redo to re-apply"
        ));
    }

    pub(crate) fn command_redo(&mut self) {
        if self.busy {
            self.set_error("Finish the active request before redoing".into());
            return;
        }
        let Some(entry) = self.redo_stack.pop() else {
            self.set_error("Nothing to redo".into());
            return;
        };
        let count = entry.messages.len();
        self.state.history.extend(entry.messages);
        self.follow_transcript = true;
        self.set_ok(format!("Redid {count} message(s)"));
    }

    /// `/filter <block> <pattern> [flags]`: store a per-block output
    /// filter (spec 0017) so long tool/command output can be narrowed
    /// without losing the block. Bare `/filter` lists active filters.
    pub(crate) fn command_filter(&mut self, args: &[String]) {
        let action = match parse_block_filter(self.state.history.len(), args) {
            Ok(action) => action,
            Err(error) => {
                self.set_error(error);
                return;
            }
        };
        match action {
            FilterAction::List => {
                let active: Vec<String> = self
                    .state
                    .history
                    .iter()
                    .enumerate()
                    .filter_map(|(index, message)| {
                        self.block_filters.get(&message.id).map(|filter| {
                            format!(
                                "  block {} ({}) · {}",
                                index + 1,
                                message.role,
                                filter.describe()
                            )
                        })
                    })
                    .collect();
                if active.is_empty() {
                    self.set_status(format!("No block filters active · {FILTER_USAGE}"));
                } else {
                    self.push_system(&format!("Block filters\n{}", active.join("\n")));
                }
            }
            FilterAction::Set(index, filter) => {
                let id = self.section_id(index);
                let summary = filter.describe();
                self.block_filters.insert(id, filter);
                self.set_ok(format!("Filter block {} · {summary}", index + 1));
            }
            FilterAction::Clear(index) => {
                let id = self.section_id(index);
                if self.block_filters.remove(&id).is_some() {
                    self.set_ok(format!("Filter cleared on block {}", index + 1));
                } else {
                    self.set_status(format!("Block {} has no filter", index + 1));
                }
            }
        }
    }

    /// `/block [n]`: list the tail of the transcript with block numbers,
    /// or describe one block (role, size, filter, first line).
    pub(crate) fn command_block(&mut self, args: &[String]) {
        const LIST_LIMIT: usize = 40;
        let Some(first) = args.first() else {
            if self.state.history.is_empty() {
                self.set_status("Session is empty".into());
                return;
            }
            let start = self.state.history.len().saturating_sub(LIST_LIMIT);
            let mut lines = vec![format!(
                "Blocks {}-{} of {}",
                start + 1,
                self.state.history.len(),
                self.state.history.len()
            )];
            for (index, message) in self.state.history.iter().enumerate().skip(start) {
                let preview: String = message
                    .content
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect();
                let filtered = if self.block_filters.contains_key(&message.id) {
                    " ·filtered"
                } else {
                    ""
                };
                lines.push(format!(
                    " {:>3} {:<9} {preview}{filtered}",
                    index + 1,
                    message.role
                ));
            }
            self.push_system(&lines.join("\n"));
            return;
        };
        let number: usize = match parse_block_number(first) {
            Some(number) => number,
            None => {
                self.set_status("Usage: /block [n]".into());
                return;
            }
        };
        if number < 1 || number > self.state.history.len() {
            self.set_error(format!(
                "Block {number} out of range (1-{})",
                self.state.history.len()
            ));
            return;
        }
        let index = number - 1;
        let id = self.section_id(index);
        let message = &self.state.history[index];
        let chars = message.content.chars().count();
        let lines = message.content.lines().count();
        let filter = match self.block_filters.get(&id) {
            Some(filter) => filter.describe(),
            None => "none".to_string(),
        };
        let first_line: String = message
            .content
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(100)
            .collect();
        self.push_system(&format!(
            "Block {number} · {} · {chars} chars · {lines} line(s) · filter: {filter}\n  {first_line}",
            message.role
        ));
    }

    /// `/rerun [n]`: resubmit an earlier user block. Prompts and `!`
    /// shell lines run again immediately; `/` commands prefill the
    /// composer instead, because replaying `/clear` or `/exit` is not
    /// what rerun should mean.
    pub(crate) async fn command_rerun(&mut self, args: &[String]) -> Result<()> {
        if self.busy {
            self.set_error("Finish the active request before rerunning".into());
            return Ok(());
        }
        let arg = match args.first() {
            Some(text) => match parse_block_number(text) {
                Some(number) => Some(number),
                None => {
                    self.set_status("Usage: /rerun [n]".into());
                    return Ok(());
                }
            },
            None => None,
        };
        let prompt = match rerun_target(&self.state.history, arg) {
            Ok(prompt) => prompt,
            Err(error) => {
                self.set_error(error);
                return Ok(());
            }
        };
        self.input = prompt;
        self.cursor = self.input.len();
        self.at_cache_key.clear();
        if self.input.starts_with('/') {
            self.set_status("Rerun prefilled — Enter re-dispatches the command".into());
            return Ok(());
        }
        // Boxed: rerun -> submit -> handle_command -> rerun is a cycle
        // the compiler cannot size. Slash blocks return above, so the
        // runtime depth is one.
        Box::pin(self.submit()).await
    }

    /// Any new user-authored turn invalidates the redo stack.
    pub(crate) fn command_editor(&mut self) {
        let editor = std::env::var("EDITOR").unwrap_or_default();
        if editor.trim().is_empty() {
            self.set_status("Set $EDITOR to compose prompts externally (e.g. EDITOR=nvim)".into());
            return;
        }
        self.editor_requested = true;
    }

    pub(crate) fn command_thinking(&mut self, args: &[String]) {
        self.state.show_thinking = toggle_value(args.first(), self.state.show_thinking);
        let enabled = self.state.show_thinking;
        self.persist_config(|config| config.show_thinking = enabled);
        self.set_ok(format!(
            "Thinking {}",
            if enabled { "shown" } else { "hidden" }
        ));
    }

    pub(crate) fn command_attention(&mut self, args: &[String]) {
        self.attention_bell = toggle_value(args.first(), self.attention_bell);
        let enabled = self.attention_bell;
        self.persist_config(|config| config.attention_bell = enabled);
        self.set_ok(format!("Bell {}", if enabled { "on" } else { "off" }));
    }

    pub(crate) fn set_mouse(&mut self, enabled: bool) {
        self.mouse_enabled = enabled;
        self.persist_config(|config| config.mouse = enabled);
        if enabled {
            let _ = execute!(io::stdout(), EnableMouseCapture);
            self.set_ok("Mouse on · Shift+drag selects".into());
        } else {
            let _ = execute!(io::stdout(), DisableMouseCapture);
            self.set_ok("Mouse off".into());
        }
    }

    pub(crate) fn command_mouse(&mut self, args: &[String]) {
        let enabled = toggle_value(args.first(), self.mouse_enabled);
        self.set_mouse(enabled);
    }

    /// Saved reusable workflows: the same Markdown commands as
    /// `/commands`, presented as runnable workflows. `reload` refreshes
    /// both scopes first.
    pub(crate) fn command_workflows(&mut self, args: &[String]) {
        if args.first().is_some_and(|action| action == "reload") {
            self.refresh_custom_commands();
        }
        if self.custom_commands.is_empty() {
            self.push_system(
                "No saved workflows yet. Add name.md files to the commands directories, then /workflows reload.",
            );
            return;
        }
        let mut lines = vec!["Saved workflows".to_string(), String::new()];
        for command in &self.custom_commands {
            lines.push(format!("/{:<18} {}", command.name, command.description));
        }
        lines.push(String::new());
        lines.push("Run with /<name> [args] · Ctrl+P to find".to_string());
        self.push_system(&lines.join("\n"));
    }

    pub(crate) fn command_export(&mut self, args: &[String]) {
        let format = args.first().map(String::as_str).unwrap_or("markdown");
        let extension = match format {
            "md" | "markdown" => "md",
            "txt" | "text" => "txt",
            "json" => "json",
            "html" => "html",
            "pdf" => "pdf",
            _ => {
                self.set_status(format!("Unsupported export format: {format}"));
                return;
            }
        };
        let path = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
            self.state
                .workspace
                .join(format!("r105-export.{extension}"))
        });
        let path = if path.is_absolute() {
            path
        } else {
            match safe_path(&self.state.workspace, &path.to_string_lossy()) {
                Ok(path) => path,
                Err(error) => {
                    self.set_error(format!("Export path rejected: {error}"));
                    return;
                }
            }
        };
        match export::write(&self.state, format, &path) {
            Ok(()) => self.set_ok(format!("Exported {}", path.display())),
            Err(error) => self.set_error(format!("Export failed: {error}")),
        }
    }

    pub(crate) fn command_theme(&mut self, args: &[String]) {
        if let Some(theme) = args.first() {
            if THEMES.contains(&theme.as_str()) {
                self.state.theme = theme.clone();
                self.persist_config(|config| config.theme = theme.clone());
                self.set_ok(format!("Theme: {theme}"));
            } else {
                self.set_error(format!("Unknown theme; choose {}", THEMES.join(", ")));
            }
            return;
        }
        let selected = THEMES
            .iter()
            .position(|name| *name == self.state.theme)
            .unwrap_or(0);
        self.overlay = Overlay::Theme {
            selected,
            original: self.state.theme.clone(),
        };
        self.set_status("Choose a theme — preview is live, Enter keeps it, Esc reverts".into());
    }

    pub(crate) fn workspace_map(&self) -> String {
        let mut lines = Vec::new();
        for entry in walkdir::WalkDir::new(&self.state.workspace)
            .max_depth(3)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != self.state.workspace)
            .take(80)
        {
            let relative = entry
                .path()
                .strip_prefix(&self.state.workspace)
                .unwrap_or(entry.path());
            lines.push(format!(
                "{} {}",
                if entry.file_type().is_dir() {
                    "▸"
                } else {
                    "·"
                },
                relative.display()
            ));
        }
        if lines.is_empty() {
            "Workspace is empty".into()
        } else {
            lines.join("\n")
        }
    }

    pub(crate) fn workspace_diff(&self) -> String {
        match OsCommand::new("git")
            .args([
                "-C",
                &self.state.workspace.to_string_lossy(),
                "diff",
                "--stat",
            ])
            .output()
        {
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if value.is_empty() {
                    "No Git changes".into()
                } else {
                    value
                }
            }
            Ok(output) => String::from_utf8_lossy(&output.stderr).trim().to_string(),
            Err(error) => format!("git diff unavailable: {error}"),
        }
    }

    pub(crate) fn settings_rows(&self) -> Vec<(String, String)> {
        vec![
            ("Theme".into(), self.state.theme.clone()),
            ("Permissions".into(), self.state.permission_posture.clone()),
            ("Reasoning".into(), self.state.reasoning_effort.clone()),
            (
                "Autocompact".into(),
                if self.state.auto_compact {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
            (
                "Show thinking".into(),
                if self.state.show_thinking {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
            (
                "Bell".into(),
                if self.attention_bell {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
            (
                "Mouse".into(),
                if self.mouse_enabled {
                    "on".into()
                } else {
                    "off".into()
                },
            ),
        ]
    }

    pub(crate) fn cycle_setting(&mut self, index: usize, direction: i32) {
        match index {
            0 => {
                let position = THEMES
                    .iter()
                    .position(|name| *name == self.state.theme)
                    .unwrap_or(0);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(THEMES.len() - 1)
                };
                let theme = THEMES[next].to_string();
                self.state.theme = theme.clone();
                self.persist_config(|config| config.theme = theme.clone());
                self.set_ok(format!("Theme: {theme}"));
            }
            1 => {
                const VALID: [&str; 4] = ["full-access", "restricted", "sandboxed", "off"];
                let position = VALID
                    .iter()
                    .position(|name| *name == self.state.permission_posture)
                    .unwrap_or(2);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(VALID.len() - 1)
                };
                let value = VALID[next].to_string();
                self.state.permission_posture = value.clone();
                self.persist_config(|config| config.permission_posture = value.clone());
                self.set_ok(format!("Permissions: {value}"));
            }
            2 => {
                const VALID: [&str; 5] = ["auto", "off", "low", "medium", "high"];
                let position = VALID
                    .iter()
                    .position(|name| *name == self.state.reasoning_effort)
                    .unwrap_or(0);
                let next = if direction < 0 {
                    position.saturating_sub(1)
                } else {
                    (position + 1).min(VALID.len() - 1)
                };
                let value = VALID[next].to_string();
                self.state.reasoning_effort = value.clone();
                self.persist_config(|config| config.reasoning_effort = value.clone());
                self.set_ok(format!("Reasoning: {value}"));
            }
            3 => {
                self.state.auto_compact = !self.state.auto_compact;
                let enabled = self.state.auto_compact;
                self.persist_config(|config| config.auto_compact = enabled);
                self.set_status(format!(
                    "Autocompact {}",
                    if enabled { "on" } else { "off" }
                ));
            }
            4 => {
                self.state.show_thinking = !self.state.show_thinking;
                let enabled = self.state.show_thinking;
                self.persist_config(|config| config.show_thinking = enabled);
                self.set_ok(format!(
                    "Thinking {}",
                    if enabled { "shown" } else { "hidden" }
                ));
            }
            5 => {
                self.attention_bell = !self.attention_bell;
                let enabled = self.attention_bell;
                self.persist_config(|config| config.attention_bell = enabled);
                self.set_ok(format!("Bell {}", if enabled { "on" } else { "off" }));
            }
            6 => {
                self.set_mouse(!self.mouse_enabled);
            }
            _ => {}
        }
    }
}

pub(crate) fn git_branch_for(workspace: &Path) -> Option<String> {
    let output = OsCommand::new("git")
        .args([
            "-C",
            &workspace.to_string_lossy(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
}

/// Global custom-command directory: `commands/` under the config root,
/// next to `skills/` and `sessions/`.
pub(crate) fn commands_dir(paths: &ConfigPaths) -> PathBuf {
    paths.config_dir.join("commands")
}

/// Project custom-command directory inside the active workspace.
pub(crate) fn project_commands_dir(workspace: &Path) -> PathBuf {
    workspace.join(".r105").join("commands")
}

pub(crate) fn count_skills(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(|value| value != '.')
                })
                .count()
        })
        .unwrap_or(0)
}

pub(crate) fn tool_names(names: &[String]) -> String {
    const MAX_SHOWN: usize = 3;
    const MAX_CHARS: usize = 80;
    if names.is_empty() {
        return "none".into();
    }
    let mut text = names
        .iter()
        .take(MAX_SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > MAX_SHOWN {
        text.push_str(&format!(", +{} more", names.len() - MAX_SHOWN));
    }
    if text.len() > MAX_CHARS {
        text.truncate(MAX_CHARS);
        text.push('…');
    }
    text
}

pub(crate) fn provider_label(preset: &Preset) -> String {
    format!(
        "{}  ·  {}  [{}]",
        preset.label, preset.description, preset.id
    )
}

pub(crate) fn toggle_value(value: Option<&String>, current: bool) -> bool {
    match value.map(|value| value.to_ascii_lowercase()).as_deref() {
        None => !current,
        Some("on" | "true" | "yes" | "1") => true,
        Some("off" | "false" | "no" | "0") => false,
        Some(_) => current,
    }
}

pub(crate) fn copy_to_clipboard(value: &str) -> bool {
    let commands: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("clip", &[]),
    ];
    for (program, args) in commands {
        let Ok(mut child) = OsCommand::new(program)
            .args(*args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let Some(mut stdin) = child.stdin.take() else {
            continue;
        };
        if stdin.write_all(value.as_bytes()).is_err() {
            let _ = child.kill();
            continue;
        }
        drop(stdin);
        if child.wait().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    false
}

pub(crate) fn skill_name(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.contains('/')
        || trimmed.contains('\\')
    {
        return None;
    }
    let name = if trimmed.ends_with(".md") {
        trimmed.strip_suffix(".md")?.to_string()
    } else {
        trimmed.to_string()
    };
    let path = Path::new(&name);
    (path.components().count() == 1
        && path
            .components()
            .next()
            .is_some_and(|component| matches!(component, std::path::Component::Normal(_))))
    .then_some(name)
}

pub(crate) fn extract_models(value: &Value) -> Vec<ModelInfo> {
    let source = value.get("data").or_else(|| value.get("models"));
    let Some(items) = source.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut models: Vec<ModelInfo> = Vec::new();
    for item in items {
        let (id, status, context) = match item {
            Value::String(value) => (Some(value.clone()), None, None),
            Value::Object(object) => {
                let id = object
                    .get("id")
                    .or_else(|| object.get("name"))
                    .or_else(|| object.get("model"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let status = object.get("status").and_then(|status| match status {
                    Value::String(value) => Some(value.clone()),
                    Value::Object(object) => object
                        .get("value")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    _ => None,
                });
                (id, status, model_context(object))
            }
            _ => (None, None, None),
        };
        if let Some(id) = id
            && !models.iter().any(|model| model.id == id)
        {
            models.push(ModelInfo {
                id,
                status,
                context,
            });
        }
    }
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models
}

/// Context window from model metadata. Providers disagree on the field
/// name, so the common shapes are accepted: OpenRouter reports
/// `context_length` (bare and under `top_provider`), vLLM `max_model_len`,
/// llama.cpp `meta.n_ctx_train`, and some serve `context_window` or
/// `n_ctx`. First positive value wins.
fn model_context(object: &serde_json::Map<String, Value>) -> Option<u64> {
    const DIRECT: &[&str] = &[
        "context_length",
        "context_window",
        "max_context_length",
        "max_model_len",
        "n_ctx",
        "n_ctx_train",
    ];
    const NESTED: &[(&str, &str)] = &[
        ("meta", "n_ctx"),
        ("meta", "n_ctx_train"),
        ("top_provider", "context_length"),
    ];
    DIRECT
        .iter()
        .find_map(|key| positive_integer(object.get(*key)))
        .or_else(|| {
            NESTED.iter().find_map(|(parent, key)| {
                object
                    .get(*parent)
                    .and_then(Value::as_object)
                    .and_then(|inner| positive_integer(inner.get(*key)))
            })
        })
}

fn positive_integer(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|value| value as u64))
            .filter(|value| *value > 0),
        Value::String(text) => text.trim().parse::<u64>().ok().filter(|value| *value > 0),
        _ => None,
    }
}
