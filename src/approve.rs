//! Per-action approval policy: one pure `resolve` over mode, posture,
//! allow/deny lists, and per-category ask levels.
//!
//! The TUI pre-checks calls with `resolve` to show approval cards; `tool::execute`
//! re-runs it as the enforcement floor so no caller can bypass policy.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::Value;

use crate::config::Config;

/// Allow, ask, or deny as a configured per-category level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Ask,
    Deny,
}

impl Action {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "deny" => Ok(Self::Deny),
            other => bail!("invalid approval level '{other}' (want allow|ask|deny)"),
        }
    }
}

/// Policy outcome for a single tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Needs an interactive checkpoint; carries the card summary.
    Ask(String),
    /// Refused; carries the user-facing reason.
    Deny(String),
}

/// Risk category of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// `execute_rust`: compiles and runs code.
    Exec,
    /// `write_file`: mutates the workspace.
    Write,
    /// `read_file`, `list_files`: read-only inspection.
    Read,
    /// `web_search`, `web_fetch`: network access.
    Network,
    /// `mcp_*`: external servers.
    Mcp,
    /// `plugin_*`: local plugin code.
    Plugin,
    /// Pure builtins and `todo_write`: always allowed, never asked.
    Free,
}

pub fn category(name: &str) -> Category {
    match name {
        "execute_rust" => Category::Exec,
        "write_file" => Category::Write,
        "read_file" | "list_files" => Category::Read,
        "web_search" | "web_fetch" => Category::Network,
        _ if name.starts_with("mcp_") => Category::Mcp,
        _ if name.starts_with("plugin_") => Category::Plugin,
        _ => Category::Free,
    }
}

/// Approval policy: per-category levels plus regex lists matched against
/// `summarize(name, args)`. `session_allow` holds card-approved (`a`)
/// patterns for this run only; it is never persisted.
#[derive(Debug, Clone)]
pub struct Policy {
    pub exec: Action,
    pub write: Action,
    pub read: Action,
    pub network: Action,
    pub mcp: Action,
    pub plugin: Action,
    pub allowlist: Vec<Regex>,
    pub denylist: Vec<Regex>,
    pub session_allow: Vec<Regex>,
}

impl Default for Policy {
    /// Permissive: for tests and headless contexts without a config.
    /// Real runs build from config, where exec/write/mcp/plugin ask.
    fn default() -> Self {
        Self {
            exec: Action::Allow,
            write: Action::Allow,
            read: Action::Allow,
            network: Action::Allow,
            mcp: Action::Allow,
            plugin: Action::Allow,
            allowlist: Vec::new(),
            denylist: Vec::new(),
            session_allow: Vec::new(),
        }
    }
}

impl Policy {
    pub fn from_config(config: &Config) -> Result<Self> {
        let compile = |key: &str, patterns: &[String]| {
            patterns
                .iter()
                .map(|pattern| {
                    Regex::new(pattern)
                        .with_context(|| format!("invalid regex in {key}: {pattern}"))
                })
                .collect::<Result<Vec<Regex>>>()
        };
        Ok(Self {
            exec: Action::parse(&config.approval_exec)?,
            write: Action::parse(&config.approval_write)?,
            read: Action::parse(&config.approval_read)?,
            network: Action::parse(&config.approval_network)?,
            mcp: Action::parse(&config.approval_mcp)?,
            plugin: Action::parse(&config.approval_plugin)?,
            allowlist: compile("command_allowlist", &config.command_allowlist)?,
            denylist: compile("command_denylist", &config.command_denylist)?,
            session_allow: Vec::new(),
        })
    }

    /// Locked down: every categorized call denied (pure builtins and
    /// the todo list stay free). Used when the config policy is invalid.
    pub fn locked_down() -> Self {
        Self {
            exec: Action::Deny,
            write: Action::Deny,
            read: Action::Deny,
            network: Action::Deny,
            mcp: Action::Deny,
            plugin: Action::Deny,
            ..Self::default()
        }
    }

    fn level(&self, category: Category) -> Action {
        match category {
            Category::Exec => self.exec,
            Category::Write => self.write,
            Category::Read => self.read,
            Category::Network => self.network,
            Category::Mcp => self.mcp,
            Category::Plugin => self.plugin,
            Category::Free => Action::Allow,
        }
    }

    /// Remember a card-approved (`a`) call for the rest of this run.
    /// The pattern matches this exact call text only.
    pub fn allow_session(&mut self, text: &str) {
        if let Ok(pattern) = Regex::new(&regex::escape(text)) {
            self.session_allow.push(pattern);
        }
    }
}

