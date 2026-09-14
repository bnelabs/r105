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

pub fn help_text() -> String {
    let mut output = String::from("Commands\n\n");
    for item in COMMANDS {
        output.push_str(&format!("  {:<42} {}\n", item.usage, item.description));
    }
    output.push_str(
        "\nKeys\n  Enter send (steer while busy)   Alt/Shift+Enter newline   Tab mode/complete   Esc cancel\n  Ctrl+C quit/cancel   Ctrl+X cancel   Ctrl+O details   Ctrl+T tasks   Ctrl+R history hint\n  Up/Down history or file picks   @file attach file context   !cmd run shell into context\n",
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
}
