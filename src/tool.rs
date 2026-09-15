//! Native tool protocol and built-in tool implementations.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    model::ToolCall,
    sandbox::Sandbox,
    security::{
        MAX_CODE_SIZE, MAX_FILE_CONTENT, MAX_FILE_READ, MAX_SEARCH_QUERY, resolve_public_socket,
        safe_path, truncate_output, validate_tool_text, validate_web_url,
    },
};

const MAX_WEB_BODY: usize = 5 * 1024 * 1024;

#[derive(Clone)]
pub struct ToolContext {
    pub workspace: PathBuf,
    pub plugins_dir: PathBuf,
    pub sandbox: Sandbox,
    pub cancellation: CancellationToken,
    pub allow_network: bool,
    pub allow_code: bool,
    pub mode: String,
    pub policy: crate::approve::Policy,
    /// Shared with the TUI: `todo_write` replaces the list, the UI syncs
    /// it into session state when the round's results land.
    pub todos: Arc<Mutex<Vec<crate::model::TodoItem>>>,
}

/// Tools usable in plan mode: read-only inspection, web research, and
/// the todo list (which mutates no workspace state). Everything else —
/// `execute_rust`, `write_file`, `plugin_*`, `mcp_*` — is denied.
pub const PLAN_MODE_TOOLS: &[&str] = &[
    "read_file",
    "list_files",
    "get_time",
    "calculate",
    "convert",
    "system_info",
    "web_search",
    "web_fetch",
    "todo_write",
];

/// Ask mode answers directly; only the todo list may be touched so a
/// spoken plan can still be recorded. (Added alongside 0014; until then
/// the name simply matches nothing.)
pub const ASK_MODE_TOOLS: &[&str] = &["todo_write"];

/// Whether `name` may run under `mode`. Unknown modes fail open to
/// build behavior so a corrupt session file cannot brick tool use.
pub fn mode_allows(mode: &str, name: &str) -> bool {
    match mode {
        "ask" => ASK_MODE_TOOLS.contains(&name),
        "plan" => PLAN_MODE_TOOLS.contains(&name),
        _ => true,
    }
}

/// Tool definitions offered to the model under `mode`. Ask mode hides
/// everything but the todo list so denied calls never start; plan keeps
/// the full set (denials arrive as feedback) so writes can be discussed.
pub fn definitions_for_mode(all: Vec<Value>, mode: &str) -> Vec<Value> {
    if mode != "ask" {
        return all;
    }
    all.into_iter()
        .filter(|definition| {
            definition
                .pointer("/function/name")
                .and_then(Value::as_str)
                .is_some_and(|name| ASK_MODE_TOOLS.contains(&name))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub name: String,
    pub call_id: String,
    pub content: String,
}

pub fn definitions_from(plugins_dir: &Path) -> Vec<Value> {
    let mut result = builtin_definitions();
    result.extend(crate::plugin::definitions_from(plugins_dir));
    result.extend(crate::mcp::definitions());
    result
}

fn builtin_definitions() -> Vec<Value> {
    vec![
        definition(
            "execute_rust",
            "Compile and execute Rust code in the configured sandbox.",
            json!({"code": {"type": "string", "description": "Rust source containing fn main()."}}),
            &["code"],
        ),
        definition(
            "write_file",
            "Write content to a file inside the workspace and report the change.",
            json!({
                "path": {"type": "string", "description": "Relative path inside the workspace."},
                "content": {"type": "string", "description": "UTF-8 file content."}
            }),
            &["path", "content"],
        ),
        definition(
            "read_file",
            "Read a UTF-8 file from the workspace.",
            json!({"path": {"type": "string", "description": "Relative path inside the workspace."}}),
            &["path"],
        ),
        definition(
            "list_files",
            "List entries in a workspace directory.",
            json!({"path": {"type": "string", "description": "Relative directory path; defaults to ."}}),
            &[],
        ),
        definition(
            "get_time",
            "Return the current system time.",
            json!({}),
            &[],
        ),
        definition(
            "calculate",
            "Safely evaluate arithmetic with bounded numbers and recursion.",
            json!({"expression": {"type": "string", "description": "Arithmetic expression."}}),
            &["expression"],
        ),
        definition(
            "convert",
            "Convert common length, mass, time, data, speed, volume, and temperature units.",
            json!({
                "value": {"type": "number"},
                "from_unit": {"type": "string"},
                "to_unit": {"type": "string"}
            }),
            &["value", "from_unit", "to_unit"],
        ),
        definition(
            "system_info",
            "Return basic host and process information as JSON.",
            json!({}),
            &[],
        ),
        definition(
            "web_search",
            "Search the public web and return concise result links.",
            json!({"query": {"type": "string"}}),
            &["query"],
        ),
        definition(
            "web_fetch",
            "Fetch a public HTTP(S) page after SSRF and redirect checks.",
            json!({"url": {"type": "string"}}),
            &["url"],
        ),
        definition(
            "todo_write",
            "Replace the visible task list for a multi-step task. Call with the full list each time (pending, in_progress, completed); exactly one item may be in_progress. Use for plans the user can follow, not for single-step answers.",
            json!({"items": {"type": "array", "items": {"type": "object"}}}),
            &["items"],
        ),
    ]
}

fn definition(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": schema}
    })
}

