//! Provider catalog and connection resolution.
//!
//! The catalog is deliberately metadata only. Credentials are read from the
//! current environment and are kept in memory by the running process.

use std::env;

use crate::backend::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Direct,
    Router,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Router => "router",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub backend: BackendKind,
    pub base_url: Option<&'static str>,
    pub description: &'static str,
    pub api_key_env: Option<&'static str>,
    pub api_key_required: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "opencode",
        label: "OpenCode Zen",
        backend: BackendKind::Direct,
        base_url: Some("https://opencode.ai/zen/v1"),
        description: "OpenCode curated cloud models",
        api_key_env: Some("OPENCODE_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "opencode-go",
        label: "OpenCode Go",
        backend: BackendKind::Direct,
        base_url: Some("https://opencode.ai/zen/go/v1"),
        description: "OpenCode lower-cost cloud models",
        api_key_env: Some("OPENCODE_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "router",
        label: "llama-router",
        backend: BackendKind::Router,
        base_url: Some("http://127.0.0.1:8010"),
        description: "Local router with profiles and routing metadata",
        api_key_env: None,
        api_key_required: false,
    },
    Preset {
        id: "llamacpp",
        label: "llama.cpp",
        backend: BackendKind::Direct,
        base_url: Some("http://127.0.0.1:8080/v1"),
        description: "Local llama-server OpenAI-compatible endpoint",
        api_key_env: None,
        api_key_required: false,
    },
    Preset {
        id: "ollama",
        label: "Ollama",
        backend: BackendKind::Direct,
        base_url: Some("http://127.0.0.1:11434/v1"),
        description: "Local Ollama OpenAI-compatible endpoint",
        api_key_env: None,
        api_key_required: false,
    },
    Preset {
        id: "lmstudio",
        label: "LM Studio",
        backend: BackendKind::Direct,
        base_url: Some("http://127.0.0.1:1234/v1"),
        description: "Local LM Studio OpenAI-compatible endpoint",
        api_key_env: None,
        api_key_required: false,
    },
    Preset {
        id: "vllm",
        label: "vLLM",
        backend: BackendKind::Direct,
        base_url: Some("http://127.0.0.1:8000/v1"),
        description: "Local or hosted vLLM endpoint",
        api_key_env: Some("OPENAI_API_KEY"),
        api_key_required: false,
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        backend: BackendKind::Direct,
        base_url: Some("https://api.openai.com/v1"),
        description: "OpenAI API",
        api_key_env: Some("OPENAI_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "groq",
        label: "Groq",
        backend: BackendKind::Direct,
        base_url: Some("https://api.groq.com/openai/v1"),
        description: "Groq hosted inference",
        api_key_env: Some("GROQ_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        backend: BackendKind::Direct,
        base_url: Some("https://openrouter.ai/api/v1"),
        description: "OpenRouter model gateway",
        api_key_env: Some("OPENROUTER_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "deepseek",
        label: "DeepSeek",
        backend: BackendKind::Direct,
        base_url: Some("https://api.deepseek.com/v1"),
        description: "DeepSeek API",
        api_key_env: Some("DEEPSEEK_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "together",
        label: "Together AI",
        backend: BackendKind::Direct,
        base_url: Some("https://api.together.xyz/v1"),
        description: "Together hosted inference",
        api_key_env: Some("TOGETHER_API_KEY"),
        api_key_required: true,
    },
    Preset {
        id: "custom",
        label: "Custom OpenAI-compatible API",
        backend: BackendKind::Direct,
        base_url: None,
        description: "Enter any OpenAI-compatible base URL",
        api_key_env: None,
        api_key_required: false,
    },
];

pub fn aliases(id: &str) -> String {
    match id.trim().to_ascii_lowercase().as_str() {
        "local" => "ollama".to_string(),
        "llama-router" => "router".to_string(),
        "lm-studio" => "lmstudio".to_string(),
        "llama.cpp" | "llama-cpp" => "llamacpp".to_string(),
        "opencode-zen" | "zen" => "opencode".to_string(),
        "opencodego" => "opencode-go".to_string(),
        value => value.to_string(),
    }
}

pub fn preset(id: &str) -> Option<&'static Preset> {
    let normalized = aliases(id);
    PRESETS.iter().find(|item| item.id == normalized)
}

pub fn valid_url(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && !value.chars().any(char::is_whitespace)
}

fn env_key(preset: Option<&Preset>, explicit_url: bool) -> Option<String> {
    preset
        .and_then(|item| item.api_key_env)
        .and_then(|name| env::var(name).ok())
        .or_else(|| {
            explicit_url
                .then(|| env::var("OPENAI_API_KEY").ok())
                .flatten()
        })
}

/// Resolve CLI/config/environment settings into one in-memory connection.
pub fn resolve_connection(
    provider_id: Option<&str>,
    backend_name: Option<&str>,
    explicit_url: Option<&str>,
) -> Connection {
    let selected = provider_id.and_then(preset);
    let provider = selected.map(|item| item.id.to_string());
    let env_url = env::var("R105_URL").ok();
    let inferred_url = explicit_url.or(env_url.as_deref());
    let ambient_url = explicit_url
        .map(str::to_string)
        .or_else(|| env_url.clone())
        .or_else(|| env::var("OPENAI_BASE_URL").ok());
    let backend = backend_name
        .and_then(|value| match value {
            "router" => Some(BackendKind::Router),
            "direct" => Some(BackendKind::Direct),
            _ => None,
        })
        .or_else(|| selected.map(|item| item.backend))
        .or_else(|| {
            inferred_url
                .filter(|url| url.contains(":8010"))
                .map(|_| BackendKind::Router)
        })
        .unwrap_or_else(|| {
            if ambient_url.is_some() {
                BackendKind::Direct
            } else {
                BackendKind::Router
            }
        });

    let base_url = explicit_url
        .map(str::to_string)
        .or_else(|| selected.and_then(|item| item.base_url).map(str::to_string))
        .or(env_url)
        .or_else(|| env::var("OPENAI_BASE_URL").ok())
        .unwrap_or_else(|| {
            if backend == BackendKind::Router {
                "http://127.0.0.1:8010".to_string()
            } else {
                "https://api.openai.com/v1".to_string()
            }
        });

    let base_url = if valid_url(&base_url) {
        base_url.trim_end_matches('/').to_string()
    } else {
        // The caller can surface this as a connection error. Keeping the
        // value here makes the selected URL visible in diagnostics.
        base_url.trim_end_matches('/').to_string()
    };

    let key = env_key(selected, explicit_url.is_some());
    let model = env::var("R105_MODEL").unwrap_or_else(|_| crate::model::DEFAULT_MODEL.to_string());
    Connection {
        provider_id: provider,
        backend: backend.as_str().to_string(),
        base_url,
        api_key: key,
        model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve() {
        assert_eq!(aliases("llama.cpp"), "llamacpp");
        assert_eq!(preset("zen").map(|p| p.id), Some("opencode"));
    }

    #[test]
    fn urls_reject_credentials_and_non_http_schemes() {
        assert!(valid_url("https://example.com/v1"));
        assert!(!valid_url("https://user:pass@example.com/v1"));
        assert!(!valid_url("file:///tmp/model"));
    }

    #[test]
    fn default_is_local_router() {
        let connection = resolve_connection(None, None, Some("http://127.0.0.1:8010"));
        assert_eq!(connection.backend, "router");
    }
}
