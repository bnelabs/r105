//! Versioned, atomic session persistence compatible with r105 0.8.x files.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::{ConfigPaths, atomic_write_json},
    model::{ChatState, FunctionCall, Message, ToolCall},
};

pub const SESSION_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    history: Vec<Message>,
    state: SavedState,
    message_count: usize,
    saved_at: String,
    /// Source session for forks and checkpoints; absent in older files.
    #[serde(default)]
    parent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedState {
    profile: Option<String>,
    quality: Option<String>,
    max_tokens: Option<u32>,
    json_mode: bool,
    cache_prompt: bool,
    model: String,
    context_tokens: u64,
    trace_id: String,
    active_skills: Vec<String>,
    #[serde(default)]
    skill_params: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    #[serde(default = "default_saved_mode")]
    mode: String,
}

fn default_saved_mode() -> String {
    "build".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub name: String,
    pub saved_at: String,
    pub message_count: usize,
    pub preview: String,
    /// Source session when this file is a fork or checkpoint.
    pub parent: Option<String>,
}

pub fn save(paths: &ConfigPaths, name: &str, state: &ChatState) -> Result<PathBuf> {
    save_with_parent(paths, name, state, None)
}

/// Save with an explicit parent link (forks, checkpoints). The manual
/// loader ignores the key, so files with parents still load anywhere.
pub fn save_with_parent(
    paths: &ConfigPaths,
    name: &str,
    state: &ChatState,
    parent: Option<&str>,
) -> Result<PathBuf> {
    let path = session_path(paths, name)?;
    let file = SessionFile {
        version: SESSION_FORMAT_VERSION,
        history: state.history.clone(),
        state: SavedState {
            profile: state.profile.clone(),
            quality: state.quality.clone(),
            max_tokens: state.max_tokens,
            json_mode: state.json_mode,
            cache_prompt: state.cache_prompt,
            model: state.model.clone(),
            context_tokens: state.context_tokens,
            trace_id: state.trace_id.clone(),
            active_skills: state.active_skills.clone(),
            skill_params: state.skill_params.clone(),
            mode: state.mode.clone(),
        },
        message_count: state.history.len(),
        saved_at: now_string(),
        parent: parent
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string),
    };
    atomic_write_json(&path, &file)?;
    Ok(path)
}

pub fn load(paths: &ConfigPaths, name: &str, state: &mut ChatState) -> Result<usize> {
    let path = session_path(paths, name)?;
    let raw = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw).context("parsing session JSON")?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > SESSION_FORMAT_VERSION {
        bail!(
            "session uses newer format v{version}; this build reads up to v{SESSION_FORMAT_VERSION}"
        );
    }
    let history = value
        .get("history")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(parse_message)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    state.history = history;
    if let Some(saved) = value.get("state").and_then(Value::as_object) {
        state.profile = optional_string(saved.get("profile"));
        state.quality = optional_string(saved.get("quality"));
        state.max_tokens = saved
            .get("max_tokens")
            .and_then(Value::as_u64)
            .map(|v| v as u32);
        state.json_mode = saved
            .get("json_mode")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        state.cache_prompt = saved
            .get("cache_prompt")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if let Some(model) = optional_string(saved.get("model")) {
            state.model = model;
        }
        if let Some(mode) = optional_string(saved.get("mode")) {
            state.mode = mode;
        }
        if let Some(context) = saved
            .get("context_tokens")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)
        {
            state.context_tokens = context;
        }
        if let Some(trace) = optional_string(saved.get("trace_id")) {
            state.trace_id = trace;
        }
        state.active_skills = saved
            .get("active_skills")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        state.skill_params = saved
            .get("skill_params")
            .and_then(Value::as_object)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|(skill, params)| {
                        let params = params.as_object()?;
                        Some((
                            skill.clone(),
                            params
                                .iter()
                                .filter_map(|(key, value)| {
                                    Some((key.clone(), value.as_str()?.to_string()))
                                })
                                .collect(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
    }
    state.last_usage = crate::model::Usage::default();
    Ok(state.history.len())
}

pub fn list(paths: &ConfigPaths) -> Vec<SessionInfo> {
    let Ok(entries) = fs::read_dir(&paths.sessions_dir) else {
        return Vec::new();
    };
    let mut result = entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|v| v.to_str()) == Some("json"))
        .filter_map(|entry| {
            let path = entry.path();
            let raw = fs::read_to_string(&path).ok()?;
            let value = serde_json::from_str::<Value>(&raw).ok()?;
            let history = value.get("history").and_then(Value::as_array)?;
            let preview = history
                .iter()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
                .and_then(|message| message.get("content"))
                .map(value_text)
                .unwrap_or_default();
            Some(SessionInfo {
                name: path.file_stem()?.to_string_lossy().to_string(),
                saved_at: value
                    .get("saved_at")
                    .map(value_text)
                    .unwrap_or_else(|| "unknown".into()),
                message_count: history.len(),
                preview: preview.chars().take(80).collect(),
                parent: optional_string(value.get("parent")),
            })
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| right.saved_at.cmp(&left.saved_at));
    result
}

pub fn search(paths: &ConfigPaths, query: &str) -> Vec<Value> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    list(paths)
        .into_iter()
        .filter_map(|item| {
            let path = session_path(paths, &item.name).ok()?;
            let value = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
            let matches = value
                .get("history")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(|message| {
                    let content = value_text(message.get("content").unwrap_or(&Value::Null));
                    let snippet = case_insensitive_snippet(&content, &needle)?;
                    Some(serde_json::json!({
                        "role": message.get("role").map(value_text).unwrap_or_default(),
                        "snippet": snippet
                    }))
                })
                .take(3)
                .collect::<Vec<_>>();
            (!matches.is_empty()).then(|| {
                serde_json::json!({
                    "name": item.name,
                    "saved_at": item.saved_at,
                    "message_count": item.message_count,
                    "matches": matches
                })
            })
        })
        .take(20)
        .collect()
}

