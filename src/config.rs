use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::backend::Connection;

pub const THEMES: [&str; 4] = ["r105", "dracula", "solarized-dark", "high-contrast"];

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub sessions_dir: PathBuf,
    pub plugins_dir: PathBuf,
}

impl ConfigPaths {
    pub fn discover() -> Self {
        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config_root = env::var_os("R105_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_CONFIG_HOME").map(|path| PathBuf::from(path).join("r105")))
            .unwrap_or_else(|| home.join(".config").join("r105"));
        Self {
            home,
            config_file: config_root.join("config.json"),
            sessions_dir: config_root.join("sessions"),
            plugins_dir: config_root.join("plugins"),
            config_dir: config_root,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub workspace: Option<PathBuf>,
    pub skills_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub quality: Option<String>,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub auto_compact: bool,
    pub cache_prompt: bool,
    pub keybindings: BTreeMap<String, String>,
    pub sandbox_backend: String,
    pub permission_posture: String,
    #[serde(default = "default_approval_ask")]
    pub approval_exec: String,
    #[serde(default = "default_approval_ask")]
    pub approval_write: String,
    #[serde(default = "default_approval_allow")]
    pub approval_read: String,
    #[serde(default = "default_approval_allow")]
    pub approval_network: String,
    #[serde(default = "default_approval_ask")]
    pub approval_mcp: String,
    #[serde(default = "default_approval_ask")]
    pub approval_plugin: String,
    pub command_allowlist: Vec<String>,
    pub command_denylist: Vec<String>,
    #[serde(default = "default_completion_on")]
    pub completion_enabled: bool,
    #[serde(default = "default_completion_debounce_ms")]
    pub completion_debounce_ms: u64,
    #[serde(default = "default_completion_history_max")]
    pub completion_history_max: u64,
    pub reasoning_effort: String,
    pub show_thinking: bool,
    pub thinking_default_expanded: bool,
    /// Ring the terminal bell when a request fully settles.
    pub attention_bell: bool,
    /// Capture the mouse for transcript wheel-scrolling. Off by default so
    /// the terminal keeps native selection behavior.
    pub mouse: bool,
    pub model_contexts: BTreeMap<String, u64>,
    pub context_tokens: Option<u64>,
    pub model_families: BTreeMap<String, Option<String>>,
    pub mcp_servers: Vec<Value>,
    pub backend: Option<String>,
    pub url: Option<String>,
    pub provider: Option<String>,
    pub allow_plugin_overrides: bool,
    pub docker_image: Option<String>,
    pub timeout_seconds: u64,
}

fn default_approval_ask() -> String {
    "ask".to_string()
}

fn default_approval_allow() -> String {
    "allow".to_string()
}

fn default_completion_on() -> bool {
    true
}

fn default_completion_history_max() -> u64 {
    500
}

fn default_completion_debounce_ms() -> u64 {
    250
}

impl Default for Config {
    fn default() -> Self {
        let paths = ConfigPaths::discover();
        Self {
            theme: "r105".to_string(),
            workspace: None,
            skills_dir: paths.config_dir.join("skills"),
            plugins_dir: paths.plugins_dir,
            quality: None,
            profile: None,
            model: None,
            auto_compact: true,
            cache_prompt: false,
            keybindings: BTreeMap::new(),
            sandbox_backend: "auto".to_string(),
            permission_posture: "sandboxed".to_string(),
            approval_exec: default_approval_ask(),
            approval_write: default_approval_ask(),
            approval_read: default_approval_allow(),
            approval_network: default_approval_allow(),
            approval_mcp: default_approval_ask(),
            approval_plugin: default_approval_ask(),
            command_allowlist: Vec::new(),
            command_denylist: Vec::new(),
            completion_enabled: true,
            completion_debounce_ms: default_completion_debounce_ms(),
            completion_history_max: default_completion_history_max(),
            reasoning_effort: "auto".to_string(),
            show_thinking: true,
            thinking_default_expanded: false,
            attention_bell: true,
            mouse: false,
            model_contexts: BTreeMap::new(),
            context_tokens: None,
            model_families: BTreeMap::new(),
            mcp_servers: Vec::new(),
            backend: None,
            url: None,
            provider: None,
            allow_plugin_overrides: false,
            docker_image: None,
            timeout_seconds: 120,
        }
    }
}

impl Config {
    pub fn load(paths: &ConfigPaths) -> Result<Self> {
        if !paths.config_file.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&paths.config_file)
            .with_context(|| format!("reading {}", paths.config_file.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("parsing {}", paths.config_file.display()))?;
        let Some(object) = value.as_object() else {
            anyhow::bail!("config.json must be a JSON object");
        };
        let unknown: Vec<&str> = object
            .keys()
            .filter_map(|key| (!known_keys().contains(key.as_str())).then_some(key.as_str()))
            .collect();
        if !unknown.is_empty() {
            let message = format!("unknown config keys: {}", unknown.join(", "));
            if strict_config_enabled() {
                anyhow::bail!("{message}");
            }
            eprintln!("warning: {message}; ignoring them");
        }

        let mut filtered = object.clone();
        for key in &unknown {
            filtered.remove(*key);
        }
        let config = serde_json::from_value(Value::Object(filtered))
            .with_context(|| format!("validating {}", paths.config_file.display()))?;
        validate(&config)?;
        Ok(config)
    }

