"""Provider presets and credential resolution for the guided connection flow.

The catalog contains only connection metadata. API keys are resolved from the
current process environment or from the connection dialog and are never
written to ``config.json``.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Literal
from urllib.parse import urlsplit

BackendName = Literal["direct", "router"]


@dataclass(frozen=True)
class ConnectionPreset:
    """A named OpenAI-compatible connection target."""

    id: str
    label: str
    backend: BackendName
    base_url: str | None
    description: str
    api_key_env: str | None = None
    api_key_required: bool = False
    docs_url: str | None = None

    @property
    def needs_api_key_field(self) -> bool:
        """Whether the guided flow should offer an API-key field."""
        return self.api_key_env is not None or self.api_key_required or self.id == "custom"


# Keep the order intentional: the two OpenCode connections are the first cloud
# choices, followed by common local servers and then other hosted APIs.
CONNECTION_PRESETS: tuple[ConnectionPreset, ...] = (
    ConnectionPreset(
        "opencode",
        "OpenCode Zen",
        "direct",
        "https://opencode.ai/zen/v1",
        "OpenCode's curated cloud models",
        "OPENCODE_API_KEY",
        True,
        "https://opencode.ai/auth",
    ),
    ConnectionPreset(
        "opencode-go",
        "OpenCode Go",
        "direct",
        "https://opencode.ai/zen/go/v1",
        "OpenCode's lower-cost cloud models",
        "OPENCODE_API_KEY",
        True,
        "https://opencode.ai/auth",
    ),
    ConnectionPreset(
        "router",
        "llama-router",
        "router",
        "http://127.0.0.1:8010",
        "Local router with profiles and routing metadata",
    ),
    ConnectionPreset(
        "llamacpp",
        "llama.cpp",
        "direct",
        "http://127.0.0.1:8080/v1",
        "Local llama-server OpenAI-compatible endpoint",
    ),
    ConnectionPreset(
        "ollama",
        "Ollama",
        "direct",
        "http://127.0.0.1:11434/v1",
        "Local Ollama OpenAI-compatible endpoint",
    ),
    ConnectionPreset(
        "lmstudio",
        "LM Studio",
        "direct",
        "http://127.0.0.1:1234/v1",
        "Local LM Studio OpenAI-compatible endpoint",
    ),
    ConnectionPreset(
        "vllm",
        "vLLM",
        "direct",
        "http://127.0.0.1:8000/v1",
        "Local or hosted vLLM endpoint",
        "OPENAI_API_KEY",
        False,
    ),
    ConnectionPreset(
        "openai",
        "OpenAI",
        "direct",
        "https://api.openai.com/v1",
        "OpenAI API",
        "OPENAI_API_KEY",
        True,
    ),
    ConnectionPreset(
        "groq",
        "Groq",
        "direct",
        "https://api.groq.com/openai/v1",
        "Groq hosted inference",
        "GROQ_API_KEY",
        True,
    ),
    ConnectionPreset(
        "openrouter",
        "OpenRouter",
        "direct",
        "https://openrouter.ai/api/v1",
        "OpenRouter model gateway",
        "OPENROUTER_API_KEY",
        True,
    ),
    ConnectionPreset(
        "deepseek",
        "DeepSeek",
        "direct",
        "https://api.deepseek.com/v1",
        "DeepSeek API",
        "DEEPSEEK_API_KEY",
        True,
    ),
    ConnectionPreset(
        "together",
        "Together AI",
        "direct",
        "https://api.together.xyz/v1",
        "Together hosted inference",
        "TOGETHER_API_KEY",
        True,
    ),
    ConnectionPreset(
        "custom",
        "Custom OpenAI-compatible API",
        "direct",
        None,
        "Enter any OpenAI-compatible base URL",
        None,
        False,
    ),
)


CONNECTION_ALIASES: dict[str, str] = {
    "local": "ollama",
    "llama-router": "router",
    "lm-studio": "lmstudio",
    "llama.cpp": "llamacpp",
    "llama-cpp": "llamacpp",
    "opencode-zen": "opencode",
    "zen": "opencode",
    "opencodego": "opencode-go",
}


def get_connection_preset(provider_id: str | None) -> ConnectionPreset | None:
    """Return a preset by id or alias."""
    if not provider_id:
        return None
    normalized = CONNECTION_ALIASES.get(provider_id.strip().lower(), provider_id.strip().lower())
    return next((preset for preset in CONNECTION_PRESETS if preset.id == normalized), None)


def resolve_api_key(
    preset: ConnectionPreset,
    entered: str | None = None,
    *,
    fallback: str | None = None,
) -> str | None:
    """Resolve a session API key without persisting it.

    An empty string is returned for a named credential environment variable
    that is not set. That deliberate sentinel prevents a provider-specific
    connection from accidentally borrowing an unrelated ``OPENAI_API_KEY``.
    ``None`` means the provider has no configured credential environment.
    """
    if entered is not None and entered.strip():
        return entered.strip()
    if fallback:
        return fallback
    if preset.api_key_env is not None:
        return os.environ.get(preset.api_key_env, "")
    return None


def valid_connection_url(value: str) -> bool:
    """Accept HTTP(S) API URLs without credentials or whitespace."""
    parsed = urlsplit(value)
    return (
        parsed.scheme in {"http", "https"}
        and bool(parsed.hostname)
        and parsed.username is None
        and parsed.password is None
        and not any(char.isspace() for char in value)
    )


def provider_options() -> list[tuple[str, str]]:
    """Return labels and ids suitable for a Textual ``Select`` widget."""
    return [
        (f"{preset.label} — {preset.description}", preset.id)
        for preset in CONNECTION_PRESETS
    ]


__all__ = [
    "CONNECTION_ALIASES",
    "CONNECTION_PRESETS",
    "ConnectionPreset",
    "get_connection_preset",
    "provider_options",
    "resolve_api_key",
    "valid_connection_url",
]
