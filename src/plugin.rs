//! Native executable plugin protocol.
//!
//! A plugin is a JSON manifest in the configured plugins directory. The
//! executable receives one JSON request on stdin and returns one JSON object
//! on stdout. Python source files are intentionally ignored with a migration
//! hint; the Rust transition never embeds a Python interpreter.

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
}

pub fn manifests() -> Vec<PluginManifest> {
    let paths = crate::config::ConfigPaths::discover();
    let directory = crate::config::Config::load(&paths)
        .map(|config| config.plugins_dir)
        .unwrap_or(paths.plugins_dir);
    load_manifests(&directory)
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

pub async fn call_from(directory: &Path, name: &str, arguments: &Value) -> Result<String> {
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
    let mut command = Command::new(&manifest.command);
    command.args(&manifest.args);
    command.env_clear();
    command.env("PATH", std::env::var("PATH").unwrap_or_default());
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
    timeout(Duration::from_secs(30), reader.read_line(&mut line))
        .await
        .context("plugin timed out")??;
    let _ = child.kill().await;
    let value: Value = serde_json::from_str(line.trim()).context("plugin returned invalid JSON")?;
    Ok(value
        .get("content")
        .or_else(|| value.get("result"))
        .map(|value| value.to_string())
        .unwrap_or_else(|| value.to_string()))
}

pub fn status() -> Vec<Value> {
    manifests()
        .into_iter()
        .map(|manifest| {
            json!({
                "name": manifest.name,
                "version": manifest.version,
                "command": manifest.command,
                "tools": manifest.tools.iter().map(|tool| tool.name.clone()).collect::<Vec<_>>()
            })
        })
        .collect()
}