    pub fn save(&self, paths: &ConfigPaths) -> Result<()> {
        fs::create_dir_all(&paths.config_dir)?;
        atomic_write_json(&paths.config_file, self)?;
        Ok(())
    }

    pub fn apply_runtime_connection(&mut self, connection: &Connection) {
        self.backend = Some(connection.backend.clone());
        self.url = Some(connection.base_url.clone());
        self.provider = connection.provider_id.clone();
        self.model = Some(connection.model.clone());
    }

    pub fn schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "r105 configuration",
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "theme": {"type": "string", "enum": ["r105", "dracula", "solarized-dark", "high-contrast"]},
                "workspace": {"type": ["string", "null"]},
                "skills_dir": {"type": "string"},
                "plugins_dir": {"type": "string"},
                "quality": {"type": ["string", "null"], "enum": ["fast", "balanced", "best", null]},
                "profile": {"type": ["string", "null"]},
                "model": {"type": ["string", "null"]},
                "auto_compact": {"type": "boolean"},
                "cache_prompt": {"type": "boolean"},
                "keybindings": {"type": "object", "additionalProperties": {"type": "string"}},
                "sandbox_backend": {"type": "string", "enum": ["auto", "nsjail", "bwrap", "docker", "rlimit", "none"]},
                "permission_posture": {"type": "string", "enum": ["full-access", "restricted", "sandboxed", "off"]},
                "approval_exec": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "approval_write": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "approval_read": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "approval_network": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "approval_mcp": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "approval_plugin": {"type": "string", "enum": ["allow", "ask", "deny"]},
                "command_allowlist": {"type": "array", "items": {"type": "string"}},
                "command_denylist": {"type": "array", "items": {"type": "string"}},
                "completion_enabled": {"type": "boolean"},
                "completion_debounce_ms": {"type": "integer", "minimum": 0, "maximum": 5000},
                "completion_history_max": {"type": "integer", "minimum": 1, "maximum": 5000},
                "reasoning_effort": {"type": "string", "enum": ["auto", "off", "low", "medium", "high"]},
                "show_thinking": {"type": "boolean"},
                "thinking_default_expanded": {"type": "boolean"},
                "attention_bell": {"type": "boolean"},
                "mouse": {"type": "boolean"},
                "model_contexts": {"type": "object", "additionalProperties": {"type": "integer", "minimum": 1}},
                "context_tokens": {"type": ["integer", "null"], "minimum": 1},
                "model_families": {"type": "object", "additionalProperties": {"type": ["string", "null"]}},
                "mcp_servers": {"type": "array"},
                "backend": {"type": ["string", "null"], "enum": ["direct", "router", null]},
                "url": {"type": ["string", "null"]},
                "provider": {"type": ["string", "null"]},
                "allow_plugin_overrides": {"type": "boolean"},
                "docker_image": {"type": ["string", "null"]},
                "timeout_seconds": {"type": "integer", "minimum": 1}
            }
        })
    }
}