pub async fn execute(name: &str, raw_arguments: &Value, context: &ToolContext) -> Result<String> {
    // Policy floor first (mode gate, posture, lists, ask level): no
    // caller — UI, headless, or test — can bypass it.
    let arguments = repair_arguments(raw_arguments);
    match crate::approve::resolve(
        name,
        &arguments,
        &context.mode,
        context.allow_code,
        context.allow_network,
        &context.policy,
    ) {
        crate::approve::Decision::Allow => {}
        crate::approve::Decision::Deny(reason) => bail!("tool '{name}' denied: {reason}"),
        crate::approve::Decision::Ask(summary) => bail!(
            "tool '{name}' requires approval ({summary}); approve the card in the TUI or add a matching command_allowlist pattern"
        ),
    }
    // Policy hooks see every tool call (built-in, MCP, plugin) after
    // repair: denies abort before anything runs, rewrites chain into the
    // call below and into the after-hooks' view of the arguments.
    let arguments = repair_arguments(raw_arguments);
    let arguments =
        crate::plugin::run_before_hooks(&context.workspace, &context.plugins_dir, name, &arguments)
            .await?;
    let content = match name {
        "execute_rust" => execute_rust(&arguments, context).await?,
        "write_file" => write_file(&arguments, &context.workspace)?,
        "read_file" => read_file(&arguments, &context.workspace)?,
        "list_files" => list_files(&arguments, &context.workspace)?,
        "get_time" => current_time(),
        "calculate" => calculate(&arguments)?,
        "convert" => convert(&arguments)?,
        "system_info" => system_info()?,
        "web_search" => web_search(&arguments, context).await?,
        "web_fetch" => web_fetch(&arguments, context).await?,
        "todo_write" => todo_write(&arguments, context)?,
        _ if name.starts_with("mcp_") => crate::mcp::call(name, &arguments).await?,
        _ if name.starts_with("plugin_") => {
            if !context.allow_code {
                bail!("plugin execution is disabled by the current permission posture");
            }
            crate::plugin::call_from(&context.workspace, &context.plugins_dir, name, &arguments)
                .await?
        }
        _ => bail!("unknown tool '{name}'"),
    };
    let content = truncate_output(content);
    crate::plugin::run_after_hooks(
        &context.workspace,
        &context.plugins_dir,
        name,
        &arguments,
        &content,
    )
    .await
}

pub async fn execute_calls(
    calls: &[ToolCall],
    context: &ToolContext,
    events: Option<mpsc::UnboundedSender<ToolResult>>,
) -> Result<Vec<ToolResult>> {
    let mut tasks = Vec::with_capacity(calls.len());
    for call in calls {
        let call = call.clone();
        let context = context.clone();
        tasks.push(tokio::spawn(async move {
            let value = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| Value::String(call.function.arguments.clone()));
            let content = execute(&call.function.name, &value, &context)
                .await
                .unwrap_or_else(|error| format!("tool error: {error:#}"));
            ToolResult {
                name: call.function.name,
                call_id: call.id,
                content,
            }
        }));
    }
    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        let result = task.await.context("tool worker panicked")?;
        if let Some(sender) = &events {
            let _ = sender.send(result.clone());
        }
        results.push(result);
    }
    Ok(results)
}