/// One-line human summary of a call, used by cards and list matching.
/// Values truncate so a pasted file never floods the approval card.
pub fn summarize(name: &str, args: &Value) -> String {
    fn scalar(args: &Value, key: &str) -> Option<String> {
        args.get(key).and_then(|value| match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
            _ => None,
        })
    }
    let detail = match name {
        "write_file" | "read_file" | "list_files" => scalar(args, "path").unwrap_or_default(),
        "web_search" => scalar(args, "query").unwrap_or_default(),
        "web_fetch" => scalar(args, "url").unwrap_or_default(),
        "execute_rust" => scalar(args, "code")
            .map(|code| code.lines().next().unwrap_or("").to_string())
            .unwrap_or_default(),
        "calculate" => scalar(args, "expression").unwrap_or_default(),
        _ => {
            let flat = args.to_string();
            flat.chars().take(80).collect()
        }
    };
    let text = if detail.is_empty() {
        name.to_string()
    } else {
        format!("{name} {detail}")
    };
    text.chars().take(160).collect()
}

/// Pure policy resolution. Order: mode gate → posture → denylist →
/// session/allowlist → per-category level.
pub fn resolve(
    name: &str,
    args: &Value,
    mode: &str,
    allow_code: bool,
    allow_network: bool,
    policy: &Policy,
) -> Decision {
    if !crate::tool::mode_allows(mode, name) {
        return Decision::Deny(format!("not available in {mode} mode"));
    }
    match category(name) {
        Category::Exec | Category::Plugin if !allow_code => {
            return Decision::Deny("code execution is disabled by the permission posture".into());
        }
        Category::Network if !allow_network => {
            return Decision::Deny("network is disabled by the permission posture".into());
        }
        _ => {}
    }
    let text = summarize(name, args);
    if let Some(pattern) = policy.denylist.iter().find(|regex| regex.is_match(&text)) {
        return Decision::Deny(format!("matched denylist '{pattern}'"));
    }
    if policy
        .session_allow
        .iter()
        .chain(policy.allowlist.iter())
        .any(|regex| regex.is_match(&text))
    {
        return Decision::Allow;
    }
    match policy.level(category(name)) {
        Action::Allow => Decision::Allow,
        Action::Deny => Decision::Deny(format!(
            "{} calls are denied by configuration",
            category_name(category(name))
        )),
        Action::Ask => Decision::Ask(text),
    }
}

fn category_name(category: Category) -> &'static str {
    match category {
        Category::Exec => "code-execution",
        Category::Write => "file-write",
        Category::Read => "file-read",
        Category::Network => "network",
        Category::Mcp => "MCP",
        Category::Plugin => "plugin",
        Category::Free => "builtin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> Policy {
        Policy {
            exec: Action::Ask,
            write: Action::Ask,
            read: Action::Allow,
            network: Action::Allow,
            mcp: Action::Deny,
            plugin: Action::Ask,
            ..Policy::default()
        }
    }

    /// Deny beats everything, including the allowlist.
    #[test]
    fn approval_deny_beats_allowlist() {
        let mut policy = policy();
        policy.allowlist.push(Regex::new("write_file").unwrap());
        policy.denylist.push(Regex::new("secret").unwrap());
        let args = json!({"path": "secret.txt", "content": "x"});
        assert!(matches!(
            resolve("write_file", &args, "build", true, true, &policy),
            Decision::Deny(_)
        ));
    }

    /// Ask without a session allow resolves to Ask (the TUI cards it;
    /// headless `execute` turns it into a denial).
    #[test]
    fn approval_ask_without_session_denies() {
        let policy = policy();
        let decision = resolve(
            "execute_rust",
            &json!({"code": "fn main() {}"}),
            "build",
            true,
            true,
            &policy,
        );
        assert!(matches!(decision, Decision::Ask(_)));
        let mut allowed = policy;
        allowed.allow_session("execute_rust fn main() {}");
        assert_eq!(
            resolve(
                "execute_rust",
                &json!({"code": "fn main() {}"}),
                "build",
                true,
                true,
                &allowed
            ),
            Decision::Allow
        );
    }

    /// Session-approved calls pass; mode and posture still gate first.
    #[test]
    fn approval_session_allow_permits() {
        let mut policy = policy();
        policy.allow_session("write_file notes.txt");
        assert_eq!(
            resolve(
                "write_file",
                &json!({"path": "notes.txt", "content": "hi"}),
                "build",
                true,
                true,
                &policy
            ),
            Decision::Allow
        );
        assert!(matches!(
            resolve(
                "write_file",
                &json!({"path": "notes.txt", "content": "hi"}),
                "plan",
                true,
                true,
                &policy
            ),
            Decision::Deny(_)
        ));
    }

    /// Bad config surfaces as an error, never as a silent open gate.
    #[test]
    fn approval_invalid_regex_rejected() {
        let config = Config {
            command_denylist: vec!["([a-z".to_string()],
            ..Config::default()
        };
        assert!(Policy::from_config(&config).is_err());
        let config = Config {
            approval_exec: "sometimes".to_string(),
            ..Config::default()
        };
        assert!(Policy::from_config(&config).is_err());
    }

    #[test]
    fn approval_posture_and_unknown_tools() {
        let policy = Policy::default();
        assert!(matches!(
            resolve(
                "web_fetch",
                &json!({"url": "https://x"}),
                "build",
                true,
                false,
                &policy
            ),
            Decision::Deny(_)
        ));
        assert!(matches!(
            resolve("mcp_ping", &json!({}), "build", true, true, &policy),
            Decision::Allow
        ));
    }
}
