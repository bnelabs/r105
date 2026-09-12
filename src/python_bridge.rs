//! Optional compatibility bridge for Python execution.
//!
//! The Rust binary never embeds a Python interpreter. When a user explicitly
//! configures `R105_PYTHON_BRIDGE` (or `python_bridge_command` in config.json),
//! this module sends one JSON request to that external executable. The bridge
//! can be the bundled stdlib-only reference script or a user-maintained
//! wrapper, and it runs inside the same Rust subprocess boundary as native
//! tools.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{sandbox::Sandbox, security::MAX_CODE_SIZE};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_BRIDGE_RESPONSE: usize = 200_000;

#[derive(Debug, Deserialize)]
struct BridgeResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    error: Option<String>,
}

/// Return a configured bridge command, split without invoking a shell.
pub fn command_parts(spec: Option<&str>) -> Result<Option<Vec<String>>> {
    let configured = spec
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("R105_PYTHON_BRIDGE")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });

    if let Some(value) = configured {
        let parts = split_command(&value)?;
        if parts.is_empty() {
            bail!("R105_PYTHON_BRIDGE is empty");
        }
        return Ok(Some(parts));
    }

    Ok(which::which("r105-python-bridge")
        .ok()
        .map(|path| vec![path.to_string_lossy().into_owned()]))
}

/// Describe bridge availability without exposing any command arguments that
/// might contain private paths unnecessarily.
pub fn status(spec: Option<&str>) -> String {
    match command_parts(spec) {
        Ok(Some(parts)) if which::which(&parts[0]).is_ok() => {
            format!("Python bridge available: {}", parts[0])
        }
        Ok(Some(parts)) => format!(
            "Python bridge unavailable: executable not found ({})",
            parts[0]
        ),
        Ok(None) => "Python bridge unavailable (optional; set R105_PYTHON_BRIDGE)".into(),
        Err(error) => format!("Python bridge configuration invalid: {error}"),
    }
}

/// Execute Python through the external bridge protocol.
pub async fn execute(
    spec: Option<&str>,
    code: &str,
    workspace: &Path,
    sandbox: &Sandbox,
    allow_network: bool,
    cancellation: &CancellationToken,
) -> Result<String> {
    if code.is_empty() {
        bail!("Python code is required");
    }
    if code.len() > MAX_CODE_SIZE {
        bail!(
            "Python code is too large ({} bytes, max {MAX_CODE_SIZE})",
            code.len()
        );
    }
    let parts = command_parts(spec)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Python bridge is not installed; set R105_PYTHON_BRIDGE to an executable command"
        )
    })?;

    let request = json!({
        "protocol": PROTOCOL_VERSION,
        "action": "execute",
        "code": code,
        "workspace": workspace,
        "allow_network": allow_network,
        "timeout_seconds": sandbox.timeout_seconds(),
    });
    let mut input = serde_json::to_vec(&request).context("encoding Python bridge request")?;
    input.push(b'\n');

    let program = &parts[0];
    let arguments = parts[1..].to_vec();
    let output = sandbox
        .run_with_input(
            program,
            &arguments,
            workspace,
            allow_network,
            cancellation,
            &input,
        )
        .await
        .context("running Python bridge")?;
    if output.stdout.len() > MAX_BRIDGE_RESPONSE {
        bail!("Python bridge response is too large");
    }
    if output.status != Some(0) {
        let detail = if output.stderr.trim().is_empty() {
            output.stdout
        } else {
            output.stderr
        };
        bail!(
            "Python bridge exited with {:?}: {}",
            output.status,
            detail.chars().take(800).collect::<String>()
        );
    }

    let response: BridgeResponse = serde_json::from_str(output.stdout.trim())
        .context("Python bridge returned invalid JSON")?;
    if !response.ok {
        bail!(
            "Python bridge rejected the request: {}",
            response
                .error
                .unwrap_or_else(|| "unknown bridge error".into())
        );
    }
    if response.exit_code.unwrap_or(1) == 0 {
        return Ok(response.stdout);
    }
    Ok(if response.stderr.trim().is_empty() {
        response.stdout
    } else {
        response.stderr
    })
}

fn split_command(input: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' && quote != Some('\'') {
            let Some(next) = characters.peek().copied() else {
                bail!("bridge command ends with an escape character");
            };
            // Keep ordinary backslashes intact so Windows drive and UNC paths
            // survive parsing. Backslash still quotes whitespace and quote
            // characters for shell-like commands.
            if next.is_whitespace() || next == '\'' || next == '"' {
                current.push(next);
                characters.next();
            } else {
                current.push('\\');
            }
            continue;
        }
        match (quote, character) {
            (Some(active), value) if value == active => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, value) if value.is_whitespace() => {
                if !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
            }
            (_, value) => current.push(value),
        }
    }
    if quote.is_some() {
        bail!("bridge command has an unterminated quote");
    }
    if !current.is_empty() {
        parts.push(current);
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_configured_command_without_shell_expansion() {
        assert_eq!(
            split_command("python3 '/tmp/r105 bridge.py' --stdio").unwrap(),
            vec!["python3", "/tmp/r105 bridge.py", "--stdio"]
        );
        assert!(split_command("python3 '").is_err());
        assert!(split_command("python3 \\").is_err());
        assert_eq!(
            split_command(r#"C:\Python314\python.exe C:\r105\bridge.py"#).unwrap(),
            vec![r#"C:\Python314\python.exe"#, r#"C:\r105\bridge.py"#]
        );
    }

    #[test]
    fn explicit_command_wins_over_environment_lookup() {
        assert_eq!(
            command_parts(Some("r105-python-bridge --stdio")).unwrap(),
            Some(vec!["r105-python-bridge".into(), "--stdio".into()])
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn executes_a_bridge_request_inside_the_subprocess_boundary() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("bridge.sh");
        fs::write(
            &script,
            "#!/bin/sh\nread request\nprintf '%s\\n' '{\"ok\":true,\"exit_code\":0,\"stdout\":\"bridge ok\\n\",\"stderr\":\"\"}'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();
        let sandbox = Sandbox::detect("none", None, 10);
        let result = execute(
            Some(&format!("{} --stdio", script.display())),
            "print(1)",
            directory.path(),
            &sandbox,
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(result, "bridge ok\n");
    }
}
