//! Slash command registry and parser shared by the TUI and future CLI modes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Build,
    Plan,
    Ask,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Plan => "plan",
            Self::Ask => "ask",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub usage: &'static str,
    pub description: &'static str,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "/",
        usage: "/",
        description: "show commands and keybindings",
    },
    CommandSpec {
        name: "/help",
        usage: "/help [command]",
        description: "show commands and keybindings",
    },
    CommandSpec {
        name: "/state",
        usage: "/state",
        description: "show active settings and connection",
    },
    CommandSpec {
        name: "/history",
        usage: "/history",
        description: "show a transcript preview",
    },
    CommandSpec {
        name: "/connect",
        usage: "/connect",
        description: "choose provider, API key, and model",
    },
    CommandSpec {
        name: "/provider",
        usage: "/provider",
        description: "alias for guided provider setup",
    },
    CommandSpec {
        name: "/models",
        usage: "/models",
        description: "refresh and choose available models",
    },
    CommandSpec {
        name: "/model",
        usage: "/model [name]",
        description: "show or switch the active model",
    },
    CommandSpec {
        name: "/health",
        usage: "/health",
        description: "check backend connectivity",
    },
    CommandSpec {
        name: "/profiles",
        usage: "/profiles",
        description: "list llama-router profiles",
    },
    CommandSpec {
        name: "/profile",
        usage: "/profile [name|auto]",
        description: "set the llama-router profile",
    },
    CommandSpec {
        name: "/plan",
        usage: "/plan",
        description: "switch to planning mode",
    },
    CommandSpec {
        name: "/build",
        usage: "/build",
        description: "switch to implementation mode",
    },
    CommandSpec {
        name: "/ask",
        usage: "/ask",
        description: "switch to question mode",
    },
    CommandSpec {
        name: "/skills",
        usage: "/skills",
        description: "list available Markdown skills",
    },
    CommandSpec {
        name: "/skill",
        usage: "/skill <use|show|drop|clear> ...",
        description: "activate or inspect a skill",
    },
    CommandSpec {
        name: "/compact",
        usage: "/compact",
        description: "summarize older conversation context",
    },
    CommandSpec {
        name: "/tokens",
        usage: "/tokens",
        description: "show context usage and estimate confidence",
    },
    CommandSpec {
        name: "/quality",
        usage: "/quality [fast|balanced|best]",
        description: "set router quality hint",
    },
    CommandSpec {
        name: "/json",
        usage: "/json [on|off]",
        description: "toggle JSON response mode",
    },
    CommandSpec {
        name: "/max",
        usage: "/max [tokens]",
        description: "set or clear completion token limit",
    },
    CommandSpec {
        name: "/cache-prompt",
        usage: "/cache-prompt [on|off]",
        description: "toggle llama.cpp prompt caching",
    },
    CommandSpec {
        name: "/config",
        usage: "/config <show|reload>",
        description: "inspect or reload configuration",
    },
    CommandSpec {
        name: "/clear",
        usage: "/clear",
        description: "clear the visible transcript",
    },
    CommandSpec {
        name: "/diff",
        usage: "/diff",
        description: "show the current workspace diff",
    },
    CommandSpec {
        name: "/map",
        usage: "/map",
        description: "show a compact workspace map",
    },
    CommandSpec {
        name: "/workspace",
        usage: "/workspace [path]",
        description: "show or change the workspace",
    },
    CommandSpec {
        name: "/session",
        usage: "/session <save|load|list|search|delete|diff|fork> ...",
        description: "manage local sessions",
    },
    CommandSpec {
        name: "/export",
        usage: "/export <markdown|text|json|html> [path]",
        description: "export the transcript",
    },
    CommandSpec {
        name: "/mcp",
        usage: "/mcp <list|tools|reconnect>",
        description: "inspect configured MCP servers",
    },
    CommandSpec {
        name: "/plugin",
        usage: "/plugin <list|reload>",
        description: "inspect native Rust plugins",
    },
    CommandSpec {
        name: "/theme",
        usage: "/theme [name]",
        description: "pick a theme (live preview) or switch directly",
    },
    CommandSpec {
        name: "/autocompact",
        usage: "/autocompact [on|off]",
        description: "toggle automatic context compaction",
    },
    CommandSpec {
        name: "/reasoning",
        usage: "/reasoning [auto|off|low|medium|high]",
        description: "set reasoning effort hint",
    },
    CommandSpec {
        name: "/permissions",
        usage: "/permissions [full-access|restricted|sandboxed|off]",
        description: "set local tool permission posture",
    },
    CommandSpec {
        name: "/approve",
        usage: "/approve execute_python",
        description: "approve Python bridge execution for this run",
    },
    CommandSpec {
        name: "/preview",
        usage: "/preview <filename>",
        description: "preview a workspace file",
    },
    CommandSpec {
        name: "/bridge",
        usage: "/bridge",
        description: "show optional Python bridge status",
    },
    CommandSpec {
        name: "/copy",
        usage: "/copy [n]",
        description: "copy the last response or its nth code block",
    },
    CommandSpec {
        name: "/tasks",
        usage: "/tasks",
        description: "show active and queued work",
    },
    CommandSpec {
        name: "/retry",
        usage: "/retry",
        description: "retry the last failed prompt",
    },
    CommandSpec {
        name: "/undo",
        usage: "/undo",
        description: "remove the last exchange and restore its prompt",
    },
    CommandSpec {
        name: "/redo",
        usage: "/redo",
        description: "re-apply the last undone exchange",
    },
    CommandSpec {
        name: "/editor",
        usage: "/editor",
        description: "compose the prompt in $EDITOR",
    },
    CommandSpec {
        name: "/settings",
        usage: "/settings",
        description: "change theme, permissions, reasoning, and toggles",
    },
    CommandSpec {
        name: "/thinking",
        usage: "/thinking [on|off]",
        description: "show or hide the model's thinking blocks",
    },
    CommandSpec {
        name: "/attention",
        usage: "/attention [on|off]",
        description: "toggle the bell when a request finishes",
    },
    CommandSpec {
        name: "/commands",
        usage: "/commands [reload]",
        description: "list Markdown-backed custom commands",
    },
    CommandSpec {
        name: "/sh",
        usage: "/sh <request>",
        description: "draft a shell command from plain words",
    },
    CommandSpec {
        name: "/exit",
        usage: "/exit",
        description: "save and quit r105",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub name: String,
    pub args: Vec<String>,
}