pub(crate) fn repair_arguments(raw: &Value) -> Value {
    match raw {
        Value::Object(_) => raw.clone(),
        Value::String(text) => {
            let mut text = text.trim();
            let fence = "\x60\x60\x60";
            if let Some(stripped) = text.strip_prefix(fence) {
                text = stripped;
                if let Some(stripped) = text.strip_prefix("json") {
                    text = stripped;
                }
                text = text.trim();
                if let Some(stripped) = text.strip_suffix(fence) {
                    text = stripped.trim();
                }
            }
            serde_json::from_str(text).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
        }
        _ => Value::Object(serde_json::Map::new()),
    }
}

fn argument_string(arguments: &Value, key: &str) -> Result<String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("{key} is required"))
}

fn write_file(arguments: &Value, workspace: &Path) -> Result<String> {
    let path = safe_path(workspace, &argument_string(arguments, "path")?)?;
    let content = argument_string(arguments, "content")?;
    validate_tool_text(&content, "content", MAX_FILE_CONTENT)?;
    let old = fs::read_to_string(&path).unwrap_or_default();
    if old == content {
        return Ok(format!("no changes for {}", path.display()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let is_new = !path.exists();
    fs::write(&path, content.as_bytes())?;
    if is_new {
        Ok(format!(
            "created {} ({} bytes)",
            path.display(),
            content.len()
        ))
    } else {
        Ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            path.display()
        ))
    }
}

fn read_file(arguments: &Value, workspace: &Path) -> Result<String> {
    let path = safe_path(workspace, &argument_string(arguments, "path")?)?;
    let metadata = fs::metadata(&path).with_context(|| format!("reading {}", path.display()))?;
    if metadata.len() > MAX_FILE_READ {
        bail!(
            "file too large ({} bytes, max {MAX_FILE_READ})",
            metadata.len()
        );
    }
    fs::read_to_string(&path).with_context(|| format!("decoding {}", path.display()))
}

fn list_files(arguments: &Value, workspace: &Path) -> Result<String> {
    let requested = arguments.get("path").and_then(Value::as_str).unwrap_or(".");
    let path = safe_path(workspace, requested)?;
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).with_context(|| format!("listing {}", path.display()))? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let kind = if metadata.is_dir() { "dir" } else { "file" };
        entries.push(format!(
            "{} ({kind}, {} bytes)",
            entry.file_name().to_string_lossy(),
            metadata.len()
        ));
    }
    entries.sort();
    Ok(if entries.is_empty() {
        "empty directory".to_string()
    } else {
        entries.join("\n")
    })
}

/// Replace the visible task list. The model sends the full list every
/// time; at most 20 items, exactly one `in_progress` (extras demote to
/// pending so a sloppy update cannot claim two active tasks).
fn todo_write(arguments: &Value, context: &ToolContext) -> Result<String> {
    use crate::model::{TodoItem, TodoStatus};

    const MAX_TODOS: usize = 20;
    let items = arguments
        .get("items")
        .and_then(Value::as_array)
        .context("todo_write needs an 'items' array")?;
    if items.len() > MAX_TODOS {
        bail!("todo_write takes at most {MAX_TODOS} items");
    }
    let mut todos = Vec::with_capacity(items.len());
    let mut active_seen = false;
    for (position, item) in items.iter().enumerate() {
        let content = item
            .get("content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .with_context(|| format!("todo item {position} needs non-empty 'content'"))?;
        if content.chars().count() > 200 {
            bail!("todo item {position} content exceeds 200 chars");
        }
        let status = match item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
        {
            "pending" => TodoStatus::Pending,
            "completed" => TodoStatus::Completed,
            "in_progress" if !active_seen => {
                active_seen = true;
                TodoStatus::InProgress
            }
            "in_progress" => TodoStatus::Pending,
            other => bail!("todo item {position} has unknown status '{other}'"),
        };
        todos.push(TodoItem {
            content: content.to_string(),
            status,
        });
    }
    let done = todos
        .iter()
        .filter(|item| item.status == TodoStatus::Completed)
        .count();
    let total = todos.len();
    context
        .todos
        .lock()
        .map_err(|_| anyhow::anyhow!("todo list lock poisoned"))?
        .clone_from(&todos);
    Ok(format!("todo list updated: {done}/{total} done"))
}

fn current_time() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(value) => format!("{} seconds since UNIX epoch", value.as_secs()),
        Err(error) => format!("system clock error: {error}"),
    }
}

