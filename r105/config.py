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
    "model_families": {},
    "mcp_servers": [],
    "url": None,
    "allow_plugin_overrides": False,
    "docker_image": None,
}

VALID_THEMES = {"r105", "dracula", "solarized-dark", "high-contrast"}
VALID_SANDBOX_BACKENDS = {"auto", "nsjail", "bwrap", "docker", "rlimit", "none"}
VALID_PERMISSION_POSTURES = {"full-access", "restricted", "sandboxed", "off"}
VALID_REASONING_EFFORTS = {"auto", "off", "low", "medium", "high"}


# -- Declarative schema via pydantic (preferred) with manual fallback --------
# ``pydantic`` provides declarative validation; when it is not installed we
# fall back to the manual ``_validate_config`` checks below so ``r105`` runs
# with only stdlib + httpx/textual/rich.
_PYDANTIC_AVAILABLE = False
_PydanticBaseModel: Any = object
_PydanticValidationError: Any = Exception
_field_validator: Any = None
try:  # pragma: no cover - import-time feature detection
    from pydantic import BaseModel as _BaseModel
    from pydantic import ValidationError as _ValidationError
    from pydantic import field_validator as _field_validator_fn
    _PydanticBaseModel = _BaseModel
    _PydanticValidationError = _ValidationError
    _field_validator = _field_validator_fn
    _PYDANTIC_AVAILABLE = True
except ImportError:  # pragma: no cover
    pass


if _PYDANTIC_AVAILABLE:
    class R105Config(_PydanticBaseModel):
        """Declarative config schema (pydantic v2)."""
        theme: str | None = "r105"
        workspace: str | None = None
        skills_dir: str | None = None
        plugins_dir: str | None = None
        quality: str | None = None
        profile: str | None = None
        model: str | None = None
        auto_compact: bool = True
        sandbox_backend: str | None = "auto"
        permission_posture: str | None = "sandboxed"
        reasoning_effort: str | None = "auto"
        show_thinking: bool = True
        thinking_default_expanded: bool = False
        model_contexts: dict[str, int] = {}
        context_tokens: int | None = None
        model_families: dict[str, str | None] = {}
        mcp_servers: list[dict[str, Any]] = []
        url: str | None = None
        allow_plugin_overrides: bool = False
        docker_image: str | None = None

        model_config = {"extra": "forbid"}

        @_field_validator("theme")
        @classmethod
        def _check_theme(cls, v: str | None) -> str | None:
            if v is not None and v not in VALID_THEMES:
                raise ValueError(f"Invalid theme '{v}'. Valid: {sorted(VALID_THEMES)}")
            return v

        @_field_validator("sandbox_backend")
        @classmethod
        def _check_sandbox(cls, v: str | None) -> str | None:
            if v is not None and v not in VALID_SANDBOX_BACKENDS:
                raise ValueError(f"Invalid sandbox_backend '{v}'. Valid: {sorted(VALID_SANDBOX_BACKENDS)}")
            return v

        @_field_validator("permission_posture")
        @classmethod
        def _check_posture(cls, v: str | None) -> str | None:
            if v is not None and v not in VALID_PERMISSION_POSTURES:
                raise ValueError(f"Invalid permission_posture '{v}'. Valid: {sorted(VALID_PERMISSION_POSTURES)}")
            return v

        @_field_validator("reasoning_effort")
        @classmethod
        def _check_effort(cls, v: str | None) -> str | None:
            if v is not None and v not in VALID_REASONING_EFFORTS:
                raise ValueError(f"Invalid reasoning_effort '{v}'. Valid: {sorted(VALID_REASONING_EFFORTS)}")
            return v

        @_field_validator("context_tokens")
        @classmethod
        def _check_ctx(cls, v: int | None) -> int | None:
            if v is not None and (not isinstance(v, int) or v <= 0):
                raise ValueError(f"context_tokens must be a positive integer, got {v!r}")
            return v
else:
    R105Config = None  # type: ignore[assignment,misc]


def _validate_config(raw: dict[str, Any]) -> None:
    """Validate config keys and values. Raises ValueError with helpful message on failure.

    Manual checks run first to preserve stable error messages (tests depend
    on them); when pydantic is installed the declarative ``R105Config``
    schema is also enforced as the single source of truth for new keys.
    """
    if not isinstance(raw, dict):
        raise ValueError("config.json must be a JSON object")

    # Manual validation first (stable messages, no optional deps required).
    _validate_config_manual(raw)

    if _PYDANTIC_AVAILABLE and R105Config is not None:
        try:
            R105Config(**raw)
        except _PydanticValidationError as exc:
            raise ValueError(str(exc)) from exc


def _validate_config_manual(raw: dict[str, Any]) -> None:
    """Manual validation fallback (also runs when pydantic is present)."""
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

    if "model_families" in raw:
        if not isinstance(raw["model_families"], dict):
            raise ValueError(
                "model_families must be an object mapping name fragments to family names (or null)"
            )
        for frag, family in raw["model_families"].items():
            if not isinstance(frag, str) or not frag.strip():
                raise ValueError(
                    f"model_families keys must be non-empty strings, got {frag!r}"
                )
            if family is not None and not isinstance(family, str):
                raise ValueError(
                    f"model_families['{frag}'] must be a family name string or null, got {family!r}"
                )

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

    if "allow_plugin_overrides" in raw and not isinstance(raw["allow_plugin_overrides"], bool):
        raise ValueError("allow_plugin_overrides must be true or false")

    if "docker_image" in raw and raw["docker_image"] is not None and not isinstance(raw["docker_image"], str):
        raise ValueError("docker_image must be a string or null")


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
    if config.get("model_families"):
        overrides["model_families"] = config["model_families"]
    if config.get("context_tokens") is not None:
        overrides["context_tokens"] = config["context_tokens"]
    return overrides