pub fn parse(input: &str) -> Option<ParsedCommand> {
    let mut words = shell_words(input.trim());
    let name = words.next()?.to_ascii_lowercase();
    if !name.starts_with('/') {
        return None;
    }
    Some(ParsedCommand {
        name,
        args: words.collect(),
    })
}

pub fn command(name: &str) -> Option<&'static CommandSpec> {
    let normalized = if name.starts_with('/') {
        name.to_ascii_lowercase()
    } else {
        format!("/{name}")
    };
    COMMANDS.iter().find(|item| item.name == normalized)
}

pub fn filtered(query: &str) -> Vec<&'static CommandSpec> {
    let query = query.trim().to_ascii_lowercase();
    let mut scored: Vec<(i32, &CommandSpec)> = COMMANDS
        .iter()
        .filter_map(|item| fuzzy_score(item.name, &query).map(|score| (score, item)))
        .collect();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.name.cmp(right.name))
    });
    scored.into_iter().map(|(_, item)| item).collect()
}

/// One palette row: a built-in command or a Markdown-backed custom one.
/// `custom` marks file-defined rows so the TUI can flag them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteItem {
    pub name: String,
    pub description: String,
    pub custom: bool,
}

/// Built-in matches first (existing order), then custom commands sorted by
/// name. Built-ins always win a name collision: shadowed files never appear
/// here and are reported by `/commands` instead.
pub fn palette_items(query: &str, customs: &[crate::custom::CustomCommand]) -> Vec<PaletteItem> {
    let mut items: Vec<PaletteItem> = filtered(query)
        .into_iter()
        .map(|item| PaletteItem {
            name: item.name.into(),
            description: item.description.into(),
            custom: false,
        })
        .collect();
    let query = query.trim().to_ascii_lowercase();
    let mut customs: Vec<PaletteItem> = customs
        .iter()
        .filter(|command| {
            !COMMANDS
                .iter()
                .any(|item| item.name == format!("/{}", command.name))
        })
        .filter(|command| fuzzy_score(&format!("/{}", command.name), &query).is_some())
        .map(|command| PaletteItem {
            name: format!("/{}", command.name),
            description: command.description.clone(),
            custom: true,
        })
        .collect();
    customs.sort_by(|left, right| left.name.cmp(&right.name));
    items.append(&mut customs);
    items
}

