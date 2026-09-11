"""Chat state and related data classes."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from r105.skills import skill_messages

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
    quality: str | None = None
    max_tokens: int | None = None
    json_mode: bool = False
    auto_compact: bool = True
    # llama.cpp accepts this optional request flag to reuse the stable prompt
    # prefix. It is opt-in because other OpenAI-compatible providers may
    # reject unknown request fields.
    cache_prompt: bool = False
    keybindings: dict[str, str] = field(default_factory=dict)
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
    # Exact provider usage is populated after a response when the backend
    # returns standard usage metadata. The history length/model guard keeps a
    # stale response from being shown after a local state change.
    last_backend_total_tokens: int | None = field(default=None, repr=False)
    last_backend_history_length: int | None = field(default=None, repr=False)
    last_backend_model: str | None = field(default=None, repr=False)


@dataclass(frozen=True)
class ChatResult:
    content: str
    wall_seconds: float
    prompt_tps: float | None
    generation_tps: float | None
    raw: dict[str, Any]
    tool_calls: list[dict[str, Any]] = field(default_factory=list)
    prompt_tokens: int | None = None
    completion_tokens: int | None = None
    total_tokens: int | None = None


@dataclass(frozen=True)
class TokenEstimate:
    """A local token estimate and how much confidence to place in it."""

    tokens: int
    source: str
    confidence: float

    @property
    def confidence_label(self) -> str:
        if self.confidence >= 0.9:
            return "high"
        if self.confidence >= 0.6:
            return "medium"
        return "low"


@dataclass(frozen=True)
class TokenUsage:
    used_tokens: int
    context_tokens: int
    estimate_source: str = "heuristic"
    confidence: float = 0.0

    @property
    def percent(self) -> float:
        if self.context_tokens <= 0:
            return 0.0
        return min(100.0, (self.used_tokens / self.context_tokens) * 100)

    @property
    def confidence_label(self) -> str:
        if self.confidence >= 0.9:
            return "high"
        if self.confidence >= 0.6:
            return "medium"
        return "low"

    @property
    def estimate_label(self) -> str:
        return f"{self.estimate_source}/{self.confidence_label}"


def token_usage(state: ChatState) -> TokenUsage:
    texts: list[str] = []
    texts.extend(message.get("content", "") for message in skill_messages(state))
    texts.extend(str(message.get("content", "")) for message in state.history)
    if (
        state.last_backend_total_tokens is not None
        and state.last_backend_history_length == len(state.history)
        and state.last_backend_model == state.model
    ):
        return TokenUsage(
            used_tokens=state.last_backend_total_tokens,
            context_tokens=state.context_tokens,
            estimate_source="backend",
            confidence=1.0,
        )

    estimates = [estimate_token_info(text, model=state.model) for text in texts if text]
    if not estimates:
        return TokenUsage(
            used_tokens=0,
            context_tokens=state.context_tokens,
            estimate_source="none",
            confidence=1.0,
        )
    total = sum(item.tokens for item in estimates)
    weights = sum(max(1, item.tokens) for item in estimates)
    confidence = sum(item.confidence * max(1, item.tokens) for item in estimates) / weights
    sources = {item.source for item in estimates}
    source = next(iter(sources)) if len(sources) == 1 else "mixed"
    return TokenUsage(
        used_tokens=total,
        context_tokens=state.context_tokens,
        estimate_source=source,
        confidence=confidence,
    )


def invalidate_backend_usage(state: ChatState) -> None:
    """Discard response usage after local history/model changes."""
    state.last_backend_total_tokens = None
    state.last_backend_history_length = None
    state.last_backend_model = None


def estimate_tokens(text: str, model: str | None = None) -> int:
    """Estimate token count, using tiktoken if available, heuristic otherwise.

    ``tiktoken`` (``pip install 'r105[dev]'`` or ``pip install tiktoken``) is
    strongly recommended for accurate counts; the heuristic fallback is
    calibrated for modern BPE/SentencePiece tokenizers (Llama 3, Gemma 2/3,
    Qwen, Mistral) and accepts an optional *model* hint for family-specific
    tuning.
    """
    return estimate_token_info(text, model=model).tokens


def estimate_token_info(text: str, model: str | None = None) -> TokenEstimate:
    """Return a token estimate plus its source and confidence score.

    Backend usage metadata supersedes this local estimate after a response.
    A model-specific tiktoken encoding is high confidence; generic tiktoken
    and heuristic estimates are marked lower because they may not match a
    llama.cpp or SentencePiece tokenizer exactly.
    """
    if not text:
        return TokenEstimate(0, "none", 1.0)
    if _tiktoken_available():
        count, encoding_source = _tiktoken_count_with_source(text, model=model)
        if count is not None:
            if encoding_source == "model":
                return TokenEstimate(count, "tiktoken", 0.99)
            if model and _sentencepiece_model_hint(model):
                return TokenEstimate(count, "tiktoken-approx", 0.55)
            return TokenEstimate(count, "tiktoken", 0.75)

    heuristic = _heuristic_token_count(text, model=model)
    has_non_ascii = any(ord(char) > 127 for char in text)
    confidence = 0.25 if has_non_ascii else 0.35
    return TokenEstimate(heuristic, "heuristic", confidence)


def _sentencepiece_model_hint(model: str) -> bool:
    lowered = model.lower()
    return any(
        family in lowered
        for family in ("gemma", "llama", "mistral", "mixtral", "phi", "qwen")
    )


def _tiktoken_available(*_args: Any) -> bool:
    """Check if tiktoken can be used."""
    try:
        __import__("tiktoken")
        return True
    except ImportError:
        return False


# Backwards-compat: old helper took ``text``; keep accepting it.
def _tiktoken_available_legacy(text: str) -> bool:  # pragma: no cover
    del text
    return _tiktoken_available()


def _tiktoken_count(text: str, model: str | None = None) -> int:
    """Count tokens using tiktoken with a fallback to heuristic."""
    if not text:
        return 0
    count, _source = _tiktoken_count_with_source(text, model=model)
    return count if count is not None else _heuristic_token_count(text, model=model)


def _tiktoken_count_with_source(
    text: str, model: str | None = None
) -> tuple[int | None, str | None]:
    """Return a tiktoken count and whether it used a model-specific encoding."""
    if not text:
        return 0, "empty"
    try:
        import tiktoken

        enc = None
        encoding_source: str | None = None
        # Prefer a model-specific encoding when a hint is available.
        if model:
            try:
                enc = tiktoken.encoding_for_model(model)
                encoding_source = "model"
            except Exception:
                enc = None
        if enc is None:
            # cl100k_base covers GPT-4/3.5 + most modern BPE models; o200k_base
            # is newer (GPT-4o). Try o200k first, fall back to cl100k, then gpt2.
            for name in ("o200k_base", "cl100k_base", "gpt2"):
                try:
                    enc = tiktoken.get_encoding(name)
                    encoding_source = "generic"
                    break
                except Exception:
                    continue
        if enc is None:
            return None, None
        return len(enc.encode(text, disallowed_special=())), encoding_source
    except Exception:
        return None, None


def _heuristic_token_count(text: str, model: str | None = None) -> int:
    """Estimate token count using a heuristic calibrated for modern tokenizers.

    Covers BPE (GPT, Qwen) and SentencePiece/Unigram (Llama 3, Gemma 2/3,
    Mistral, Phi) where:
    - Common English averages ~1.3 tokens/word (BPE) to ~1.5 (SentencePiece
      with byte-fallback for CJK/emoji).
    - Code with indentation/operators is 2-4x denser.
    - CJK characters are ~1 token/char; emoji/ZWJ sequences are 2-7 tokens.
    - Whitespace-heavy formatting creates extra tokens.

    *model* optionally tunes the multiplier (e.g. Gemma/Llama SentencePiece
    models skew higher on non-ASCII). Without tiktoken this is approximate —
    install tiktoken for exact counts.
    """
    if not text:
        return 0
    # CJK Unified Ideographs, Hiragana/Katakana, Hangul: ~1 token per char.
    cjk_chars = len(re.findall(r"[\u4e00-\u9fff\u3040-\u30ff\uac00-\ud7af\u3400-\u4dbf]", text))
    # Emoji / pictographs / ZWJ sequences: expensive (2-7 tokens each).
    emoji_chars = len(re.findall(r"[\U0001F000-\U0001FAFF\u2600-\u27BF\uFE0F\u200D]", text))
    # Word-like tokens + individual punctuation/operators.
    pieces = re.findall(r"\w+|[^\w\s]", text, flags=re.UNICODE)
    # Exclude already-counted CJK/emoji from the generic piece count to avoid
    # double counting: approximate by subtracting their char counts.
    base = max(0, len(pieces) - cjk_chars - emoji_chars)
    indent_lines = len(re.findall(r"^\s{2,}", text, flags=re.MULTILINE))
    # Family-tuned multiplier: SentencePiece models (Llama/Gemma/Mistral)
    # fragment words more aggressively than BPE.
    multiplier = 1.3
    if model:
        lowered = model.lower()
        if any(k in lowered for k in ("gemma", "llama", "mistral", "mixtral", "phi", "qwen")):
            multiplier = 1.45
    estimated = int(base * multiplier) + indent_lines + cjk_chars + emoji_chars * 3
    return max(1, estimated)


# Backwards-compat alias: old private name used with underscore prefix.
_skill_messages = skill_messages
