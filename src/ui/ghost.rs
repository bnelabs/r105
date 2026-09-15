//! UiApp ghost text: debounce, sidecar lifecycle, and `/completion`.
//!
//! The tick owns scheduling: input changes invalidate the shown ghost
//! and bump the generation (dropping stale flights), qualifying input
//! debounces into at most one in-flight request, and readiness applies
//! only to the current generation. Everything fails silent — the
//! deterministic menus are always underneath.

use std::time::Duration;

use super::*;
use crate::ghost::ghost_prefix;

/// Port parsed from the endpoint URL; the sidecar binds it on spawn.
fn endpoint_port(endpoint: &str) -> u16 {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.port())
        .unwrap_or(11438)
}

fn endpoint_reachable(endpoint: &str) -> bool {
    let socket = url::Url::parse(endpoint).ok().and_then(|url| {
        url.socket_addrs(|| None)
            .ok()
            .and_then(|mut addrs| addrs.pop())
    });
    socket.is_some_and(|address| {
        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
    })
}

impl UiApp {
    /// 45ms-tick scheduler: invalidate on edit, debounce, spawn at most
    /// one request, and lazily start the sidecar once per run.
    pub(crate) fn tick_ghost(&mut self) {
        let client = match &self.ghost_client {
            Some(client) => client.clone(),
            None => return,
        };
        if self.input != self.ghost_seen_input {
            self.ghost_seen_input = self.input.clone();
            self.ghost_changed_at = Instant::now();
            self.ghost_generation = self.ghost_generation.wrapping_add(1);
            self.ghost_text = None;
        }
        if self.ghost_inflight && self.ghost_request == self.input {
            return;
        }
        if self.ghost_text.is_some() || !matches!(self.overlay, Overlay::None) || self.busy {
            return;
        }
        let Some(prefix) = ghost_prefix(&self.input) else {
            return;
        };
        if self.ghost_changed_at.elapsed() < self.ghost_debounce {
            return;
        }
        if !endpoint_reachable(&self.ghost_endpoint) {
            if self.sidecar_attempted {
                return;
            }
            self.sidecar_attempted = true;
            let notice = self.ensure_sidecar();
            self.set_status(notice);
            if !endpoint_reachable(&self.ghost_endpoint) {
                return;
            }
        }
        let generation = self.ghost_generation;
        self.ghost_inflight = true;
        self.ghost_request = self.input.clone();
        let sender = self.tx.clone();
        tokio::spawn(async move {
            let text = client.complete(&prefix, 24).await;
            let _ = sender.send(UiEvent::GhostReady { generation, text });
        });
    }

    /// Apply a finished request iff it is still current. Stale flights
    /// (superseded by an edit) only release the flag when nothing newer
    /// is flying.
    pub(crate) fn on_ghost_ready(&mut self, generation: u64, text: Option<String>) {
        if generation != self.ghost_generation {
            return;
        }
        self.ghost_inflight = false;
        if self.input != self.ghost_request {
            return;
        }
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.ghost_text = Some(text);
        }
    }

    /// Accept the visible ghost when the cursor is at the end of input.
    pub(crate) fn accept_ghost(&mut self) -> bool {
        if self.cursor != self.input.len() {
            return false;
        }
        if let Some(ghost) = self.ghost_text.take() {
            self.input.push_str(&ghost);
            self.cursor = self.input.len();
            self.ghost_generation = self.ghost_generation.wrapping_add(1);
            self.ghost_seen_input = self.input.clone();
            return true;
        }
        false
    }

    /// Dismiss without accepting; the debounce restarts from the edit.
    pub(crate) fn dismiss_ghost(&mut self) -> bool {
        if self.ghost_text.take().is_none() {
            return false;
        }
        self.ghost_generation = self.ghost_generation.wrapping_add(1);
        true
    }

    fn resolved_model_path(&self) -> PathBuf {
        if !self.ghost_model.is_empty() {
            return PathBuf::from(&self.ghost_model).expanduser();
        }
        self.paths
            .config_dir
            .join("models")
            .join("qwen2.5-coder-0.5b-q8_0.gguf")
    }

    /// Start the sidecar if needed; returns the status-line message.
    pub(crate) fn ensure_sidecar(&mut self) -> String {
        if let Some(handle) = &mut self.sidecar
            && handle.alive()
        {
            return format!("Ghost sidecar running (pid {})", handle.pid());
        }
        if endpoint_reachable(&self.ghost_endpoint) {
            return format!("Ghost endpoint already live at {}", self.ghost_endpoint);
        }
        let model = self.resolved_model_path();
        if !model.exists() {
            return format!(
                "Ghost model missing at {} · fetch it: curl -L -o {} https://huggingface.co/ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF/resolve/main/qwen2.5-coder-0.5b-q8_0.gguf",
                model.display(),
                model.display()
            );
        }
        let server = match which::which("llama-server") {
            Ok(path) => path.to_string_lossy().to_string(),
            Err(_) => {
                return "llama-server not found · install llama.cpp (brew install llama.cpp)"
                    .into();
            }
        };
        let port = endpoint_port(&self.ghost_endpoint);
        match crate::ghost::spawn_sidecar(&server, &model.to_string_lossy(), port) {
            Ok(handle) => {
                let pid = handle.pid();
                self.sidecar = Some(handle);
                format!("Ghost sidecar starting (pid {pid}) · first suggestion warms up")
            }
            Err(error) => format!("Ghost sidecar failed to start: {error:#}"),
        }
    }

    pub(crate) async fn command_completion(&mut self, args: &[String]) {
        match args.first().map(String::as_str).unwrap_or("status") {
            "start" => {
                let notice = self.ensure_sidecar();
                self.set_status(notice);
            }
            "stop" => {
                self.sidecar = None;
                self.ghost_text = None;
                self.ghost_inflight = false;
                self.set_status("Ghost sidecar stopped".into());
            }
            "status" => {
                let client = match &self.ghost_client {
                    Some(client) => client.clone(),
                    None => {
                        self.set_status(
                            "Ghost completion disabled (completion_enabled=false)".into(),
                        );
                        return;
                    }
                };
                let live = client.healthy().await;
                let model = self.resolved_model_path();
                self.set_status(format!(
                    "Ghost {} · {} · model {}",
                    if live { "live" } else { "down" },
                    self.ghost_endpoint,
                    if model.exists() {
                        model.display().to_string()
                    } else {
                        format!("missing at {}", model.display())
                    }
                ));
            }
            other => self.set_error(format!(
                "Usage: /completion [status|start|stop] (got {other})"
            )),
        }
    }
}
