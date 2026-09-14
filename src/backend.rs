//! OpenAI-compatible HTTP backend with native SSE streaming.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::{Client, Response, header};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{
    model::{ChatResult, ChatState, Message, Usage},
    sse::{SseParser, ToolAccumulator, parse_chat_response, parse_stream_data},
};

#[derive(Debug, Clone)]
pub struct Connection {
    pub provider_id: Option<String>,
    pub backend: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl Connection {
    pub fn is_router(&self) -> bool {
        self.backend == "router"
    }

    pub fn display_name(&self) -> &str {
        self.provider_id.as_deref().unwrap_or_else(|| {
            if self.is_router() {
                "llama-router"
            } else {
                "OpenAI-compatible"
            }
        })
    }
}

#[derive(Debug, Clone)]
pub enum BackendEvent {
    Token(String),
    Status(String),
}

#[derive(Clone)]
pub struct Backend {
    client: Client,
    connection: Connection,
    timeout: Duration,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Backend")
            .field("connection", &self.connection)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Backend {
    pub fn new(connection: Connection, timeout_seconds: u64) -> Result<Self> {
        if !crate::provider::valid_url(&connection.base_url) {
            bail!("invalid backend URL: {}", connection.base_url);
        }
        let timeout = Duration::from_secs(timeout_seconds.max(1));
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(timeout)
            // Redirects are handled explicitly by web tools. API requests
            // must never silently leave the selected provider.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("r105/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            client,
            connection,
            timeout,
        })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn with_connection(&self, connection: Connection) -> Result<Self> {
        Self::new(connection, self.timeout.as_secs())
    }

    fn endpoint(&self, path: &str) -> String {
        let base = self.connection.base_url.trim_end_matches('/');
        if base.ends_with("/v1") && path.starts_with("/v1/") {
            format!("{base}{}", &path[3..])
        } else {
            format!("{base}{path}")
        }
    }

    fn headers(&self, trace_id: Option<&str>) -> reqwest::header::HeaderMap {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );
        if let Some(key) = self
            .connection
            .api_key
            .as_deref()
            .filter(|key| !key.is_empty())
            && let Ok(value) = header::HeaderValue::from_str(&format!("Bearer {key}"))
        {
            headers.insert(header::AUTHORIZATION, value);
        }
        if let Some(trace) = trace_id
            && let Ok(value) = header::HeaderValue::from_str(trace)
        {
            headers.insert("x-r105-trace-id", value);
        }
        headers
    }

    fn payload(
        &self,
        state: &ChatState,
        messages: &[Message],
        tools: &[Value],
        stream: bool,
    ) -> Value {
        let mut payload = json!({
            "model": state.model,
            "messages": messages,
            "stream": stream,
        });
        if !tools.is_empty() {
            payload["tools"] = Value::Array(tools.to_vec());
            payload["tool_choice"] = json!("auto");
        }
        if let Some(max_tokens) = state.max_tokens {
            payload["max_tokens"] = json!(max_tokens);
        }
        if state.json_mode {
            payload["response_format"] = json!({"type": "json_object"});
        }
        if state.cache_prompt && self.is_llama_cpp() {
            payload["cache_prompt"] = json!(true);
        }
        if self.connection.is_router() {
            payload["metadata"] = json!({
                "profile": state.profile,
                "quality": state.quality,
                "reasoning_effort": state.reasoning_effort,
                "trace_id": state.trace_id,
            });
        } else if state.reasoning_effort != "auto" {
            payload["reasoning_effort"] = json!(state.reasoning_effort);
        }
        payload
    }

    fn is_llama_cpp(&self) -> bool {
        // Only the explicit provider id opts into llama.cpp extensions.
        // Matching on a port substring (e.g. ":8080") misfires for any
        // unrelated service on that port.
        self.connection
            .provider_id
            .as_deref()
            .is_some_and(|id| id == "llamacpp")
    }

