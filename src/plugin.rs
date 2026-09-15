//! Native executable plugin protocol.
//!
//! A plugin is a JSON manifest in the configured plugins directory. The
//! executable receives one JSON request on stdin and returns one JSON object
//! on stdout. Python source files are not loaded as plugins.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{Duration, timeout},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PluginManifest {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
    /// Hook names this plugin wants (`before_tool`, `after_tool`).
    /// Only declaring plugins are ever spawned for hook events.
    #[serde(default)]
    pub hooks: Vec<String>,
}

/// Outcome of one `before_tool` hook response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeforeDecision {
    Proceed,
    Rewrite(Value),
    Deny(String),
}

/// Interpret a `before_tool` hook response: `{"deny": reason}` blocks,
/// `{"arguments": {...}}` replaces the arguments, anything else proceeds.
pub fn apply_before_response(response: &Value) -> BeforeDecision {
    if let Some(reason) = response
        .get("deny")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        return BeforeDecision::Deny(reason.to_string());
    }
    if let Some(replacement) = response.get("arguments")
        && replacement.is_object()
    {
        return BeforeDecision::Rewrite(replacement.clone());
    }
    BeforeDecision::Proceed
}

/// Interpret an `after_tool` hook response: `{"result": …}` replaces the
/// result text (stringified when it is not a string), anything else
/// keeps the current result.
pub fn apply_after_response(response: &Value, current: &str) -> String {
    match response.get("result") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => current.to_string(),
    }
}

/// Manifests declaring a hook, sorted by name for deterministic order.
pub fn hooks_for(directory: &Path, hook: &str) -> Vec<PluginManifest> {
    load_manifests(directory)
        .into_iter()
        .filter(|manifest| manifest.hooks.iter().any(|name| name == hook))
        .collect()
}

fn load_manifests(directory: &Path) -> Vec<PluginManifest> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut manifests = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(manifest) = serde_json::from_str::<PluginManifest>(&text)
            && !manifest.name.trim().is_empty()
            && !manifest.command.trim().is_empty()
        {
            manifests.push(manifest);
        }
    }
    manifests.sort_by(|left, right| left.name.cmp(&right.name));
    manifests
}

pub fn definitions_from(directory: &Path) -> Vec<Value> {
    let mut result = Vec::new();
    for manifest in load_manifests(directory) {
        for tool in manifest.tools {
            let name = format!("plugin_{}_{}", manifest.name, tool.name);
            let properties = if tool.parameters.is_object() {
                tool.parameters
            } else {
                json!({})
            };
            let mut schema = json!({"type": "object", "properties": properties});
            if !tool.required.is_empty() {
                schema["required"] = json!(tool.required);
            }
            result.push(json!({
                "type": "function",
                "function": {
                    "name": name,
                    "description": format!("[plugin:{}] {}", manifest.name, tool.description),
                    "parameters": schema
                }
            }));
        }
    }
    result
}

pub async fn call_from(
    workspace: &Path,
    plugins_dir: &Path,
    name: &str,
    arguments: &Value,
) -> Result<String> {
    let directory = plugins_dir;
    let Some((manifest, tool_name)) = load_manifests(directory).into_iter().find_map(|manifest| {
        let prefix = format!("plugin_{}_", manifest.name);
        name.strip_prefix(&prefix)
            .filter(|tool| !tool.is_empty())
            .map(|tool| (manifest, tool.to_string()))
    }) else {
        bail!("plugin '{name}' is not installed");
    };
    let plugin_name = manifest.name.clone();
    if !manifest.tools.iter().any(|tool| tool.name == tool_name) {
        bail!("plugin '{plugin_name}' does not expose tool '{tool_name}'");
    }
    let request = json!({
        "method": "call",
        "tool": tool_name,
        "arguments": arguments,
    });
    let value = invoke_plugin(&manifest, workspace, request, 30).await?;
    Ok(value
        .get("content")
        .or_else(|| value.get("result"))
        .map(|value| value.to_string())
        .unwrap_or_else(|| value.to_string()))
}

/// Run every `before_tool` hook in manifest order, chaining argument
/// rewrites. A deny aborts with an error naming the plugin; transport
/// failures also error (visibly) rather than silently skipping policy.
pub async fn run_before_hooks(
    workspace: &Path,
    plugins_dir: &Path,
    tool: &str,
    arguments: &Value,
) -> Result<Value> {
    let mut current = arguments.clone();
    for manifest in hooks_for(plugins_dir, "before_tool") {
        let request = json!({
            "method": "before_tool",
            "tool": tool,
            "arguments": current,
        });
        let response = invoke_plugin(&manifest, workspace, request, 5)
            .await
            .with_context(|| format!("before_tool hook '{}' failed", manifest.name))?;
        match apply_before_response(&response) {
            BeforeDecision::Proceed => {}
            BeforeDecision::Rewrite(next) => current = next,
            BeforeDecision::Deny(reason) => {
                bail!(
                    "tool '{tool}' denied by plugin '{}': {reason}",
                    manifest.name
                )
            }
        }
    }
    Ok(current)
}