pub fn delete(paths: &ConfigPaths, name: &str) -> Result<bool> {
    let path = session_path(paths, name)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

/// Timestamped backup before history-destroying commands (rewind,
/// compact, clear). Checkpoints live in the sessions dir so
/// `/session load <name>` restores them with the existing code path;
/// only the newest 10 survive so automatic backups never fill the disk.
pub fn save_checkpoint(
    paths: &ConfigPaths,
    state: &ChatState,
    reason: &str,
    parent: Option<&str>,
) -> Result<String> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let clean: String = reason
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect();
    let clean = if clean.is_empty() {
        "manual".to_string()
    } else {
        clean
    };
    let mut name = format!("checkpoint-{clean}-{epoch}");
    let mut suffix = 1;
    while session_path(paths, &name)?.exists() {
        suffix += 1;
        // Zero-padded so lexicographic order stays chronological when
        // several checkpoints share one epoch second.
        name = format!("checkpoint-{clean}-{epoch}-{suffix:03}");
    }
    save_with_parent(paths, &name, state, parent)?;
    prune_checkpoints(paths, 10);
    Ok(name)
}

fn prune_checkpoints(paths: &ConfigPaths, keep: usize) {
    let Ok(entries) = fs::read_dir(&paths.sessions_dir) else {
        return;
    };
    let mut checkpoints: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let stem = entry.path().file_stem()?.to_string_lossy().to_string();
            stem.starts_with("checkpoint-").then_some(stem)
        })
        .collect();
    checkpoints.sort();
    if checkpoints.len() > keep {
        for stale in checkpoints.drain(..checkpoints.len() - keep) {
            let _ = delete(paths, &stale);
        }
    }
}

pub fn diff(paths: &ConfigPaths, name: &str, state: &ChatState) -> Result<String> {
    let path = session_path(paths, name)?;
    let value: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let saved = value
        .get("history")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let current = state.history.len();
    Ok(format!(
        "diff vs saved session '{name}':\n  messages: {:+} ({saved} saved -> {current} current)",
        current as i64 - saved as i64
    ))
}

