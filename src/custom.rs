//! Zero-code slash commands from Markdown files.
//!
//! A `commands/` directory holding `name.md` files turns each file into a
//! `/name` command: the body becomes the prompt, with `$1`, `$@` and friends
//! substituted from the typed arguments. Two scopes are merged, mirroring
//! how skills resolve: a global directory under the config root and a
//! project directory (`.r105/commands`) under the active workspace. The
//! global scope wins on duplicate names; built-in commands always win over
//! both and shadowed files are reported, never silently run.

use std::path::{Path, PathBuf};

/// One Markdown-backed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommand {
    /// Lowercased command name without the leading `/` (the file stem).
    pub name: String,
    /// One-line description: frontmatter `description`, else the first
    /// non-empty body line truncated to 60 chars.
    pub description: String,
    /// Frontmatter `argument-hint`, shown by `/help /name`.
    pub argument_hint: Option<String>,
    /// Template body after frontmatter, with `$`-placeholders intact.
    pub content: String,
    /// Scope label for display (`user` or `project`).
    pub source: String,
    /// Absolute file path, for `/help` and diagnostics.
    pub path: PathBuf,
}

/// Scan `global_dir` then `project_dir` (non-recursive) for `*.md` files.
/// Later scopes do not override earlier ones; callers filter built-in
/// collisions so shadowing stays visible.
pub fn load_commands(global_dir: &Path, project_dir: &Path) -> Vec<CustomCommand> {
    let mut commands = Vec::new();
    load_dir(global_dir, "user", &mut commands);
    load_dir(project_dir, "project", &mut commands);
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    commands
}

fn load_dir(dir: &Path, source: &str, commands: &mut Vec<CustomCommand>) {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !valid_name(&stem) {
            continue;
        }
        if commands.iter().any(|command| command.name == stem) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (frontmatter, body) = split_frontmatter(&raw);
        let description = frontmatter
            .get("description")
            .cloned()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| first_line(&body));
        let content = body.trim().to_string();
        if content.is_empty() {
            continue;
        }
        commands.push(CustomCommand {
            name: stem,
            description,
            argument_hint: frontmatter
                .get("argument-hint")
                .cloned()
                .filter(|value| !value.is_empty()),
            content,
            source: source.into(),
            path,
        });
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// Split an optional leading `---` frontmatter block into `key: value`
/// pairs plus the remaining body. Malformed blocks degrade to "no
/// frontmatter" rather than failing the load.
fn split_frontmatter(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut frontmatter = std::collections::HashMap::new();
    let mut lines = raw.lines();
    if lines.next().is_some_and(|line| line.trim() == "---") {
        let mut body_start = None;
        let mut pairs = Vec::new();
        for (index, line) in lines.by_ref().enumerate() {
            if line.trim() == "---" {
                body_start = Some(index);
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_ascii_lowercase();
                if !key.is_empty() {
                    pairs.push((key, unquote(value.trim())));
                }
            }
        }
        if let Some(end) = body_start {
            for (key, value) in pairs {
                frontmatter.insert(key, value);
            }
            let body: String = raw.lines().skip(end + 2).collect::<Vec<_>>().join("\n");
            return (frontmatter, body);
        }
    }
    (frontmatter, raw.to_string())
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].into()
    } else {
        value.into()
    }
}

fn first_line(body: &str) -> String {
    match body.lines().map(str::trim).find(|line| !line.is_empty()) {
        None => String::new(),
        Some(line) if line.chars().count() > 60 => {
            format!("{}...", line.chars().take(60).collect::<String>())
        }
        Some(line) => line.into(),
    }
}

/// Substitute argument placeholders in template `content` (single pass, so
/// argument values containing `$`-patterns are never re-expanded):
/// `$1`…`$9` (multi-digit allowed), `$@`/`$ARGUMENTS` for all args,
/// `${N:-default}` for a positional with fallback, `${@:-default}` /
/// `${ARGUMENTS:-default}` for all args with fallback, `${@:N}` and
/// `${@:N:L}` for bash-style slices (1-indexed, `0` treated as `1`).
/// Malformed placeholders are left literal.
pub fn substitute_args(content: &str, args: &[String]) -> String {
    let mut output = String::with_capacity(content.len());
    let mut index = 0;
    while index < content.len() {
        if content.as_bytes()[index] != b'$' {
            let character = content[index..].chars().next().unwrap_or('$');
            output.push(character);
            index += character.len_utf8();
            continue;
        }
        let rest = &content[index + 1..];
        if let Some(after_brace) = rest.strip_prefix('{') {
            match parse_braced(after_brace, args) {
                Some((value, consumed)) => {
                    output.push_str(&value);
                    index += 1 + consumed; // `$` plus `{...}`
                }
                None => {
                    output.push('$');
                    index += 1;
                }
            }
            continue;
        }
        if rest.starts_with("ARGUMENTS") {
            output.push_str(&args.join(" "));
            index += 1 + "ARGUMENTS".len();
            continue;
        }
        if rest.starts_with('@') {
            output.push_str(&args.join(" "));
            index += 2; // `$@`
            continue;
        }
        let digits: String = rest
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            output.push('$');
            index += 1;
        } else {
            let position: usize = digits.parse().unwrap_or(usize::MAX);
            output.push_str(
                args.get(position.saturating_sub(1))
                    .map_or("", String::as_str),
            );
            index += 1 + digits.len();
        }
    }
    output
}