/// Run every `after_tool` hook in manifest order, chaining result
/// rewrites. Like `before_tool`, transport failures are loud errors.
pub async fn run_after_hooks(
    workspace: &Path,
    plugins_dir: &Path,
    tool: &str,
    arguments: &Value,
    result: &str,
) -> Result<String> {
    let mut current = result.to_string();
    for manifest in hooks_for(plugins_dir, "after_tool") {
        let request = json!({
            "method": "after_tool",
            "tool": tool,
            "arguments": arguments,
            "result": current,
        });
        let response = invoke_plugin(&manifest, workspace, request, 5)
            .await
            .with_context(|| format!("after_tool hook '{}' failed", manifest.name))?;
        current = apply_after_response(&response, &current);
    }
    Ok(current)
}

/// Spawn one plugin executable, send one JSON request on stdin, read one
/// JSON object from stdout. Shared by tool calls and hooks; only the
/// timeout differs (tool calls may compute, hooks must be fast).
async fn invoke_plugin(
    manifest: &PluginManifest,
    workspace: &Path,
    request: Value,
    timeout_secs: u64,
) -> Result<Value> {
    let mut command = Command::new(&manifest.command);
    command.args(&manifest.args);
    // Confine plugin execution to the workspace directory with a sanitized
    // environment. Plugins still run as the local user (no namespace isolation),
    // so network/code posture is enforced by the caller in tool::execute.
    command.current_dir(workspace);
    command.kill_on_drop(true);
    command.env_clear();
    command.env("PATH", std::env::var("PATH").unwrap_or_default());
    command.env("HOME", workspace);
    command.env("R105_PLUGIN", &manifest.name);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("starting plugin {}", manifest.name))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(serde_json::to_string(&request)?.as_bytes())
            .await?;
        stdin.write_all(b"\n").await?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("plugin stdout unavailable"))?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    timeout(
        Duration::from_secs(timeout_secs),
        reader.read_line(&mut line),
    )
    .await
    .context("plugin timed out")??;
    let _ = child.kill().await;
    serde_json::from_str(line.trim()).context("plugin returned invalid JSON")
}

pub fn status_from(directory: &Path) -> Vec<Value> {
    let manifests = load_manifests(directory);
    manifests
        .into_iter()
        .map(|manifest| {
            json!({
                "name": manifest.name,
                "version": manifest.version,
                "command": manifest.command,
                "tools": manifest.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>(),
                "hooks": manifest.hooks,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_manifest(directory: &Path, file: &str, body: &str) {
        std::fs::write(directory.join(file), body).unwrap();
    }

    #[test]
    fn before_hook_deny_blocks_tool() {
        let decision = apply_before_response(&json!({"deny": "policy says no"}));
        assert_eq!(decision, BeforeDecision::Deny("policy says no".to_string()));
        // A blank deny is not a deny: the call proceeds.
        assert_eq!(
            apply_before_response(&json!({"deny": "   "})),
            BeforeDecision::Proceed
        );
    }

    #[test]
    fn before_hook_rewrites_arguments() {
        let replacement = json!({"path": "safe.txt"});
        let decision = apply_before_response(&json!({"arguments": replacement}));
        assert_eq!(
            decision,
            BeforeDecision::Rewrite(json!({"path": "safe.txt"}))
        );
        // Non-object arguments are ignored, not fatal.
        assert_eq!(
            apply_before_response(&json!({"arguments": "oops"})),
            BeforeDecision::Proceed
        );
        assert_eq!(apply_before_response(&json!({})), BeforeDecision::Proceed);
    }

    #[test]
    fn after_hook_rewrites_result() {
        assert_eq!(
            apply_after_response(&json!({"result": "scrubbed"}), "secret"),
            "scrubbed"
        );
        // Non-string results stringify instead of failing the tool.
        assert_eq!(
            apply_after_response(&json!({"result": {"redacted": true}}), "secret"),
            "{\"redacted\":true}"
        );
        assert_eq!(apply_after_response(&json!({}), "kept"), "kept");
    }

    #[test]
    fn hooks_only_fire_for_declaring_plugins() {
        let directory = tempdir().unwrap();
        write_manifest(
            directory.path(),
            "gate.json",
            r#"{"name":"gate","command":"sh","tools":[],"hooks":["before_tool"]}"#,
        );
        write_manifest(
            directory.path(),
            "plain.json",
            r#"{"name":"plain","command":"sh","tools":[]}"#,
        );
        let names: Vec<String> = hooks_for(directory.path(), "before_tool")
            .into_iter()
            .map(|manifest| manifest.name)
            .collect();
        assert_eq!(names, vec!["gate".to_string()]);
        assert!(hooks_for(directory.path(), "after_tool").is_empty());
    }

    /// End-to-end through the real spawn path: a shell plugin that denies.
    #[tokio::test]
    #[cfg(unix)]
    async fn deny_hook_blocks_before_hooks() {
        let workspace = tempdir().unwrap();
        let plugins = tempdir().unwrap();
        write_manifest(
            plugins.path(),
            "gate.json",
            r#"{"name":"gate","command":"sh","args":["-c","cat >/dev/null; echo '{\"deny\": \"nope\"}'"],"tools":[],"hooks":["before_tool"]}"#,
        );
        let error = run_before_hooks(
            workspace.path(),
            plugins.path(),
            "write_file",
            &json!({"path": "x"}),
        )
        .await
        .unwrap_err();
        assert!(
            error.to_string().contains("denied by plugin 'gate'"),
            "{error:#}"
        );
    }
}