fn calculate(arguments: &Value) -> Result<String> {
    let expression = argument_string(arguments, "expression")?;
    validate_tool_text(&expression, "expression", 512)?;
    let value = ExpressionParser::new(&expression).parse()?;
    Ok(format_number(value))
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < i64::MAX as f64 {
        format!("{}", value as i64)
    } else {
        format!("{value:.12}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn convert(arguments: &Value) -> Result<String> {
    let value = arguments
        .get("value")
        .and_then(Value::as_f64)
        .ok_or_else(|| anyhow::anyhow!("value must be a number"))?;
    let from = argument_string(arguments, "from_unit")?.to_ascii_lowercase();
    let to = argument_string(arguments, "to_unit")?.to_ascii_lowercase();
    let result = match (unit_category(&from), unit_category(&to)) {
        (Some((category, base)), Some((target_category, target)))
            if category == target_category =>
        {
            match category {
                "temperature" => convert_temperature(value, &from, &to)?,
                _ => value * base / target,
            }
        }
        (Some(_), Some(_)) => bail!("cannot convert between incompatible units"),
        _ => bail!("unknown unit(s): {from:?} and/or {to:?}"),
    };
    Ok(format_number(result))
}

fn unit_category(unit: &str) -> Option<(&'static str, f64)> {
    Some(match unit {
        "m" | "meter" | "meters" => ("length", 1.0),
        "km" | "kilometer" | "kilometers" => ("length", 1000.0),
        "cm" | "centimeter" | "centimeters" => ("length", 0.01),
        "mm" | "millimeter" | "millimeters" => ("length", 0.001),
        "mi" | "mile" | "miles" => ("length", 1609.344),
        "ft" | "foot" | "feet" => ("length", 0.3048),
        "in" | "inch" | "inches" => ("length", 0.0254),
        "kg" | "kilogram" | "kilograms" => ("mass", 1.0),
        "g" | "gram" | "grams" => ("mass", 0.001),
        "lb" | "pound" | "pounds" => ("mass", 0.45359237),
        "s" | "sec" | "second" | "seconds" => ("time", 1.0),
        "min" | "minute" | "minutes" => ("time", 60.0),
        "h" | "hr" | "hour" | "hours" => ("time", 3600.0),
        "b" | "byte" | "bytes" => ("data", 1.0),
        "kb" => ("data", 1000.0),
        "mb" => ("data", 1_000_000.0),
        "gb" => ("data", 1_000_000_000.0),
        "m/s" => ("speed", 1.0),
        "km/h" => ("speed", 1000.0 / 3600.0),
        "l" | "liter" | "liters" => ("volume", 1.0),
        "ml" | "milliliter" | "milliliters" => ("volume", 0.001),
        "c" | "°c" | "celsius" => ("temperature", 1.0),
        "f" | "°f" | "fahrenheit" => ("temperature", 1.0),
        "k" | "kelvin" => ("temperature", 1.0),
        _ => return None,
    })
}

fn convert_temperature(value: f64, from: &str, to: &str) -> Result<f64> {
    let celsius = match from {
        "c" | "°c" | "celsius" => value,
        "f" | "°f" | "fahrenheit" => (value - 32.0) * 5.0 / 9.0,
        "k" | "kelvin" => value - 273.15,
        _ => bail!("unknown temperature unit"),
    };
    Ok(match to {
        "c" | "°c" | "celsius" => celsius,
        "f" | "°f" | "fahrenheit" => celsius * 9.0 / 5.0 + 32.0,
        "k" | "kelvin" => celsius + 273.15,
        _ => bail!("unknown temperature unit"),
    })
}

fn system_info() -> Result<String> {
    let value = json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "current_dir": std::env::current_dir()?.display().to_string(),
        "pid": std::process::id(),
    });
    Ok(serde_json::to_string_pretty(&value)?)
}

