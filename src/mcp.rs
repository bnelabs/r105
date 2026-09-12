//! Lightweight native MCP client.
//!
//! Stdio JSON-RPC and request/response HTTP transports are supported without
//! a Python dependency. Declared tool schemas are available immediately; the
//! UI can reconnect a server to refresh its live tool list.

use std::{
    collections::HashSet,
    fs,
    process::Stdio,
    sync::{OnceLock, RwLock},
};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    time::{Duration, timeout},
};

#[derive(Debug, Clone)]
struct Server {
    name: String,
    transport: String,
    command: Option<String>,
    args: Vec<String>,
    env: serde_json::Map<String, Value>,
    url: Option<String>,
    tools: Vec<Value>,
}

fn servers() -> Vec<Server> {
    let paths = crate::config::ConfigPaths::discover();
    let Ok(text) = fs::read_to_string(paths.config_file) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    value
        .get("mcp_servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            let name = object.get("name")?.as_str()?.to_string();
            let tools = object
                .get("tools")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Some(Server {
                name,
                transport: object
                    .get("transport")
                    .and_then(Value::as_str)
                    .unwrap_or("stdio")
                    .to_string(),
                command: object
                    .get("command")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                args: object
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                env: object
                    .get("env")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default(),
                url: object
                    .get("url")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                tools,
            })
        })
        .collect()
}

pub fn definitions() -> Vec<Value> {
    let mut result = Vec::new();
    for server in servers() {
        for tool in server.tools {
            let Some(object) = tool.as_object() else {
                continue;
            };
            append_unique(&mut result, tool_definition(&server.name, object));
        }
    }
    if let Ok(cache) = discovered_cache().read() {
        for tool in cache.iter().cloned() {
            append_unique(&mut result, tool);
        }
    }
    result
}

pub fn status() -> Vec<Value> {
    servers()
        .into_iter()
        .map(|server| {
            let discovered_tools = discovered_cache()
                .read()
                .map(|cache| {
                    let prefix = format!("mcp_{}_", server.name);
                    cache
                        .iter()
                        .filter(|tool| {
                            tool.pointer("/function/name")
                                .and_then(Value::as_str)
                                .is_some_and(|name| name.starts_with(&prefix))
                        })
                        .count()
                })
                .unwrap_or_default();
            json!({
                "name": server.name,
                "transport": server.transport,
                "command": server.command,
                "url": server.url,
                "declared_tools": server.tools.len()
                ,"discovered_tools": discovered_tools
            })
        })
        .collect()
}

pub async fn call(name: &str, arguments: &Value) -> Result<String> {
    let Some((server, tool_name)) = servers().into_iter().find_map(|server| {
        let prefix = format!("mcp_{}_", server.name);
        name.strip_prefix(&prefix)
            .filter(|tool| !tool.is_empty())
            .map(|tool| (server, tool.to_string()))
    }) else {
        bail!("MCP tool '{name}' is not configured");
    };
    let server_name = server.name.clone();
    if !server.tools.is_empty()
        && !server
            .tools
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(tool_name.as_str()))
    {
        bail!("MCP server '{server_name}' does not expose '{tool_name}'");
    }
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": tool_name, "arguments": arguments}
    });
    let response = if server.transport.eq_ignore_ascii_case("stdio") {
        call_stdio(&server, request).await?
    } else {
        call_http(&server, request).await?
    };
    Ok(format_result(&response))
}

pub async fn reconnect(server_name: Option<&str>) -> Result<String> {
    let configured = servers();
    let selected = configured
        .into_iter()
        .filter(|server| server_name.is_none_or(|name| name == server.name))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(match server_name {
            Some(name) => anyhow::anyhow!("MCP server '{name}' is not configured"),
            None => anyhow::anyhow!("no MCP servers are configured"),
        });
    }

    let names = selected
        .iter()
        .map(|server| server.name.clone())
        .collect::<HashSet<_>>();
    let mut discovered = Vec::new();
    let mut failures = Vec::new();
    for server in &selected {
        match discover_server(server).await {
            Ok(tools) => discovered.extend(tools),
            Err(error) => failures.push(format!("{}: {error:#}", server.name)),
        }
    }

    if let Ok(mut cache) = discovered_cache().write() {
        cache.retain(|tool| {
            let Some(name) = tool.pointer("/function/name").and_then(Value::as_str) else {
                return true;
            };
            !names
                .iter()
                .any(|server| name.starts_with(&format!("mcp_{server}_")))
        });
        cache.extend(discovered.iter().cloned());
    }

    let selected_count = selected.len();
    let discovered_count = discovered.len();
    if failures.is_empty() {
        Ok(format!(
            "MCP reconnect complete: {selected_count} server(s), {discovered_count} tool(s) discovered"
        ))
    } else {
        Ok(format!(
            "MCP reconnect partial: {discovered_count} tool(s) discovered; {}",
            failures.join("; ")
        ))
    }
}

