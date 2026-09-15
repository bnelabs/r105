use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::Config;

pub const DEFAULT_MODEL: &str = "gemma-4-12b-it";
pub const DEFAULT_CONTEXT_TOKENS: u64 = 262_144;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub type_: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".to_string()
}

/// One entry of the model-maintained task list (`todo_write` tool).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: String,
    /// Stable per-message key for transcript section state (per-block
    /// expand). Empty in files written before v1.4 and assigned lazily
    /// by the TUI, so old sessions keep loading unchanged.
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
            id: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            id: String::new(),
            tool_calls,
            tool_call_id: None,
            name: None,
        }
    }

    pub fn tool(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            id: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
            name: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
            id: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            name: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResult {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub raw: Value,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub wall_seconds: f64,
}

impl Default for ChatResult {
    fn default() -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            raw: Value::Null,
            usage: Usage::default(),
            wall_seconds: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub used_tokens: u64,
    pub context_tokens: u64,
    pub source: String,
    pub confidence: f32,
}

impl TokenUsage {
    pub fn percent(&self) -> f32 {
        if self.context_tokens == 0 {
            return 0.0;
        }
        ((self.used_tokens as f32 / self.context_tokens as f32) * 100.0).min(100.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatState {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub quality: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub json_mode: bool,
    #[serde(default = "default_true")]
    pub auto_compact: bool,
    #[serde(default)]
    pub cache_prompt: bool,
    #[serde(default)]
    pub keybindings: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_reasoning")]
    pub reasoning_effort: String,
    #[serde(default = "default_true")]
    pub show_thinking: bool,
    #[serde(default)]
    pub thinking_default_expanded: bool,
    #[serde(default = "default_posture")]
    pub permission_posture: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub skills_dir: PathBuf,
    #[serde(default)]
    pub active_skills: Vec<String>,
    #[serde(default)]
    pub skill_params:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub model_contexts: std::collections::BTreeMap<String, u64>,
    #[serde(default = "default_context")]
    pub context_tokens: u64,
    #[serde(default)]
    pub history: Vec<Message>,
    /// Working task list, replaced wholesale by `todo_write`. Session
    /// files do not persist it (working state, like scroll position);
    /// the tool-result messages in history keep the record.
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    #[serde(default)]
    pub workspace: PathBuf,
    #[serde(default = "new_trace_id")]
    pub trace_id: String,
    #[serde(default)]
    pub last_usage: Usage,
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    "r105".to_string()
}

fn default_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_reasoning() -> String {
    "auto".to_string()
}

fn default_posture() -> String {
    "sandboxed".to_string()
}

fn default_mode() -> String {
    "build".to_string()
}

fn default_context() -> u64 {
    DEFAULT_CONTEXT_TOKENS
}

fn new_trace_id() -> String {
    Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// One-line instruction prepended to the request when the session mode
/// is not build, so the model does not spam calls the gate will deny.
pub fn mode_preamble(mode: &str) -> Option<&'static str> {
    match mode {
        "plan" => Some(
            "Mode: plan. Investigate and propose only; do not call tools that \
             modify files or execute code. Read-only and web-research tools remain available.",
        ),
        "ask" => Some(
            "Mode: ask. Answer directly from knowledge and the conversation; \
             do not call tools.",
        ),
        _ => None,
    }
}

impl ChatState {
    pub fn from_config(config: &Config, workspace: PathBuf) -> Self {
        Self {
            profile: config.profile.clone(),
            quality: config.quality.clone(),
            max_tokens: None,
            json_mode: false,
            auto_compact: config.auto_compact,
            cache_prompt: config.cache_prompt,
            keybindings: config.keybindings.clone(),
            theme: config.theme.clone(),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            reasoning_effort: config.reasoning_effort.clone(),
            show_thinking: config.show_thinking,
            thinking_default_expanded: config.thinking_default_expanded,
            permission_posture: config.permission_posture.clone(),
            mode: default_mode(),
            skills_dir: config.skills_dir.clone(),
            active_skills: Vec::new(),
            skill_params: std::collections::BTreeMap::new(),
            model_contexts: config.model_contexts.clone(),
            context_tokens: config.context_tokens.unwrap_or(DEFAULT_CONTEXT_TOKENS),
            history: Vec::new(),
            todos: Vec::new(),
            workspace,
            trace_id: new_trace_id(),
            last_usage: Usage::default(),
        }
    }

    pub fn token_usage(&self) -> TokenUsage {
        if let Some(total) = self.last_usage.total_tokens {
            return TokenUsage {
                used_tokens: total,
                context_tokens: self.context_tokens,
                source: "backend".to_string(),
                confidence: 1.0,
            };
        }

        let chars: usize = self.history.iter().map(|m| m.content.len()).sum();
        let used = (chars as u64).div_ceil(4);
        TokenUsage {
            used_tokens: used,
            context_tokens: self.context_tokens,
            source: "heuristic".to_string(),
            confidence: if self.history.is_empty() { 1.0 } else { 0.35 },
        }
    }

    pub fn prompt_messages(&self, user_message: Option<&str>) -> Vec<Message> {
        let mut messages = Vec::with_capacity(self.history.len() + 2);
        if let Some(preamble) = mode_preamble(&self.mode) {
            messages.push(Message::system(preamble));
        }
        for skill in &self.active_skills {
            let name = skill.strip_suffix(".md").unwrap_or(skill);
            let path = PathBuf::from(name);
            let content = if name.is_empty()
                || path.is_absolute()
                || path.components().count() != 1
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                format!("Skill reference rejected: {skill}")
            } else {
                let path = self.skills_dir.join(format!("{name}.md"));
                let mut content = std::fs::read_to_string(path).unwrap_or_else(|_| skill.clone());
                if let Some(params) = self
                    .skill_params
                    .get(skill)
                    .or_else(|| self.skill_params.get(name))
                {
                    for (key, value) in params {
                        content = content.replace(&format!("{{{key}}}"), value);
                    }
                }
                content
            };
            messages.push(Message::system(content));
        }
        messages.extend(self.history.clone());
        if let Some(message) = user_message {
            messages.push(Message::user(message));
        }
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state_with_mode(mode: &str) -> ChatState {
        let mut state: ChatState = serde_json::from_value(json!({})).unwrap();
        state.mode = mode.to_string();
        state
    }

    #[test]
    fn mode_preamble_present_only_when_set() {
        assert!(mode_preamble("build").is_none());
        assert!(mode_preamble("bogus").is_none());
        let plan = mode_preamble("plan").unwrap();
        assert!(plan.contains("plan") && plan.contains("Read-only"));
        let ask = mode_preamble("ask").unwrap();
        assert!(ask.contains("ask") && ask.contains("do not call tools"));
    }

    #[test]
    fn mode_preamble_heads_prompt_messages() {
        let state = state_with_mode("plan");
        let messages = state.prompt_messages(Some("look around"));
        assert_eq!(
            messages.first().map(|message| message.role.as_str()),
            Some("system")
        );
        assert!(messages.first().unwrap().content.contains("Mode: plan"));
        let build = state_with_mode("build");
        let messages = build.prompt_messages(Some("hi"));
        assert!(
            messages
                .iter()
                .all(|message| !message.content.contains("Mode: plan"))
        );
    }

    #[test]
    fn mode_defaults_to_build_and_roundtrips() {
        let state: ChatState = serde_json::from_value(json!({})).unwrap();
        assert_eq!(state.mode, "build");
        let planned = state_with_mode("plan");
        let back: ChatState =
            serde_json::from_value(serde_json::to_value(&planned).unwrap()).unwrap();
        assert_eq!(back.mode, "plan");
    }
}