/// Indented parent-chain view of saved sessions with the 80-char preview
/// per node. Roots (no parent, or a parent that no longer exists) come
/// first; anything unreachable (cycles, however constructed) renders as
/// its own root so the command always terminates with full coverage.
pub fn tree(paths: &ConfigPaths) -> String {
    let items = list(paths);
    if items.is_empty() {
        return "No saved sessions".to_string();
    }
    let known: HashSet<&str> = items.iter().map(|item| item.name.as_str()).collect();
    let mut children: BTreeMap<Option<&str>, Vec<&SessionInfo>> = BTreeMap::new();
    for item in &items {
        let key = item
            .parent
            .as_deref()
            .filter(|parent| known.contains(parent));
        children.entry(key).or_default().push(item);
    }
    for group in children.values_mut() {
        group.sort_by(|left, right| {
            left.saved_at
                .cmp(&right.saved_at)
                .then_with(|| left.name.cmp(&right.name))
        });
    }
    let mut lines = vec!["Sessions".to_string()];
    let mut visited: HashSet<&str> = HashSet::new();
    if let Some(roots) = children.remove(&None) {
        for root in roots {
            render_tree_node(root, &children, &mut visited, 0, &mut lines);
        }
    }
    let mut rest: Vec<&SessionInfo> = items
        .iter()
        .filter(|item| !visited.contains(item.name.as_str()))
        .collect();
    rest.sort_by(|left, right| left.name.cmp(&right.name));
    for item in rest {
        render_tree_node(item, &children, &mut visited, 0, &mut lines);
    }
    lines.join("\n")
}

fn render_tree_node<'a>(
    node: &'a SessionInfo,
    children: &BTreeMap<Option<&'a str>, Vec<&'a SessionInfo>>,
    visited: &mut HashSet<&'a str>,
    depth: usize,
    lines: &mut Vec<String>,
) {
    // A repeated name means a parent cycle; the first occurrence already
    // shows the subtree, so the repeat is dropped silently.
    if !visited.insert(node.name.as_str()) {
        return;
    }
    lines.push(format!(
        "{}{}  {}",
        "  ".repeat(depth),
        node.name,
        node.preview
    ));
    if let Some(kids) = children.get(&Some(node.name.as_str())) {
        for kid in kids {
            render_tree_node(kid, children, visited, depth + 1, lines);
        }
    }
}

/// Drop trailing tool messages whose call has no matching assistant
/// tool-call in the kept prefix. Truncating at a turn boundary can
/// strand results from an interrupted tool round; only provably
/// stranded results are removed, everything else is kept verbatim.
pub fn repair_prefix(mut messages: Vec<Message>) -> Vec<Message> {
    loop {
        let stranded = match messages.last() {
            Some(last) if last.role == "tool" => match &last.tool_call_id {
                Some(call_id) if !call_id.is_empty() => {
                    !messages[..messages.len() - 1].iter().any(|message| {
                        message.role == "assistant"
                            && message.tool_calls.iter().any(|call| call.id == *call_id)
                    })
                }
                _ => false,
            },
            _ => false,
        };
        if !stranded {
            return messages;
        }
        messages.pop();
    }
}

fn session_path(paths: &ConfigPaths, name: &str) -> Result<PathBuf> {
    let safe = name.trim().replace(['/', '\\'], "_").replace("..", "_");
    if safe.is_empty() {
        bail!("session name is required");
    }
    fs::create_dir_all(&paths.sessions_dir)?;
    Ok(paths.sessions_dir.join(format!("{safe}.json")))
}