/// Closest command name for an unknown `/input`, across built-ins and
/// customs. Only confident matches (prefix/substring or a strong fuzzy
/// score) are suggested so typos get help and gibberish stays silent.
pub fn suggest(input: &str, customs: &[crate::custom::CustomCommand]) -> Option<String> {
    let query = input.trim().to_ascii_lowercase();
    if query.len() < 2 || !query.starts_with('/') {
        return None;
    }
    let mut best: Option<(i32, String)> = None;
    let mut consider = |name: &str| {
        let score = fuzzy_score(name, &query)
            .filter(|score| *score >= 60)
            .or_else(|| {
                edit_distance(name, &query)
                    .and_then(|distance| (distance <= 2).then(|| 50 - distance as i32 * 10))
            });
        if let Some(score) = score
            && best.as_ref().is_none_or(|(best_score, best_name)| {
                // Equal evidence prefers the shorter name: the more general
                // command is the safer guess (`/modell` → `/model`).
                score > *best_score || (score == *best_score && name.len() < best_name.len())
            })
        {
            best = Some((score, name.into()));
        }
    };
    for item in COMMANDS {
        consider(item.name);
    }
    for command in customs {
        consider(&format!("/{}", command.name));
    }
    best.map(|(_, name)| name)
}

/// Fixed value sets for first-argument completion (`/theme <Tab>`).
/// Returns `None` for commands with free-form or dynamic arguments.
pub fn static_arg_values(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "/theme" => Some(&crate::config::THEMES),
        "/quality" => Some(&["fast", "balanced", "best"]),
        "/reasoning" => Some(&["auto", "off", "low", "medium", "high"]),
        "/permissions" => Some(&["full-access", "restricted", "sandboxed", "off"]),
        "/json" | "/cache-prompt" | "/autocompact" | "/thinking" | "/attention" => {
            Some(&["on", "off"])
        }
        "/profile" => Some(&[
            "auto",
            "simple",
            "strict_json",
            "coding",
            "complex_reasoning",
            "long_context_qa",
            "tool_agent",
            "creative",
        ]),
        "/session" => Some(&["save", "load", "list", "search", "delete", "diff", "fork"]),
        "/export" => Some(&["markdown", "text", "json", "html", "md", "txt", "pdf"]),
        "/mcp" => Some(&["list", "tools", "reconnect"]),
        "/plugin" => Some(&["list", "reload"]),
        "/skill" => Some(&["use", "show", "drop", "clear"]),
        "/config" => Some(&["show", "reload"]),
        "/approve" => Some(&["execute_python"]),
        "/connect" => Some(&["status", "show", "url", "custom"]),
        _ => None,
    }
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let candidate = candidate.to_ascii_lowercase();
    if candidate == query {
        return Some(1000);
    }
    if candidate.starts_with(query) {
        return Some(800 - candidate.len() as i32);
    }
    if candidate.contains(query) {
        return Some(500 - candidate.len() as i32);
    }
    let mut position = 0usize;
    let mut score = 0;
    for needle in query.chars() {
        let relative = candidate[position..].find(needle)?;
        score += 20 - relative.min(19) as i32;
        position += relative + needle.len_utf8();
    }
    Some(score)
}

/// Classic Levenshtein edit distance over chars. Only used for
/// did-you-mean ranking across ~50 command names, so the quadratic table
/// is negligible and there is no new dependency.
fn edit_distance(left: &str, right: &str) -> Option<usize> {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (index, left_char) in left.iter().enumerate() {
        let mut next = vec![index + 1];
        for (other, right_char) in right.iter().enumerate() {
            let cost = usize::from(left_char != right_char);
            next.push(
                (row[other] + cost)
                    .min(row[other + 1] + 1)
                    .min(next[other] + 1),
            );
        }
        row = next;
    }
    Some(row[right.len()])
}

/// Keep a highlighted command inside the visible palette window. The helper
/// is deliberately independent of terminal size so Windows and Unix renderers
/// share the same scrolling behavior.
pub fn ensure_visible(selected: usize, scroll: usize, viewport: usize, len: usize) -> usize {
    if len == 0 || viewport == 0 {
        return 0;
    }
    let selected = selected.min(len - 1);
    let max_scroll = len.saturating_sub(viewport);
    let desired = if selected < scroll {
        selected
    } else if selected >= scroll + viewport {
        selected + 1 - viewport
    } else {
        scroll
    };
    desired.min(max_scroll)
}