async fn web_fetch(arguments: &Value, context: &ToolContext) -> Result<String> {
    if !context.allow_network {
        bail!("network access is disabled for this tool");
    }
    let mut current = argument_string(arguments, "url")?;
    for _ in 0..=5 {
        let parsed = validate_web_url(&current)?;
        let address = resolve_public_socket(&parsed)?;
        let host = parsed
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .resolve(host, address)
            .timeout(std::time::Duration::from_secs(20))
            .user_agent(concat!("r105/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let response = client.get(parsed.clone()).send().await?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("redirect had no valid Location header"))?;
            current = parsed.join(location)?.to_string();
            continue;
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_WEB_BODY as u64)
        {
            bail!("web response is too large (max {MAX_WEB_BODY} bytes)");
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = MAX_WEB_BODY.saturating_sub(bytes.len());
            if chunk.len() > remaining {
                bytes.extend_from_slice(&chunk[..remaining]);
                bail!("web response exceeds {MAX_WEB_BODY} bytes");
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = String::from_utf8_lossy(&bytes);
        return Ok(strip_html(&body));
    }
    bail!("too many redirects")
}

async fn web_search(arguments: &Value, context: &ToolContext) -> Result<String> {
    let query = argument_string(arguments, "query")?;
    validate_tool_text(&query, "query", MAX_SEARCH_QUERY)?;
    let url = format!(
        "https://html.duckduckgo.com/html/?q={}",
        url::form_urlencoded::byte_serialize(query.as_bytes()).collect::<String>()
    );
    let result = web_fetch(&json!({"url": url}), context).await?;
    Ok(result.lines().take(20).collect::<Vec<_>>().join("\n"))
}

fn strip_html(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_tag = false;
    for character in input.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => output.push(character),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

async fn execute_rust(arguments: &Value, context: &ToolContext) -> Result<String> {
    if !context.allow_code {
        bail!("Rust execution is disabled by the current permission posture");
    }
    let code = argument_string(arguments, "code")?;
    validate_tool_text(&code, "code", MAX_CODE_SIZE)?;
    let temp = context.workspace.join(".r105").join("runs");
    fs::create_dir_all(&temp)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let source = temp.join(format!("run-{stamp}.rs"));
    let binary = temp.join(if cfg!(windows) {
        format!("run-{stamp}.exe")
    } else {
        format!("run-{stamp}")
    });
    fs::write(&source, code)?;
    // Under the Docker backend the workspace is remounted at /workspace, so
    // host-absolute source/binary paths must be translated to guest paths.
    let guest_source = context.sandbox.guest_path(&context.workspace, &source);
    let guest_binary = context.sandbox.guest_path(&context.workspace, &binary);
    let compile = context
        .sandbox
        .run(
            "rustc",
            &[
                guest_source,
                "-O".to_string(),
                "-o".to_string(),
                guest_binary.clone(),
            ],
            &context.workspace,
            false,
            &context.cancellation,
        )
        .await?;
    if compile.status != Some(0) {
        let _ = fs::remove_file(&source);
        return Ok(compile.stderr);
    }
    let output = context
        .sandbox
        .run(
            &guest_binary,
            &[],
            &context.workspace,
            false,
            &context.cancellation,
        )
        .await?;
    let _ = fs::remove_file(&source);
    let _ = fs::remove_file(&binary);
    if output.status == Some(0) {
        Ok(output.stdout)
    } else {
        Ok(if output.stderr.is_empty() {
            format!("process exited with {:?}", output.status)
        } else {
            output.stderr
        })
    }
}

struct ExpressionParser<'a> {
    input: &'a [u8],
    position: usize,
    depth: usize,
}

impl<'a> ExpressionParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            position: 0,
            depth: 0,
        }
    }

    fn parse(mut self) -> Result<f64> {
        let value = self.expression()?;
        self.space();
        if self.position != self.input.len() {
            bail!(
                "unexpected input near '{}'",
                String::from_utf8_lossy(&self.input[self.position..])
            );
        }
        if !value.is_finite() {
            bail!("result is outside the supported numeric range");
        }
        Ok(value)
    }

    fn expression(&mut self) -> Result<f64> {
        self.binary(|parser| parser.term(), b"+-")
    }

    fn term(&mut self) -> Result<f64> {
        self.binary(|parser| parser.power(), b"*/%")
    }

    fn binary<F>(&mut self, mut next: F, operators: &[u8]) -> Result<f64>
    where
        F: FnMut(&mut Self) -> Result<f64>,
    {
        let mut value = next(self)?;
        loop {
            self.space();
            let Some(operator) = self.input.get(self.position).copied() else {
                break;
            };
            if !operators.contains(&operator)
                || operator == b'*' && self.input.get(self.position + 1) == Some(&b'*')
            {
                break;
            }
            self.position += 1;
            let right = next(self)?;
            value = match operator {
                b'+' => value + right,
                b'-' => value - right,
                b'*' => value * right,
                b'/' if right != 0.0 => value / right,
                b'%' if right != 0.0 => value % right,
                b'/' | b'%' => bail!("division by zero"),
                _ => unreachable!(),
            };
            if !value.is_finite() {
                bail!("result is outside the supported numeric range");
            }
        }
        Ok(value)
    }

    fn power(&mut self) -> Result<f64> {
        self.enter()?;
        let result = self.power_inner();
        self.depth -= 1;
        result
    }

    fn power_inner(&mut self) -> Result<f64> {
        let left = self.unary()?;
        self.space();
        if self.input.get(self.position) == Some(&b'*')
            && self.input.get(self.position + 1) == Some(&b'*')
        {
            self.position += 2;
            let exponent = self.power()?;
            if exponent.abs() > 1000.0 {
                bail!("exponent is too large");
            }
            let value = left.powf(exponent);
            if !value.is_finite() {
                bail!("result is outside the supported numeric range");
            }
            Ok(value)
        } else {
            Ok(left)
        }
    }

    fn unary(&mut self) -> Result<f64> {
        self.enter()?;
        let result = self.unary_inner();
        self.depth -= 1;
        result
    }

    fn unary_inner(&mut self) -> Result<f64> {
        self.space();
        if self.input.get(self.position) == Some(&b'+') {
            self.position += 1;
            return self.unary();
        }
        if self.input.get(self.position) == Some(&b'-') {
            self.position += 1;
            return Ok(-self.unary()?);
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<f64> {
        self.enter()?;
        self.space();
        let value = if self.input.get(self.position) == Some(&b'(') {
            self.position += 1;
            let value = self.expression()?;
            self.space();
            if self.input.get(self.position) != Some(&b')') {
                bail!("missing ')'");
            }
            self.position += 1;
            value
        } else if self
            .input
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'.')
        {
            self.number()?
        } else {
            let name = self.identifier()?;
            self.space();
            if self.input.get(self.position) == Some(&b'(') {
                self.position += 1;
                let argument = self.expression()?;
                self.space();
                if self.input.get(self.position) != Some(&b')') {
                    bail!("missing ')' after function");
                }
                self.position += 1;
                function_value(&name, argument)?
            } else {
                match name.as_str() {
                    "pi" => std::f64::consts::PI,
                    "e" => std::f64::consts::E,
                    "tau" => std::f64::consts::TAU,
                    _ => bail!("unknown name '{name}'"),
                }
            }
        };
        self.depth -= 1;
        Ok(value)
    }

    fn number(&mut self) -> Result<f64> {
        let start = self.position;
        while self.input.get(self.position).is_some_and(|byte| {
            byte.is_ascii_digit()
                || *byte == b'.'
                || *byte == b'e'
                || *byte == b'E'
                || (*byte == b'+'
                    && self.position > start
                    && matches!(
                        self.input.get(self.position.wrapping_sub(1)),
                        Some(b'e' | b'E')
                    ))
                || (*byte == b'-'
                    && self.position > start
                    && matches!(
                        self.input.get(self.position.wrapping_sub(1)),
                        Some(b'e' | b'E')
                    ))
        }) {
            self.position += 1;
        }
        let value = std::str::from_utf8(&self.input[start..self.position])?.parse::<f64>()?;
        if !value.is_finite() || value.abs() > 1e100 {
            bail!("numeric literal is too large");
        }
        Ok(value)
    }

    fn identifier(&mut self) -> Result<String> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            self.position += 1;
        }
        if start == self.position {
            bail!("expected a number, function, or constant");
        }
        Ok(String::from_utf8(
            self.input[start..self.position].to_vec(),
        )?)
    }

    fn space(&mut self) {
        while self
            .input
            .get(self.position)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.position += 1;
        }
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > 32 {
            bail!("expression nesting is too deep");
        }
        Ok(())
    }
}