fn parse_message(value: &Value) -> Option<Message> {
    let object = value.as_object()?;
    let role = object.get("role")?.as_str()?;
    // Reject unknown roles so a hand-edited or foreign session file cannot
    // inject payloads the backend would refuse (or misattribute).
    if !matches!(role, "user" | "assistant" | "system" | "tool") {
        return None;
    }
    let role = role.to_string();
    let content = value_text(object.get("content").unwrap_or(&Value::Null));
    let tool_calls = object
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let object = item.as_object()?;
                    let function = object.get("function").and_then(Value::as_object)?;
                    Some(ToolCall {
                        id: value_text(object.get("id").unwrap_or(&Value::Null)),
                        type_: object
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("function")
                            .into(),
                        function: FunctionCall {
                            name: function
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            arguments: value_text(
                                function.get("arguments").unwrap_or(&Value::Null),
                            ),
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Message {
        role,
        content,
        id: object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        tool_calls,
        tool_call_id: optional_string(object.get("tool_call_id")),
        name: optional_string(object.get("name")),
    })
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Case-insensitive substring match over Unicode chars with 60 chars of
/// context on each side. Char-based (not byte-based) so lowercasing, which
/// can change byte length, can never produce an invalid slice.
fn case_insensitive_snippet(content: &str, needle: &str) -> Option<String> {
    if needle.is_empty() {
        return None;
    }
    let content_chars: Vec<char> = content.chars().collect();
    let lower_content: Vec<char> = content.to_lowercase().chars().collect();
    let lower_needle: Vec<char> = needle.to_lowercase().chars().collect();
    if lower_needle.is_empty() || lower_needle.len() > lower_content.len() {
        return None;
    }
    let position = lower_content
        .windows(lower_needle.len())
        .position(|window| window == lower_needle.as_slice())?;
    let start = position.saturating_sub(60);
    let end = (position + lower_needle.len() + 60).min(content_chars.len());
    Some(content_chars[start..end].iter().collect())
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn saves_and_loads_stable_shape() {
        let root = tempdir().unwrap();
        let paths = ConfigPaths {
            home: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            config_file: root.path().join("config/config.json"),
            sessions_dir: root.path().join("config/sessions"),
            plugins_dir: root.path().join("config/plugins"),
        };
        let mut state = ChatState {
            workspace: root.path().to_path_buf(),
            ..ChatState::from_config(&crate::config::Config::default(), root.path().to_path_buf())
        };
        state.history.push(Message::user("hello"));
        save(&paths, "test", &state).unwrap();
        state.history.clear();
        assert_eq!(load(&paths, "test", &mut state).unwrap(), 1);
        assert_eq!(state.history[0].content, "hello");
    }

    #[test]
    fn search_handles_unicode_without_panicking() {
        // Multibyte content must match case-insensitively and keep original case.
        let snippet = case_insensitive_snippet("HELLO WÖRLD", "wörld").unwrap();
        assert!(snippet.contains("WÖRLD"));
        assert!(case_insensitive_snippet("hello world", "missing").is_none());
        assert!(case_insensitive_snippet("abc", "").is_none());
        // Expanding case folds (İ -> i + combining dot) change char counts;
        // the search must never panic even when positions don't map 1:1.
        let _ = case_insensitive_snippet("İstanbul is beautiful", "istanbul");
    }

    #[test]
    fn unknown_roles_are_rejected() {
        let value = serde_json::json!({"role": "admin", "content": "hi"});
        assert!(parse_message(&value).is_none());
        let value = serde_json::json!({"role": "user", "content": "hi"});
        assert_eq!(parse_message(&value).unwrap().role, "user");
    }

    #[test]
    fn message_ids_survive_round_trip_and_default_empty() {
        let root = tempdir().unwrap();
        let paths = ConfigPaths {
            home: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            config_file: root.path().join("config/config.json"),
            sessions_dir: root.path().join("config/sessions"),
            plugins_dir: root.path().join("config/plugins"),
        };
        let mut state = ChatState {
            workspace: root.path().to_path_buf(),
            ..ChatState::from_config(&crate::config::Config::default(), root.path().to_path_buf())
        };
        let mut tagged = Message::user("tagged");
        tagged.id = "m7".to_string();
        state.history.push(tagged);
        save(&paths, "ids", &state).unwrap();
        state.history.clear();
        assert_eq!(load(&paths, "ids", &mut state).unwrap(), 1);
        assert_eq!(state.history[0].id, "m7");
        // Files written before IDs existed load with an empty ID.
        let legacy = serde_json::json!({"role": "tool", "content": "out"});
        assert_eq!(parse_message(&legacy).unwrap().id, "");
    }

    #[test]
    fn save_checkpoint_prunes_to_ten() {
        let root = tempdir().unwrap();
        let paths = ConfigPaths {
            home: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            config_file: root.path().join("config/config.json"),
            sessions_dir: root.path().join("config/sessions"),
            plugins_dir: root.path().join("config/plugins"),
        };
        let state = ChatState {
            workspace: root.path().to_path_buf(),
            ..ChatState::from_config(&crate::config::Config::default(), root.path().to_path_buf())
        };
        let mut first = String::new();
        for _ in 0..12 {
            let name = save_checkpoint(&paths, &state, "test", None).unwrap();
            assert!(name.starts_with("checkpoint-test-"));
            if first.is_empty() {
                first = name;
            }
        }
        let kept: Vec<String> = list(&paths)
            .into_iter()
            .map(|item| item.name)
            .filter(|name| name.starts_with("checkpoint-"))
            .collect();
        assert_eq!(kept.len(), 10);
        // The oldest backup (no numeric suffix) was pruned first.
        assert!(!kept.contains(&first));
    }

    #[test]
    fn repair_prefix_drops_stranded_tool_results() {
        use crate::model::{FunctionCall, ToolCall};

        let call = |id: &str| ToolCall {
            id: id.to_string(),
            type_: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        };
        let history = vec![
            Message::user("u1"),
            Message::assistant_with_tools("a1", vec![call("c1")]),
            Message::tool("c1", "r1"),
            // Interrupted round: the call never made it into the prefix.
            Message::tool("c2", "r2"),
        ];
        let repaired = repair_prefix(history);
        assert_eq!(repaired.len(), 3);
        assert_eq!(repaired[2].content, "r1");
        // Intact prefixes pass through untouched.
        let intact = vec![
            Message::user("u1"),
            Message::assistant_with_tools("a1", vec![call("c1")]),
            Message::tool("c1", "r1"),
        ];
        assert_eq!(repair_prefix(intact).len(), 3);
    }

    #[test]
    fn tree_renders_parent_chains() {
        let root = tempdir().unwrap();
        let paths = ConfigPaths {
            home: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            config_file: root.path().join("config/config.json"),
            sessions_dir: root.path().join("config/sessions"),
            plugins_dir: root.path().join("config/plugins"),
        };
        let mut state = ChatState {
            workspace: root.path().to_path_buf(),
            ..ChatState::from_config(&crate::config::Config::default(), root.path().to_path_buf())
        };
        state.history.push(Message::user("root work"));
        save(&paths, "root", &state).unwrap();
        save_with_parent(&paths, "child", &state, Some("root")).unwrap();
        let rendered = tree(&paths);
        let root_line = rendered.lines().find(|line| line.contains("root")).unwrap();
        let child_line = rendered
            .lines()
            .find(|line| line.contains("child"))
            .unwrap();
        assert!(
            !root_line.starts_with(' '),
            "root must not indent:\n{rendered}"
        );
        assert!(
            child_line.starts_with("  child"),
            "child must indent:\n{rendered}"
        );
        assert!(
            rendered.contains("root work"),
            "preview missing:\n{rendered}"
        );
    }

    fn test_paths() -> (tempfile::TempDir, ConfigPaths) {
        let directory = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            home: directory.path().to_path_buf(),
            config_dir: directory.path().to_path_buf(),
            config_file: directory.path().join("config.json"),
            sessions_dir: directory.path().join("sessions"),
            plugins_dir: directory.path().join("plugins"),
        };
        (directory, paths)
    }

    fn test_state(mode: &str) -> ChatState {
        let mut state: ChatState = serde_json::from_value(serde_json::json!({})).unwrap();
        state.mode = mode.to_string();
        state
    }

    /// The session file carries the mode across save/load.
    #[test]
    fn mode_persists_in_session_file() {
        let (_directory, paths) = test_paths();
        save(&paths, "planned", &test_state("plan")).unwrap();
        let mut loaded = test_state("build");
        load(&paths, "planned", &mut loaded).unwrap();
        assert_eq!(loaded.mode, "plan");
    }

    /// Files written before modes existed carry no mode key; loading
    /// them leaves the live mode alone (same as model and trace id).
    #[test]
    fn mode_absent_in_session_file_leaves_state_untouched() {
        let (_directory, paths) = test_paths();
        let mut loaded = test_state("ask");
        std::fs::create_dir_all(&paths.sessions_dir).unwrap();
        std::fs::write(
            paths.sessions_dir.join("legacy.json"),
            r#"{"version":1,"history":[],"state":{"model":"x"},"message_count":0}"#,
        )
        .unwrap();
        load(&paths, "legacy", &mut loaded).unwrap();
        assert_eq!(loaded.mode, "ask", "load must not clobber without a value");
        let mut fresh = test_state("build");
        fresh.mode = "garbage".to_string();
        load(&paths, "legacy", &mut fresh).unwrap();
        assert_eq!(fresh.mode, "garbage");
    }
}
