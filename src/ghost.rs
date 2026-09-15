//! Ghost-text completion: neural assist via a local `llama-server`
//! sidecar running a Qwen-Coder GGUF. All failure modes are silent by
//! design — deterministic completion (palette, file/arg menus) is the
//! fallback, so a missing or slow sidecar never degrades the composer.

use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::json;

/// llama.cpp native completion endpoint served by `llama-server`.
fn completion_url(endpoint: &str) -> String {
    format!("{}/completion", endpoint.trim_end_matches('/'))
}

/// Health endpoint served by `llama-server`.
fn health_url(endpoint: &str) -> String {
    format!("{}/health", endpoint.trim_end_matches('/'))
}

/// Fill-in-the-middle prompt for Qwen-Coder models. The suffix is empty
/// in v1 (the composer is single-line); the model completes the prefix.
pub fn fim_prompt(prefix: &str) -> String {
    format!("<|fim_prefix|>{prefix}<|fim_suffix|><|fim_middle|>")
}

/// Reduce raw sidecar output to one ghost line: first line only, echo of
/// the typed prefix stripped, empty/identical/no-op results rejected.
pub fn clean_completion(prefix: &str, raw: &str) -> Option<String> {
    let line = raw.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    // llama.cpp may echo the prompt tail with `n_predict` overflow.
    let suffix = line.strip_prefix(prefix.trim()).unwrap_or(line).trim();
    let suffix = suffix
        .strip_prefix(prefix.trim_end())
        .unwrap_or(suffix)
        .trim();
    if suffix.is_empty() || suffix == prefix.trim() {
        return None;
    }
    Some(suffix.chars().take(120).collect())
}

/// Whether the composer text qualifies for ghost completion; returns
/// the model prefix (`!` and `/sh ` markers strip to the raw command).
/// Single-line shell drafts only in v1.
pub fn ghost_prefix(input: &str) -> Option<String> {
    if input.len() < 3 {
        return None;
    }
    if let Some(rest) = input.strip_prefix("/sh ") {
        return (!rest.trim().is_empty()).then(|| rest.to_string());
    }
    if let Some(rest) = input.strip_prefix('!') {
        return (!rest.trim().is_empty()).then(|| rest.to_string());
    }
    None
}

/// Thin client over the sidecar's `/completion` endpoint.
#[derive(Debug, Clone)]
pub struct GhostClient {
    endpoint: String,
    client: Client,
}

impl GhostClient {
    pub fn new(endpoint: &str, timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("r105-ghost/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building ghost HTTP client")?;
        Ok(Self {
            endpoint: endpoint.to_string(),
            client,
        })
    }

    pub async fn healthy(&self) -> bool {
        self.client
            .get(health_url(&self.endpoint))
            .timeout(Duration::from_millis(500))
            .send()
            .await
            .and_then(|response| response.error_for_status())
            .is_ok()
    }

    /// Complete `prefix`; `None` on any failure or unusable output.
    pub async fn complete(&self, prefix: &str, max_tokens: u32) -> Option<String> {
        let response = self
            .client
            .post(completion_url(&self.endpoint))
            .json(&json!({
                "prompt": fim_prompt(prefix),
                "n_predict": max_tokens,
                "temperature": 0.2,
                "top_p": 0.95,
                "cache_prompt": true,
                "stop": ["<|fim_", "<|im_", "\n\n"],
            }))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: serde_json::Value = response.json().await.ok()?;
        let text = value.get("content")?.as_str()?;
        clean_completion(prefix, text)
    }
}

/// Spawn `llama-server` for `model_path` on `port`. The child is killed
/// when the returned handle drops (kill-on-drop).
pub fn spawn_sidecar(server_binary: &str, model_path: &str, port: u16) -> Result<SidecarHandle> {
    let child = std::process::Command::new(server_binary)
        .args([
            "-m",
            model_path,
            "--port",
            &port.to_string(),
            "-c",
            "2048",
            "--log-disable",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("spawning {server_binary}"))?;
    // Give the server a moment to bind before the first health check;
    // readiness itself is polled by the caller.
    std::thread::sleep(Duration::from_millis(300));
    let pid = child.id();
    Ok(SidecarHandle {
        child: Some(child),
        pid,
    })
}

pub struct SidecarHandle {
    child: Option<std::process::Child>,
    pid: u32,
}

impl SidecarHandle {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn alive(&mut self) -> bool {
        matches!(
            self.child.as_mut().and_then(|child| child.try_wait().ok()),
            Some(None)
        )
    }
}

impl Drop for SidecarHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIM prompt shape the sidecar contract depends on.
    #[test]
    fn ghost_fim_prompt_shape() {
        assert_eq!(
            fim_prompt("git sta"),
            "<|fim_prefix|>git sta<|fim_suffix|><|fim_middle|>"
        );
    }

    /// Trigger shape: `!` and `/sh ` shell lines of length ≥ 3 only.
    #[test]
    fn ghost_trigger_shape() {
        assert_eq!(ghost_prefix("!git sta"), Some("git sta".to_string()));
        assert_eq!(ghost_prefix("/sh git sta"), Some("git sta".to_string()));
        assert_eq!(ghost_prefix("!ls"), Some("ls".to_string()));
        assert_eq!(ghost_prefix("!l"), None);
        assert_eq!(ghost_prefix("/sh"), None);
        assert_eq!(ghost_prefix("hello world"), None);
        assert_eq!(ghost_prefix(""), None);
    }

    /// Echoes, blanks, and multi-line output reduce to one line or None.
    #[test]
    fn ghost_clean_rejects_echo() {
        assert_eq!(
            clean_completion("git sta", "git status\nmore"),
            Some("tus".to_string())
        );
        assert_eq!(
            clean_completion("git sta", "  status  "),
            Some("status".to_string())
        );
        assert_eq!(clean_completion("git sta", "git sta"), None);
        assert_eq!(clean_completion("git sta", "   \n  "), None);
        assert_eq!(clean_completion("git sta", ""), None);
    }

    /// Live contract check: requires R105_LIVE_GHOST=1 and a sidecar on
    /// the default endpoint (measured numbers live in spec 0015).
    #[tokio::test]
    async fn ghost_live_sidecar_smoke() {
        if std::env::var("R105_LIVE_GHOST").as_deref() != Ok("1") {
            return;
        }
        let client = GhostClient::new("http://127.0.0.1:11438", Duration::from_secs(5)).unwrap();
        assert!(client.healthy().await, "sidecar down");
        let started = std::time::Instant::now();
        let text = client
            .complete("docker ps --format", 24)
            .await
            .expect("completion");
        assert!(text.contains("{{.ID}}"), "{text}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "too slow: {:?}",
            started.elapsed()
        );
    }
}
