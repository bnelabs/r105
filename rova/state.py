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
    auto_compact: bool = True
    theme: str = "rova"
    skills_dir: Path = field(default_factory=lambda: Path("skills"))
    active_skills: list[str] = field(default_factory=list)
    skill_params: dict[str, dict[str, str]] = field(default_factory=dict)
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
    """Estimate token count using a heuristic calibrated for BPE tokenizers.

    Modern LLMs (Gemma, Llama, etc.) use sub-word tokenizers where:
    - Common English words average ~1.3 tokens each
    - Code with indentation, operators, and punctuation can be 2-4x denser
    - Whitespace-heavy formatting (tabs, repeated spaces) creates extra tokens

    This estimator applies a 1.3x multiplier to the word/punctuation count
    and adds a separate allowance for whitespace-heavy code blocks.
    """
    if not text:
        return 0
    # Count word-like tokens and individual punctuation/operators
    pieces = re.findall(r"\w+|[^\w\s]", text, flags=re.UNICODE)
    base = len(pieces)
    # Count leading whitespace (indentation) — each indent level costs tokens
    indent_lines = len(re.findall(r"^\s{2,}", text, flags=re.MULTILINE))
    # Apply BPE overhead multiplier (1.3x) plus indentation penalty
    estimated = int(base * 1.3) + indent_lines
    return max(1, estimated)


def _skill_messages(state: ChatState) -> list[dict[str, str]]:
    messages: list[dict[str, str]] = []
    for name in state.active_skills:
        params = state.skill_params.get(name)
        text = _read_skill(state.skills_dir, name)
        if text and params:
            for key, value in params.items():
                text = text.replace(f"{{{key}}}", value)
        if text:
            messages.append({"role": "system", "content": f"Active skill: {name}\n{text}"})
    return messages


def _read_skill(skills_dir: Path, name: str) -> str:
    if "/" in name or "\\" in name or name.startswith("."):
        return ""
    path = skills_dir / f"{name}.md"
    if not path.is_file():
        return ""
    return path.read_text(encoding="utf-8", errors="replace").strip()