fn discovered_cache() -> &'static RwLock<Vec<Value>> {
    static CACHE: OnceLock<RwLock<Vec<Value>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Vec::new()))
}

async fn discover_server(server: &Server) -> Result<Vec<Value>> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let response = if server.transport.eq_ignore_ascii_case("stdio") {
        call_stdio(server, request).await?
    } else {
        call_http(server, request).await?
    };
    if let Some(error) = response.get("error") {
        bail!("tools/list failed: {error}");
    }
    let result = response.get("result").unwrap_or(&response);
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(tools
        .iter()
        .filter_map(|tool| {
            tool.as_object()
                .map(|object| tool_definition(&server.name, object))
        })
        .collect())
}

fn append_unique(result: &mut Vec<Value>, tool: Value) {
    let Some(name) = tool.pointer("/function/name").and_then(Value::as_str) else {
        return;
    };
    if !result
        .iter()
        .any(|item| item.pointer("/function/name").and_then(Value::as_str) == Some(name))
    {
        result.push(tool);
    }
}

fn tool_definition(server_name: &str, object: &serde_json::Map<String, Value>) -> Value {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let schema = object
        .get("inputSchema")
        .or_else(|| object.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    json!({
        "type": "function",
        "function": {
            "name": format!("mcp_{}_{}", server_name, name),
            "description": format!("[MCP:{server_name}] {}", object.get("description").and_then(Value::as_str).unwrap_or("")),
            "parameters": schema
        }
    })
}

async fn call_stdio(server: &Server, request: Value) -> Result<Value> {
    let command = server
        .command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("MCP stdio server has no command"))?;
    let mut child = Command::new(command);
    child.args(&server.args);
    child
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    for (key, value) in &server.env {
        if let Some(value) = value.as_str() {
            child.env(key, value);
        }
    }
    child
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = child
        .spawn()
        .with_context(|| format!("starting MCP server {}", server.name))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP stdin unavailable"))?;
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "r105", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    for message in [
        serde_json::to_string(&initialize)?,
        serde_json::to_string(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        )?,
        serde_json::to_string(&request)?,
    ] {
        stdin.write_all(message.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    drop(stdin);
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("MCP stdout unavailable"))?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let response = timeout(Duration::from_secs(30), async {
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                bail!("MCP server closed stdout before replying");
            }
            let value: Value = match serde_json::from_str(line.trim()) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value.get("id").and_then(Value::as_u64) == Some(2) {
                return Ok::<Value, anyhow::Error>(value);
            }
        }
    })
    .await
    .context("MCP server timed out")??;
    let _ = child.kill().await;
    Ok(response)
}

async fn call_http(server: &Server, request: Value) -> Result<Value> {
    let url = server
        .url
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("MCP HTTP server has no URL"))?;
    let response = reqwest::Client::new()
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&request)
        .send()
        .await
        .context("calling MCP HTTP server")?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        bail!(
            "MCP HTTP server returned {status}: {}",
            text.chars().take(400).collect::<String>()
        );
    }
    if let Ok(value) = serde_json::from_str(&text) {
        return Ok(value);
    }
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:")
            && let Ok(value) = serde_json::from_str(data.trim())
        {
            return Ok(value);
        }
    }
    bail!("MCP server returned neither JSON nor an SSE data frame")
}

fn format_result(value: &Value) -> String {
    if let Some(error) = value.get("error") {
        return format!("MCP error: {error}");
    }
    let result = value.get("result").unwrap_or(value);
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let text: Vec<&str> = content
            .iter()
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .collect();
        if !text.is_empty() {
            return text.join("\n");
        }
    }
    result.to_string()
}