/// Full help dump plus a custom-commands section when any are loaded.
/// `*` marks file-defined rows, matching the palette marker.
/// Pass an empty slice for the built-ins-only dump.
pub fn help_text_with(customs: &[crate::custom::CustomCommand]) -> String {
    let mut output = String::from("Commands\n\n");
    for item in COMMANDS {
        output.push_str(&format!("  {:<42} {}\n", item.usage, item.description));
    }
    let mut customs: Vec<&crate::custom::CustomCommand> = customs
        .iter()
        .filter(|custom| command(&format!("/{}", custom.name)).is_none())
        .collect();
    customs.sort_by(|left, right| left.name.cmp(&right.name));
    if !customs.is_empty() {
        output.push_str("\nCustom commands (*)\n\n");
        for command in customs {
            output.push_str(&format!(
                "  /{:<41} {} ({})\n",
                command.name, command.description, command.source
            ));
        }
    }
    output.push_str(
        "\nKeys\n  Enter send (steer while busy)   Alt/Shift+Enter newline   Tab mode/complete   Esc cancel\n  Ctrl+C quit/cancel   Ctrl+X cancel   Ctrl+O details   Ctrl+T tasks   Ctrl+R history hint\n  Up/Down history or file picks   @file attach file context   !cmd run shell into context   /sh draft shell from words\n",
    );
    output
}

fn shell_words(input: &str) -> impl Iterator<Item = String> + '_ {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    for character in input.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match (quote, character) {
            (_, '\\') => escaped = true,
            (Some(active), value) if value == active => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, value) if value.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            (_, value) => current.push(value),
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_arguments() {
        assert_eq!(
            parse("/session save \"night run\"").unwrap(),
            ParsedCommand {
                name: "/session".into(),
                args: vec!["save".into(), "night run".into()]
            }
        );
    }

    #[test]
    fn palette_selection_scrolls_at_bottom() {
        assert_eq!(ensure_visible(9, 0, 4, 10), 6);
        assert_eq!(ensure_visible(2, 6, 4, 10), 2);
        assert_eq!(ensure_visible(0, 5, 4, 10), 0);
    }

    #[test]
    fn fuzzy_commands_prioritize_exact_prefix() {
        assert_eq!(filtered("/conn")[0].name, "/connect");
    }

    #[test]
    fn review_followup_commands_are_registered() {
        for name in [
            "/undo",
            "/redo",
            "/editor",
            "/settings",
            "/thinking",
            "/attention",
        ] {
            assert!(command(name).is_some(), "{name} is registered");
        }
    }

    #[test]
    fn suggest_fixes_common_typos_and_ignores_gibberish() {
        // Transposed, added, and doubled characters all rank within distance 2.
        assert_eq!(suggest("/modle", &[]).as_deref(), Some("/model"));
        assert_eq!(suggest("/modell", &[]).as_deref(), Some("/model"));
        assert_eq!(suggest("/theem", &[]).as_deref(), Some("/theme"));
        assert_eq!(suggest("/xyz", &[]), None);
        assert_eq!(suggest("/", &[]), None);
        let customs = vec![crate::custom::CustomCommand {
            name: "deploy".into(),
            description: "Ship it".into(),
            argument_hint: None,
            content: "Deploy $1".into(),
            source: "user".into(),
            path: std::path::PathBuf::from("/tmp/deploy.md"),
        }];
        assert_eq!(suggest("/deply", &customs).as_deref(), Some("/deploy"));
    }

    #[test]
    fn palette_merges_custom_commands_after_builtins() {
        let customs = vec![crate::custom::CustomCommand {
            name: "review".into(),
            description: "Review staged work".into(),
            argument_hint: None,
            content: "Review $1".into(),
            source: "user".into(),
            path: std::path::PathBuf::from("/tmp/review.md"),
        }];
        let items = palette_items("/rev", &customs);
        assert!(
            items
                .iter()
                .any(|item| item.name == "/review" && item.custom)
        );
        assert!(
            items
                .iter()
                .all(|item| !item.custom || item.name == "/review")
        );
        // Built-in collisions never surface as custom rows.
        let shadowed = vec![crate::custom::CustomCommand {
            name: "model".into(),
            description: "shadow".into(),
            argument_hint: None,
            content: "x".into(),
            source: "user".into(),
            path: std::path::PathBuf::from("/tmp/model.md"),
        }];
        let items = palette_items("/model", &shadowed);
        assert!(items.iter().all(|item| !item.custom));
    }

    #[test]
    fn static_arg_tables_cover_choice_commands() {
        assert_eq!(
            static_arg_values("/reasoning"),
            Some(&["auto", "off", "low", "medium", "high"][..])
        );
        assert!(static_arg_values("/theme").is_some());
        assert!(static_arg_values("/max").is_none());
        assert!(static_arg_values("/sh").is_none());
    }
}
