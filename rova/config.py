"""Configuration file management for Rova.

Reads settings from ~/.config/rova/config.json on startup.
CLI arguments override config file values.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

CONFIG_DIR = Path.home() / ".config" / "rova"
CONFIG_PATH = CONFIG_DIR / "config.json"

DEFAULT_CONFIG: dict[str, Any] = {
    "theme": "rova",
    "workspace": str(Path.home() / "rova-workspace"),
    "skills_dir": str(CONFIG_DIR / "skills"),
    "plugins_dir": str(CONFIG_DIR / "plugins"),
    "quality": None,
    "profile": None,
    "auto_compact": True,
    "sandbox_backend": "auto",
    "mcp_servers": [],
    "url": "http://127.0.0.1:8010",
}


def ensure_config() -> dict[str, Any]:
    """Read the config file, creating a default one if it doesn't exist.

    Returns the merged config (defaults + file overrides).
    """
    config = dict(DEFAULT_CONFIG)
    if CONFIG_PATH.is_file():
        try:
            raw = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
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
    if "auto_compact" in config:
        overrides["auto_compact"] = config["auto_compact"]
    return overrides
