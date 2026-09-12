//! Small, provider-agnostic Server-Sent Events parser for OpenAI-compatible
//! streaming responses.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use serde_json::Value;

use crate::model::{ChatResult, ToolCall, Usage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    pending: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.pending.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();
        while let Some(boundary) = self.pending.find("\n\n") {
            let frame = self.pending[..boundary].to_string();
            self.pending.drain(..boundary + 2);
            if let Some(event) = self.parse_frame(&frame) {
                events.push(event);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        if self.pending.trim().is_empty() {
            return Vec::new();
        }
        let frame = std::mem::take(&mut self.pending);
        self.parse_frame(&frame).into_iter().collect()
    }

    fn parse_frame(&mut self, frame: &str) -> Option<SseEvent> {
        for line in frame.lines() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                self.event = Some(value.trim_start().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            }
        }
        if self.data.is_empty() {
            return None;
        }
        Some(SseEvent {
            event: self.event.take().unwrap_or_else(|| "message".to_string()),
            data: std::mem::take(&mut self.data).join("\n"),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct StreamChunk {
    pub content: String,
    pub reasoning: String,
    pub usage: Usage,
}

#[derive(Debug, Default)]
pub struct ToolAccumulator {
    calls: BTreeMap<usize, ToolCall>,
}

impl ToolAccumulator {
    pub fn apply(&mut self, delta: &Value) {
        let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };
        for value in tool_calls {
            let Some(object) = value.as_object() else {
                continue;
            };
            let index = object.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            let call = self.calls.entry(index).or_insert_with(|| ToolCall {
                id: String::new(),
                type_: "function".to_string(),
                function: crate::model::FunctionCall {
                    name: String::new(),
                    arguments: String::new(),
                },
            });
            if let Some(id) = object.get("id").and_then(Value::as_str) {
                call.id.push_str(id);
            }
            if let Some(kind) = object.get("type").and_then(Value::as_str) {
                call.type_ = kind.to_string();
            }
            if let Some(function) = object.get("function").and_then(Value::as_object) {
                if let Some(name) = function.get("name").and_then(Value::as_str) {
                    call.function.name.push_str(name);
                }
                if let Some(arguments) = function.get("arguments").and_then(Value::as_str) {
                    call.function.arguments.push_str(arguments);
                }
            }
        }
    }

    pub fn finish(self) -> Vec<ToolCall> {
        self.calls.into_values().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

pub fn parse_stream_data(data: &str, tools: &mut ToolAccumulator) -> Result<StreamChunk> {
    if data.trim() == "[DONE]" {
        return Ok(StreamChunk {
            ..StreamChunk::default()
        });
    }
    let value: Value =
        serde_json::from_str(data).map_err(|error| anyhow!("malformed SSE data: {error}"))?;
    let mut result = StreamChunk::default();
    if let Some(usage) = value.get("usage") {
        result.usage = parse_usage(usage);
    }
    let Some(choice) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    else {
        return Ok(result);
    };
    let delta = choice.get("delta").unwrap_or(&Value::Null);
    if let Some(content) = delta.get("content").and_then(Value::as_str) {
        result.content = content.to_string();
    }
    if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
        result.reasoning = reasoning.to_string();
    }
    tools.apply(delta);
    Ok(result)
}

pub fn parse_usage(value: &Value) -> Usage {
    Usage {
        prompt_tokens: value.get("prompt_tokens").and_then(Value::as_u64),
        completion_tokens: value.get("completion_tokens").and_then(Value::as_u64),
        total_tokens: value.get("total_tokens").and_then(Value::as_u64),
    }
}

pub fn parse_chat_response(value: Value, wall_seconds: f64) -> Result<ChatResult> {
    let choice = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| anyhow!("backend response did not contain choices"))?;
    let message = choice.get("message").unwrap_or(&Value::Null);
    let content = match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let usage = value.get("usage").map(parse_usage).unwrap_or_default();
    Ok(ChatResult {
        content,
        tool_calls,
        raw: value,
        usage,
        wall_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_handles_split_frames_and_done() {
        let mut parser = SseParser::default();
        assert!(
            parser
                .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n")
                .is_empty()
        );
        let events = parser.push(b"\ndata: [DONE]\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].data,
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#
        );
        assert_eq!(events[1].data, "[DONE]");
    }

    #[test]
    fn tool_arguments_are_accumulated_by_index() {
        let mut accumulator = ToolAccumulator::default();
        accumulator.apply(&serde_json::json!({"tool_calls":[{"index":0,"id":"call_","function":{"name":"read_","arguments":"{\"p"}}]}));
        accumulator.apply(&serde_json::json!({"tool_calls":[{"index":0,"id":"1","function":{"name":"file","arguments":"ath\":\"a\"}"}}]}));
        let calls = accumulator.finish();
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].function.name, "read_file");
        assert_eq!(calls[0].function.arguments, r#"{"path":"a"}"#);
    }
}
