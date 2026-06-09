"""Chat state and related data classes."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

DEFAULT_MODEL = "gemma-4-12b-it"
DEFAULT_CONTEXT_TOKENS = 262144
VALID_PROFILES = {
    "simple",
    "strict_json",
    "coding",
    "complex_reasoning",
    "long_context_qa",
    "tool_agent",
    "creative",
}
VALID_QUALITIES = {"fast", "balanced", "best"}


@dataclass
class ChatState:
    profile: str | None = None
    rag: bool | None = None
    quality: str | None = None
    max_tokens: int | None = None
    json_mode: bool = False
    skills_dir: Path = field(default_factory=lambda: Path("skills"))
    active_skills: list[str] = field(default_factory=list)
    context_tokens: int = DEFAULT_CONTEXT_TOKENS
    history: list[dict[str, str]] = field(default_factory=list)


@dataclass(frozen=True)
class ChatResult:
    content: str
    wall_seconds: float
    prompt_tps: float | None
    generation_tps: float | None
    raw: dict[str, Any]
    tool_calls: list[dict[str, Any]] = field(default_factory=list)


@dataclass(frozen=True)
class TokenUsage:
    used_tokens: int
    context_tokens: int

    @property
    def percent(self) -> float:
        if self.context_tokens <= 0:
            return 0.0
        return min(100.0, (self.used_tokens / self.context_tokens) * 100)


def token_usage(state: ChatState) -> TokenUsage:
    texts: list[str] = []
    texts.extend(message.get("content", "") for message in _skill_messages(state))
    texts.extend(str(message.get("content", "")) for message in state.history)
    return TokenUsage(
        used_tokens=sum(estimate_tokens(text) for text in texts),
        context_tokens=state.context_tokens,
    )


def estimate_tokens(text: str) -> int:
    if not text:
        return 0
    pieces = re.findall(r"\w+|[^\w\s]", text, flags=re.UNICODE)
    return max(1, len(pieces))


def _skill_messages(state: ChatState) -> list[dict[str, str]]:
    messages: list[dict[str, str]] = []
    for name in state.active_skills:
        text = _read_skill(state.skills_dir, name)
        if text:
            messages.append({"role": "system", "content": f"Active skill: {name}\n{text}"})
    return messages


def _read_skill(skills_dir: Path, name: str) -> str:
    if "/" in name or "\\" in name or name.startswith("."):
        return ""
    path = skills_dir / f"{name}.md"
    if not path.is_file():
        return ""
    return path.read_text(encoding="utf-8").strip()