    async fn check_response(response: Response) -> Result<Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let detail = body.chars().take(600).collect::<String>();
        bail!("backend returned {status}: {detail}");
    }

    async fn post_json(&self, path: &str, payload: &Value, trace_id: &str) -> Result<Value> {
        let response = self
            .client
            .post(self.endpoint(path))
            .headers(self.headers(Some(trace_id)))
            .json(payload)
            .send()
            .await
            .with_context(|| format!("connecting to {}", self.connection.base_url))?;
        let response = Self::check_response(response).await?;
        response.json().await.context("decoding backend JSON")
    }

    pub async fn chat(
        &self,
        state: &ChatState,
        prompt: &str,
        tools: &[Value],
    ) -> Result<ChatResult> {
        let started = Instant::now();
        let messages = state.prompt_messages(Some(prompt));
        let payload = self.payload(state, &messages, tools, false);
        let value = self
            .post_json("/v1/chat/completions", &payload, &state.trace_id)
            .await?;
        parse_chat_response(value, started.elapsed().as_secs_f64())
    }

    pub async fn stream_chat(
        &self,
        state: &ChatState,
        prompt: &str,
        tools: &[Value],
        events: mpsc::UnboundedSender<BackendEvent>,
        cancellation: CancellationToken,
    ) -> Result<ChatResult> {
        self.stream_with_prompt(state, Some(prompt), tools, events, cancellation)
            .await
    }

    pub async fn stream_continue(
        &self,
        state: &ChatState,
        tools: &[Value],
        events: mpsc::UnboundedSender<BackendEvent>,
        cancellation: CancellationToken,
    ) -> Result<ChatResult> {
        self.stream_with_prompt(state, None, tools, events, cancellation)
            .await
    }

    async fn stream_with_prompt(
        &self,
        state: &ChatState,
        prompt: Option<&str>,
        tools: &[Value],
        events: mpsc::UnboundedSender<BackendEvent>,
        cancellation: CancellationToken,
    ) -> Result<ChatResult> {
        let started = Instant::now();
        let messages = state.prompt_messages(prompt);
        let payload = self.payload(state, &messages, tools, true);
        let response = tokio::select! {
            _ = cancellation.cancelled() => bail!("request cancelled"),
            response = self.client
                .post(self.endpoint("/v1/chat/completions"))
                .headers(self.headers(Some(&state.trace_id)))
                .json(&payload)
                .send() => response.context("connecting to backend")?,
        };
        let response = Self::check_response(response).await?;
        let mut stream = response.bytes_stream();
        let mut parser = SseParser::default();
        let mut tool_calls = ToolAccumulator::default();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut usage = Usage::default();
        let mut finished = false;

        while !finished {
            let Some(next) = (tokio::select! {
                _ = cancellation.cancelled() => return Err(anyhow!("request cancelled")),
                next = stream.next() => next,
            }) else {
                break;
            };
            let bytes = next.context("reading backend stream")?;
            for event in parser.push(&bytes) {
                if event.event == "error" {
                    bail!("backend stream error: {}", event.data);
                }
                if event.data.trim() == "[DONE]" {
                    finished = true;
                    break;
                }
                let chunk = match parse_stream_data(&event.data, &mut tool_calls) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = events.send(BackendEvent::Status(format!(
                            "skipped malformed SSE frame: {error}"
                        )));
                        continue;
                    }
                };
                if !chunk.content.is_empty() {
                    content.push_str(&chunk.content);
                    let _ = events.send(BackendEvent::Token(chunk.content));
                }
                reasoning.push_str(&chunk.reasoning);
                if chunk.usage.total_tokens.is_some() {
                    usage = chunk.usage;
                }
            }
        }
        for event in parser.finish() {
            if event.data.trim() == "[DONE]" {
                continue;
            }
            if let Ok(chunk) = parse_stream_data(&event.data, &mut tool_calls) {
                content.push_str(&chunk.content);
                reasoning.push_str(&chunk.reasoning);
                if chunk.usage.total_tokens.is_some() {
                    usage = chunk.usage;
                }
            }
        }
        if content.is_empty() && tool_calls.is_empty() && !reasoning.is_empty() {
            content = reasoning;
        }
        let calls = tool_calls.finish();
        let raw = json!({
            "choices": [{"message": {"role": "assistant", "content": content, "tool_calls": calls}}],
            "usage": usage,
        });
        Ok(ChatResult {
            content,
            tool_calls: serde_json::from_value(raw["choices"][0]["message"]["tool_calls"].clone())
                .unwrap_or_default(),
            raw,
            usage,
            wall_seconds: started.elapsed().as_secs_f64(),
        })
    }

    pub async fn list_models(&self) -> Result<Value> {
        let path = "/v1/models";
        let response = self
            .client
            .get(self.endpoint(path))
            .headers(self.headers(None))
            .send()
            .await
            .context("listing backend models")?;
        let response = Self::check_response(response).await?;
        response.json().await.context("decoding model list")
    }

    pub async fn health(&self) -> Result<Value> {
        let path = if self.connection.is_router() {
            "/health"
        } else {
            "/v1/models"
        };
        let response = self
            .client
            .get(self.endpoint(path))
            .headers(self.headers(None))
            .send()
            .await
            .context("checking backend health")?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({"body": body}));
        Ok(json!({
            "ok": status.is_success(),
            "status": status.as_u16(),
            "provider": self.connection.display_name(),
            "backend": self.connection.backend,
            "url": self.connection.base_url,
            "response": parsed,
        }))
    }

    pub async fn profiles(&self) -> Result<Value> {
        if !self.connection.is_router() {
            bail!("profiles are only available with llama-router");
        }
        let response = self
            .client
            .get(self.endpoint("/profiles"))
            .headers(self.headers(None))
            .send()
            .await
            .context("listing router profiles")?;
        let response = Self::check_response(response).await?;
        response.json().await.context("decoding router profiles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_does_not_duplicate_v1() {
        let connection = Connection {
            provider_id: Some("llamacpp".into()),
            backend: "direct".into(),
            base_url: "http://127.0.0.1:8080/v1".into(),
            api_key: None,
            model: "local".into(),
        };
        let backend = Backend::new(connection, 10).unwrap();
        assert_eq!(
            backend.endpoint("/v1/models"),
            "http://127.0.0.1:8080/v1/models"
        );
    }

    #[test]
    fn llama_cpp_detection_requires_the_provider_id() {
        let local = |provider: Option<&str>| Backend {
            client: Client::builder().build().unwrap(),
            connection: Connection {
                provider_id: provider.map(str::to_string),
                backend: "direct".into(),
                base_url: "http://127.0.0.1:8080/v1".into(),
                api_key: None,
                model: "local".into(),
            },
            timeout: Duration::from_secs(10),
        };
        assert!(local(Some("llamacpp")).is_llama_cpp());
        // Any unrelated service can listen on :8080; the port alone
        // must not opt into llama.cpp-only request fields.
        assert!(!local(Some("custom")).is_llama_cpp());
        assert!(!local(None).is_llama_cpp());
    }
}
