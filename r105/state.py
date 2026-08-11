"""Chat state and related data classes."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from r105.skills import skill_messages as _skill_messages  # canonical location

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
VALID_REASONING_EFFORTS = {"auto", "off", "low", "medium", "high"}
VALID_PERMISSION_POSTURES = {"full-access", "restricted", "sandboxed", "off"}


@dataclass
class ChatState:
    profile: str | None = None
    rag: bool | None = None
    quality: str | None = None
    max_tokens: int | None = None
    json_mode: bool = False
    auto_compact: bool = True
    theme: str = "r105"
    model: str = DEFAULT_MODEL
    reasoning_effort: str = "auto"
    show_thinking: bool = True
    thinking_default_expanded: bool = False
    permission_posture: str = "sandboxed"
    skills_dir: Path = field(default_factory=lambda: Path("skills"))
    active_skills: list[str] = field(default_factory=list)
    skill_params: dict[str, dict[str, str]] = field(default_factory=dict)
    model_contexts: dict[str, int] = field(default_factory=dict)
    model_families: dict[str, str | None] = field(default_factory=dict)
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
    """Estimate token count, using tiktoken if available, heuristic otherwise."""
    return _tiktoken_count(text) if _tiktoken_available(text) else _heuristic_token_count(text)


def _tiktoken_available(text: str) -> bool:
    """Check if tiktoken can be used for this text."""
    try:
        return bool(__import__("tiktoken", fromlist=[""]))
    except ImportError:
        return False


def _tiktoken_count(text: str) -> int:
    """Count tokens using tiktoken with a fallback to heuristic."""
    try:
        import tiktoken
        try:
            enc = tiktoken.get_encoding("cl100k_base")
        except Exception:
            enc = tiktoken.get_encoding("gpt2")
        return len(enc.encode(text, disallowed_special=()))
    except Exception:
        return _heuristic_token_count(text)


def _heuristic_token_count(text: str) -> int:
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