fn known_keys() -> BTreeSet<&'static str> {
    [
        "theme",
        "workspace",
        "skills_dir",
        "plugins_dir",
        "quality",
        "profile",
        "model",
        "auto_compact",
        "cache_prompt",
        "keybindings",
        "sandbox_backend",
        "permission_posture",
        "approval_exec",
        "approval_write",
        "approval_read",
        "approval_network",
        "approval_mcp",
        "approval_plugin",
        "command_allowlist",
        "command_denylist",
        "completion_enabled",
        "completion_debounce_ms",
        "completion_history_max",
        "reasoning_effort",
        "show_thinking",
        "thinking_default_expanded",
        "attention_bell",
        "mouse",
        "model_contexts",
        "context_tokens",
        "model_families",
        "mcp_servers",
        "backend",
        "url",
        "provider",
        "allow_plugin_overrides",
        "docker_image",
        "timeout_seconds",
    ]
    .into_iter()
    .collect()
}

fn validate(config: &Config) -> Result<()> {
    if !THEMES.contains(&config.theme.as_str()) {
        anyhow::bail!("invalid theme '{}'", config.theme);
    }
    if !["auto", "nsjail", "bwrap", "docker", "rlimit", "none"]
        .contains(&config.sandbox_backend.as_str())
    {
        anyhow::bail!("invalid sandbox_backend '{}'", config.sandbox_backend);
    }
    if !["full-access", "restricted", "sandboxed", "off"]
        .contains(&config.permission_posture.as_str())
    {
        anyhow::bail!("invalid permission_posture '{}'", config.permission_posture);
    }
    for (key, value) in [
        ("approval_exec", &config.approval_exec),
        ("approval_write", &config.approval_write),
        ("approval_read", &config.approval_read),
        ("approval_network", &config.approval_network),
        ("approval_mcp", &config.approval_mcp),
        ("approval_plugin", &config.approval_plugin),
    ] {
        if !["allow", "ask", "deny"].contains(&value.as_str()) {
            anyhow::bail!("invalid {key} '{value}'");
        }
    }
    if !["auto", "off", "low", "medium", "high"].contains(&config.reasoning_effort.as_str()) {
        anyhow::bail!("invalid reasoning_effort '{}'", config.reasoning_effort);
    }
    if let Some(backend) = &config.backend
        && !["direct", "router"].contains(&backend.as_str())
    {
        anyhow::bail!("invalid backend '{}'", backend);
    }
    if let Some(tokens) = config.context_tokens
        && tokens == 0
    {
        anyhow::bail!("context_tokens must be positive");
    }
    if config.timeout_seconds == 0 {
        anyhow::bail!("timeout_seconds must be positive");
    }
    Ok(())
}

fn strict_config_enabled() -> bool {
    matches!(
        env::var("R105_STRICT_CONFIG")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("r105"),
        stamp
    ));
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    {
        let mut file = File::create(&temp_path)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&temp_path, path)?;
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_rejects_unknown_properties() {
        assert_eq!(Config::schema()["additionalProperties"], false);
    }

    #[test]
    fn default_config_has_local_first_defaults() {
        let config = Config::default();
        assert_eq!(config.sandbox_backend, "auto");
        assert!(config.auto_compact);
        assert_eq!(config.timeout_seconds, 120);
        assert!(config.attention_bell);
        assert!(!config.mouse);
    }
}