/// Parse a `{...}` placeholder from the text after the opening brace.
/// Returns the replacement plus how many bytes after `$` to consume
/// (`{...}` inclusive). Anything malformed is `None` so the caller leaves
/// the `$` literal and the braces untouched.
fn parse_braced(rest: &str, args: &[String]) -> Option<(String, usize)> {
    let close = rest.find('}')?;
    let inner = &rest[..close];
    let consumed = close + 2; // `{` plus `...}`
    if let Some((target, default)) = inner.split_once(":-") {
        if !valid_target(target) {
            return None;
        }
        let value = all_or_positional(target, args);
        return Some((
            if value.is_empty() {
                default.to_string()
            } else {
                value
            },
            consumed,
        ));
    }
    if let Some(slice) = inner.strip_prefix("@:") {
        let mut parts = slice.splitn(2, ':');
        let start: usize = parts.next()?.parse().ok()?;
        let selected: Vec<&str> = args
            .iter()
            .skip(start.saturating_sub(1))
            .map(String::as_str)
            .collect();
        let value = match parts.next() {
            None => selected.join(" "),
            Some(length) => {
                let length: usize = length.parse().ok()?;
                selected
                    .into_iter()
                    .take(length)
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        return Some((value, consumed));
    }
    if !valid_target(inner) {
        return None;
    }
    Some((all_or_positional(inner, args), consumed))
}

fn valid_target(target: &str) -> bool {
    target == "@"
        || target == "ARGUMENTS"
        || (!target.is_empty() && target.chars().all(|character| character.is_ascii_digit()))
}

fn all_or_positional(target: &str, args: &[String]) -> String {
    if target == "@" || target == "ARGUMENTS" {
        return args.join(" ");
    }
    match target.parse::<usize>() {
        Ok(position) => args
            .get(position.saturating_sub(1))
            .cloned()
            .unwrap_or_default(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn substitutes_positional_and_all() {
        assert_eq!(
            substitute_args("review $1 with $@", &args(&["a", "b"])),
            "review a with a b"
        );
        assert_eq!(
            substitute_args("all: $ARGUMENTS", &args(&["a", "b"])),
            "all: a b"
        );
    }

    #[test]
    fn missing_positional_is_empty_and_defaults_apply() {
        assert_eq!(substitute_args("x$2y", &args(&["a"])), "xy");
        assert_eq!(
            substitute_args("use ${2:-fallback}", &args(&["a"])),
            "use fallback"
        );
        assert_eq!(
            substitute_args("use ${1:-fallback}", &args(&["a"])),
            "use a"
        );
        assert_eq!(substitute_args("use ${@:-none}", &args(&[])), "use none");
    }

    #[test]
    fn slices_select_from_position() {
        assert_eq!(
            substitute_args("[${@:2}]", &args(&["a", "b", "c"])),
            "[b c]"
        );
        assert_eq!(
            substitute_args("[${@:2:1}]", &args(&["a", "b", "c"])),
            "[b]"
        );
    }

    #[test]
    fn malformed_placeholders_stay_literal() {
        assert_eq!(substitute_args("cost $", &args(&[])), "cost $");
        assert_eq!(substitute_args("${oops", &args(&[])), "${oops");
        assert_eq!(substitute_args("$ARGS", &args(&["a"])), "$ARGS");
    }

    #[test]
    fn substitution_is_not_recursive() {
        assert_eq!(substitute_args("$1", &args(&["$2"])), "$2");
    }

    #[test]
    fn loads_markdown_with_frontmatter() {
        let dir = std::env::temp_dir().join(format!("r105-custom-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("review.md"),
            "---\ndescription: Review staged work\nargument-hint: <scope>\n---\n\nReview $1\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        std::fs::write(dir.join("empty.md"), "---\n---\n").unwrap();
        let commands = load_commands(&dir, &dir.join("missing"));
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "review");
        assert_eq!(commands[0].description, "Review staged work");
        assert_eq!(commands[0].argument_hint.as_deref(), Some("<scope>"));
        assert_eq!(commands[0].content, "Review $1");
        assert_eq!(commands[0].source, "user");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn description_falls_back_to_first_line() {
        let (frontmatter, body) = split_frontmatter("Summarize this\nsecond line\n");
        assert!(frontmatter.is_empty());
        assert_eq!(first_line(&body), "Summarize this");
    }
}
