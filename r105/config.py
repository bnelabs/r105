"""Configuration file management for r105.

Reads settings from ~/.config/r105/config.json on startup.
CLI arguments override config file values.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

CONFIG_DIR = Path.home() / ".config" / "r105"
CONFIG_PATH = CONFIG_DIR / "config.json"

DEFAULT_CONFIG: dict[str, Any] = {
    "theme": "r105",
    "workspace": str(Path.home() / "r105-workspace"),
    "skills_dir": str(CONFIG_DIR / "skills"),
    "plugins_dir": str(CONFIG_DIR / "plugins"),
    "quality": None,
    "profile": None,
    "model": None,
    "auto_compact": True,
    "sandbox_backend": "auto",
    "permission_posture": "sandboxed",
    "reasoning_effort": "auto",
    "show_thinking": True,
    "thinking_default_expanded": False,
    "model_contexts": {},
    "context_tokens": None,
    "mcp_servers": [],
    "url": None,
}

VALID_THEMES = {"r105", "dracula", "solarized-dark", "high-contrast"}
VALID_SANDBOX_BACKENDS = {"auto", "nsjail", "bwrap", "rlimit", "none"}
VALID_PERMISSION_POSTURES = {"full-access", "restricted", "sandboxed", "off"}
VALID_REASONING_EFFORTS = {"auto", "off", "low", "medium", "high"}


def _validate_config(raw: dict[str, Any]) -> None:
    """Validate config keys and values. Raises ValueError with helpful message on failure."""
    if not isinstance(raw, dict):
        raise ValueError("config.json must be a JSON object")

    # Validate known keys
    for key in raw:
        if key not in DEFAULT_CONFIG:
            raise ValueError(f"Unknown config key: '{key}'. Valid keys are: {', '.join(sorted(DEFAULT_CONFIG.keys()))}")

    # Validate specific fields
    if "theme" in raw and raw["theme"] is not None and raw["theme"] not in VALID_THEMES:
        raise ValueError(f"Invalid theme '{raw['theme']}'. Valid themes: {', '.join(sorted(VALID_THEMES))}")

    if (
        "sandbox_backend" in raw
        and raw["sandbox_backend"] is not None
        and raw["sandbox_backend"] not in VALID_SANDBOX_BACKENDS
    ):
        raise ValueError(
            f"Invalid sandbox_backend '{raw['sandbox_backend']}'. Valid: {', '.join(sorted(VALID_SANDBOX_BACKENDS))}"
        )

    if "auto_compact" in raw and not isinstance(raw["auto_compact"], bool):
        raise ValueError("auto_compact must be true or false")

    if (
        "permission_posture" in raw
        and raw["permission_posture"] is not None
        and raw["permission_posture"] not in VALID_PERMISSION_POSTURES
    ):
        raise ValueError(
            f"Invalid permission_posture '{raw['permission_posture']}'. "
            f"Valid: {', '.join(sorted(VALID_PERMISSION_POSTURES))}"
        )

    if (
        "reasoning_effort" in raw
        and raw["reasoning_effort"] is not None
        and raw["reasoning_effort"] not in VALID_REASONING_EFFORTS
    ):
        raise ValueError(
            f"Invalid reasoning_effort '{raw['reasoning_effort']}'. "
            f"Valid: {', '.join(sorted(VALID_REASONING_EFFORTS))}"
        )

    if "show_thinking" in raw and not isinstance(raw["show_thinking"], bool):
        raise ValueError("show_thinking must be true or false")

    if "thinking_default_expanded" in raw and not isinstance(raw["thinking_default_expanded"], bool):
        raise ValueError("thinking_default_expanded must be true or false")

    if "model_contexts" in raw:
        if not isinstance(raw["model_contexts"], dict):
            raise ValueError("model_contexts must be an object mapping name fragments to token counts")
        for frag, value in raw["model_contexts"].items():
            try:
                ivalue = int(value)
            except (TypeError, ValueError):
                raise ValueError(
                    f"model_contexts['{frag}'] must be a positive integer, got {value!r}"
                ) from None
            if ivalue <= 0:
                raise ValueError(f"model_contexts['{frag}'] must be a positive integer, got {value!r}")

    if "context_tokens" in raw and raw["context_tokens"] is not None:
        try:
            ivalue = int(raw["context_tokens"])
        except (TypeError, ValueError):
            raise ValueError(f"context_tokens must be a positive integer, got {raw['context_tokens']!r}") from None
        if ivalue <= 0:
            raise ValueError(f"context_tokens must be a positive integer, got {raw['context_tokens']!r}")

    if "mcp_servers" in raw:
        if not isinstance(raw["mcp_servers"], list):
            raise ValueError("mcp_servers must be a list")
        for i, srv in enumerate(raw["mcp_servers"]):
            if not isinstance(srv, dict):
                raise ValueError(f"mcp_servers[{i}] must be an object")
            if "name" not in srv:
                raise ValueError(f"mcp_servers[{i}] missing required field 'name'")


def ensure_config() -> dict[str, Any]:
    """Read the config file, creating a default one if it doesn't exist.

    Returns the merged config (defaults + file overrides).
    """
    config = dict(DEFAULT_CONFIG)
    if CONFIG_PATH.is_file():
        try:
            raw = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
            try:
                _validate_config(raw)
            except ValueError:
                # Invalid config file - ignore it and fall back to defaults
                # Validation errors will be caught on save
                return config
            if isinstance(raw, dict):
                config.update(raw)
        except (json.JSONDecodeError, OSError):
            pass
    return config


def save_config(overrides: dict[str, Any]) -> None:
    """Merge overrides into the config file and write it back.

    Creates the config directory and file if they don't exist.
    """
    config = ensure_config()
    config.update(overrides)
    # Validate before writing
    _validate_config(config)
    # Remove keys that match defaults (keep config file lean)
    for k, v in DEFAULT_CONFIG.items():
        if k in config and config[k] == v:
            del config[k]
    CONFIG_DIR.mkdir(parents=True, exist_ok=True)
    CONFIG_PATH.write_text(json.dumps(config, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def load_state_overrides() -> dict[str, Any]:
    """Return only the keys from config that map to ChatState fields."""
    config = ensure_config()
    overrides: dict[str, Any] = {}
    if config.get("theme") and config["theme"] != DEFAULT_CONFIG["theme"]:
        overrides["theme"] = config["theme"]
    if config.get("quality"):
        overrides["quality"] = config["quality"]
    if config.get("profile"):
        overrides["profile"] = config["profile"]
    if config.get("model"):
        overrides["model"] = config["model"]
    if "auto_compact" in config:
        overrides["auto_compact"] = config["auto_compact"]
    if config.get("reasoning_effort") is not None:
        overrides["reasoning_effort"] = config["reasoning_effort"]
    if config.get("permission_posture") is not None:
        overrides["permission_posture"] = config["permission_posture"]
    if "show_thinking" in config:
        overrides["show_thinking"] = config["show_thinking"]
    if "thinking_default_expanded" in config:
        overrides["thinking_default_expanded"] = config["thinking_default_expanded"]
    if config.get("model_contexts"):
        overrides["model_contexts"] = config["model_contexts"]
    if config.get("context_tokens") is not None:
        overrides["context_tokens"] = config["context_tokens"]
    return overrides