fn function_value(name: &str, value: f64) -> Result<f64> {
    let result = match name {
        "sqrt" if value >= 0.0 => value.sqrt(),
        "sin" => value.sin(),
        "cos" => value.cos(),
        "tan" => value.tan(),
        "log" if value > 0.0 => value.ln(),
        "log10" if value > 0.0 => value.log10(),
        "exp" => value.exp(),
        "abs" => value.abs(),
        "floor" => value.floor(),
        "ceil" => value.ceil(),
        "round" => value.round(),
        "factorial" if (0.0..=10_000.0).contains(&value) && value.fract() == 0.0 => {
            (1..=(value as u64)).fold(1.0, |total, item| total * item as f64)
        }
        "sqrt" | "log" | "log10" | "factorial" => bail!("argument out of range for {name}"),
        _ => bail!("unknown function '{name}'"),
    };
    if result.is_finite() {
        Ok(result)
    } else {
        bail!("function result is outside the supported numeric range")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn calculation_has_resource_limits() {
        assert_eq!(
            calculate(&json!({"expression": "2 + 3 * 4"})).unwrap(),
            "14"
        );
        assert!(calculate(&json!({"expression": "factorial(10001)"})).is_err());
        assert!(calculate(&json!({"expression": "((((((((((((((((((((((((((((((((1))))))))))))))))))))))))))))))))"})).is_err());
        // Right-associative power towers and unary sign chains recurse;
        // both must hit the nesting limit instead of overflowing the stack.
        assert!(calculate(&json!({"expression": "2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2**2"})).is_err());
        assert!(
            calculate(
                &json!({"expression": "----------------------------------------------------1"})
            )
            .is_err()
        );
    }

    #[test]
    fn workspace_file_tools_are_native() {
        let directory = tempdir().unwrap();
        let path = json!({"path": "note.txt", "content": "hello"});
        assert!(write_file(&path, directory.path()).is_ok());
        assert_eq!(
            read_file(&json!({"path": "note.txt"}), directory.path()).unwrap(),
            "hello"
        );
    }

    /// A denying hook aborts `execute` before the tool runs.
    #[tokio::test]
    #[cfg(unix)]
    async fn deny_hook_blocks_execute_end_to_end() {
        use crate::sandbox::Sandbox;

        let workspace = tempdir().unwrap();
        let plugins = tempdir().unwrap();
        std::fs::write(
            plugins.path().join("gate.json"),
            r#"{"name":"gate","command":"sh","args":["-c","cat >/dev/null; echo '{\"deny\": \"nope\"}'"],"tools":[],"hooks":["before_tool"]}"#,
        )
        .unwrap();
        let context = ToolContext {
            workspace: workspace.path().to_path_buf(),
            plugins_dir: plugins.path().to_path_buf(),
            sandbox: Sandbox::detect("none", None, 5),
            cancellation: CancellationToken::new(),
            allow_network: false,
            allow_code: true,
            mode: "build".to_string(),
            policy: crate::approve::Policy::default(),
            todos: Arc::new(Mutex::new(Vec::new())),
        };
        let error = execute("calculate", &json!({"expression": "1+1"}), &context)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("denied by plugin 'gate'"),
            "{error:#}"
        );
    }

    fn mode_context(mode: &str) -> (tempfile::TempDir, tempfile::TempDir, ToolContext) {
        use crate::sandbox::Sandbox;

        let workspace = tempdir().unwrap();
        let plugins = tempdir().unwrap();
        let context = ToolContext {
            workspace: workspace.path().to_path_buf(),
            plugins_dir: plugins.path().to_path_buf(),
            sandbox: Sandbox::detect("none", None, 5),
            cancellation: CancellationToken::new(),
            allow_network: true,
            allow_code: true,
            mode: mode.to_string(),
            policy: crate::approve::Policy::default(),
            todos: Arc::new(Mutex::new(Vec::new())),
        };
        (workspace, plugins, context)
    }

    /// Ask mode refuses every real tool before any side effect runs.
    #[tokio::test]
    async fn mode_ask_denies_all_tools() {
        let (_workspace, _plugins, context) = mode_context("ask");
        for name in ["get_time", "calculate", "read_file", "web_search"] {
            let error = execute(name, &json!({}), &context).await.unwrap_err();
            assert!(
                error.to_string().contains("not available in ask mode"),
                "{name}: {error:#}"
            );
        }
    }

    /// Plan mode keeps reads and research, refuses mutation and exec.
    #[tokio::test]
    async fn mode_plan_allows_reads_denies_writes() {
        let (_workspace, _plugins, context) = mode_context("plan");
        assert!(
            execute("calculate", &json!({"expression": "1+1"}), &context)
                .await
                .is_ok()
        );
        assert!(execute("get_time", &json!({}), &context).await.is_ok());
        for name in ["write_file", "execute_rust"] {
            let error = execute(name, &json!({}), &context).await.unwrap_err();
            assert!(
                error.to_string().contains("not available in plan mode"),
                "{name}: {error:#}"
            );
        }
    }

    #[tokio::test]
    async fn mode_build_unchanged() {
        let (_workspace, _plugins, context) = mode_context("build");
        assert!(
            execute("calculate", &json!({"expression": "1+1"}), &context)
                .await
                .is_ok()
        );
        assert!(mode_allows("bogus-mode", "write_file"));
    }

    #[test]
    fn definitions_for_mode_ask_keeps_todo_write() {
        fn definition(name: &str) -> Value {
            json!({"type": "function", "function": {"name": name, "parameters": {}}})
        }
        let all = vec![definition("read_file"), definition("todo_write")];
        let ask = definitions_for_mode(all.clone(), "ask");
        assert_eq!(ask.len(), 1);
        assert_eq!(
            ask[0].pointer("/function/name").and_then(Value::as_str),
            Some("todo_write")
        );
        assert_eq!(definitions_for_mode(all, "build").len(), 2);
        assert_eq!(definitions_for_mode(vec![], "ask").len(), 0);
    }

    /// `todo_write` replaces the list; a second `in_progress` demotes.
    #[tokio::test]
    async fn todo_write_replaces_list() {
        use crate::model::TodoStatus;

        let (_workspace, _plugins, context) = mode_context("plan");
        let output = execute(
            "todo_write",
            &json!({"items": [
                {"content": "done thing", "status": "completed"},
                {"content": "active thing", "status": "in_progress"},
                {"content": "second active", "status": "in_progress"},
                {"content": "later thing"},
            ]}),
            &context,
        )
        .await
        .unwrap();
        assert!(output.contains("1/4"), "{output}");
        let todos = context.todos.lock().unwrap();
        assert_eq!(todos.len(), 4);
        assert_eq!(todos[0].status, TodoStatus::Completed);
        assert_eq!(todos[1].status, TodoStatus::InProgress);
        assert_eq!(todos[2].status, TodoStatus::Pending);
        assert_eq!(todos[3].status, TodoStatus::Pending);
    }

    /// Malformed updates fail visibly so the model can retry.
    #[tokio::test]
    async fn todo_write_rejects_bad_items() {
        let (_workspace, _plugins, context) = mode_context("build");
        assert!(
            execute(
                "todo_write",
                &json!({"items": [{"content": "x", "status": "later"}]}),
                &context
            )
            .await
            .is_err()
        );
        assert!(
            execute(
                "todo_write",
                &json!({"items": [{"content": "  "}]}),
                &context
            )
            .await
            .is_err()
        );
        let many: Vec<Value> = (0..21)
            .map(|index| json!({"content": format!("task {index}")}))
            .collect();
        assert!(
            execute("todo_write", &json!({"items": many}), &context)
                .await
                .is_err()
        );
    }

    /// The list is working state, allowed everywhere including ask.
    #[tokio::test]
    async fn todo_allowed_in_plan_mode() {
        let (_workspace, _plugins, plan) = mode_context("plan");
        let (_workspace, _plugins, ask) = mode_context("ask");
        let args = json!({"items": [{"content": "review"}]});
        assert!(execute("todo_write", &args, &plan).await.is_ok());
        assert!(execute("todo_write", &args, &ask).await.is_ok());
        assert!(
            execute("write_file", &args, &ask)
                .await
                .unwrap_err()
                .to_string()
                .contains("ask mode")
        );
    }
}
