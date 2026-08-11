"""Model capability catalog: resolve context-window limits for arbitrary models.

r105 is backend-agnostic — the active model may be served by llama.cpp,
Ollama, vLLM, llama-router, or any OpenAI-compatible API. This module
resolves the model's context-window capacity from, in priority order:

1. An explicit user override in config.json — either per-model via
   ``"model_contexts": {"<name-fragment>": <tokens>}`` or globally via
   ``"context_tokens": <tokens>``.
2. A live backend probe — llama.cpp ``/props`` (``default_generation_settings
   .n_ctx``) or OpenAI-style ``/v1/models`` entry metadata (``meta.n_ctx``,
   ``context_window``, ``context_length``, ``max_model_len``).
3. The built-in catalog below (substring-matched against the model name).
4. ``DEFAULT_CONTEXT_TOKENS`` as the final fallback.

The built-in catalog is intentionally conservative and editable: add entries
for model families you use, or override them in config.json.
"""

from __future__ import annotations

import re
from typing import Any

from r105.state import DEFAULT_CONTEXT_TOKENS

# ---------------------------------------------------------------------------
# Built-in catalog: (name-fragment, context-tokens).
# Longest matching fragment wins. Fragments are compared case-insensitively
# against the normalized model name (quant suffixes stripped).
# ---------------------------------------------------------------------------
BUILTIN_MODEL_CONTEXTS: list[tuple[str, int]] = [
    # Local llama.cpp workloads
    ("muse-glimmer", 131072),
    ("gemma-4-31b", 131072),
    ("gemma-4-12b", 131072),
    ("gemma-4", 131072),
    ("gemma4", 131072),
    ("gemma3", 131072),
    ("gemma2", 8192),
    ("gemma", 8192),
    ("qwen3.6", 131072),
    ("qwen3", 131072),
    ("qwen2.5", 131072),
    ("qwen2", 32768),
    ("qwen", 32768),
    ("nanbeige", 65536),
    # Common hosted/API families
    ("deepseek-v3.2", 163840),
    ("deepseek-v3.1", 131072),
    ("deepseek-v3", 131072),
    ("deepseek-r1", 131072),
    ("deepseek", 65536),
    ("llama-3.3", 131072),
    ("llama-3.1", 131072),
    ("llama-3", 8192),
    ("llama", 8192),
    ("mistral-large", 131072),
    ("mistral", 32768),
    ("mixtral", 32768),
    ("phi-4", 16384),
    ("phi-3", 16384),
    ("gpt-5", 400000),
    ("gpt-4.1", 1047576),
    ("gpt-4o", 128000),
    ("gpt-4", 8192),
    ("gpt-3.5", 16385),
    ("claude", 200000),
    ("glm-5", 131072),
    ("glm-4.6", 131072),
    ("glm-4", 131072),
    ("glm", 32768),
]

# Quantization / variant suffixes stripped before catalog matching.
_QUANT_RE = re.compile(
    r"(-q\d+_?[\w.]*|-kquant-[\w.-]+|-UD|-it|-instruct|-chat|-native|-MTP|-Preserved"
    r"|-APEX|-I-Quality|-uncensored|-heretic|-A\d+B|-vanilla|-rev\d+|\.gguf)$",
    re.IGNORECASE,
)


def normalize_model_name(name: str) -> str:
    """Normalize a model name for catalog matching.

    Strips quant/variant suffixes and GGUF extensions so that e.g.
    ``muse-glimmer-30B-kquant-17gb`` matches the ``muse-glimmer`` entry.
    """
    if not name:
        return ""
    normalized = name.strip().lower()
    prev = None
    while prev != normalized:
        prev = normalized
        normalized = _QUANT_RE.sub("", normalized).rstrip("-.")
    return normalized


# ---------------------------------------------------------------------------
# Model families — capability gating, NOT output parsing.
#
# r105 is model-agnostic: no model-specific behavior may run against an
# arbitrary model's output. Any model-specific handling (e.g. Gemma-4
# channel syntax) is gated behind :func:`model_family` and only applies to
# the families listed below. Everything else passes through untouched.
# ---------------------------------------------------------------------------

# Family -> name fragments (longest matching fragment wins).
_MODEL_FAMILY_KEYS: dict[str, list[str]] = {
    "gemma-4": ["gemma-4", "gemma4", "muse-glimmer"],  # Gemma-4-style chat templates
    "gemma": ["gemma3", "gemma2", "gemma"],
    "qwen": ["qwen"],
    "llama": ["llama"],
    "mistral": ["mistral", "mixtral"],
    "deepseek": ["deepseek"],
    "glm": ["glm"],
    "gpt": ["gpt"],
    "claude": ["claude"],
    "phi": ["phi-4", "phi-3", "phi"],
}

# Model families whose chat templates emit Gemma-4 channel markers inline in
# content: ``<|tool_call|>`` blocks and ``<|channel|>thought`` blocks.
# For any other family, r105 treats content as opaque text — no regex
# parsing, no token stripping.
_GEMMA4_CHANNEL_SYNTAX_FAMILIES = frozenset({"gemma-4"})


def model_family(
    model_name: str,
    *,
    config_families: dict[str, str | None] | None = None,
) -> str | None:
    """Return the model family (``gemma-4``, ``qwen``, ...) or None.

    Config overrides (``model_families`` in config.json) take precedence over
    the built-in catalog: keys are model-name fragments (longest match wins)
    and values are family names, or ``null`` to force the model to be treated
    as opaque (no family at all). This is the config-driven hook for
    capability gating — a Gemma-4 fine-tune with a custom name can be forced
    into the ``gemma-4`` family, and a model the catalog would misclassify
    can be excluded.
    """
    normalized = normalize_model_name(model_name)
    if config_families:
        best: tuple[int, str | None] | None = None  # (fragment_length, family)
        for fragment, family in config_families.items():
            if fragment is None or not str(fragment).strip():
                continue
            frag = str(fragment).lower()
            if frag in normalized and (best is None or len(frag) > best[0]):
                best = (len(frag), family)
        if best is not None:
            # An explicit override wins — including None (force opaque).
            return best[1]
    best = None  # (fragment_length, family)
    for family, keys in _MODEL_FAMILY_KEYS.items():
        for key in keys:
            if key in normalized and (best is None or len(key) > best[0]):
                best = (len(key), family)
    return best[1] if best else None


def uses_gemma4_channel_syntax(
    model_name: str,
    config_families: dict[str, str | None] | None = None,
) -> bool:
    """True only for models known to emit Gemma-4 channel syntax.

    This is the single gate for all Gemma-4-specific handling (native
    ``<|tool_call|>`` parsing, ``<|channel|>thought`` capture, stray-token
    stripping). False for every other model — their output is never
    regex-interpreted or rewritten. ``config_families`` (from the
    ``model_families`` config key) can override the built-in family
    classification per model-name fragment.
    """
    return (
        model_family(model_name, config_families=config_families) in _GEMMA4_CHANNEL_SYNTAX_FAMILIES
    )


def _catalog_context(model_name: str) -> int | None:
    """Return the catalog context for *model_name*, or None.

    Longest matching fragment wins so specific entries (e.g. ``gemma-4-12b``)
    take precedence over family entries (``gemma``).
    """
    normalized = normalize_model_name(model_name)
    best: tuple[int, int] | None = None  # (key_length, tokens)
    for fragment, tokens in BUILTIN_MODEL_CONTEXTS:
        frag = fragment.lower()
        if frag in normalized and (best is None or len(frag) > best[0]):
            best = (len(frag), tokens)
    return best[1] if best else None


def _extract_context_from_model_entry(entry: dict[str, Any]) -> int | None:
    """Pull a context-window value out of a backend /v1/models entry.

    Handles llama.cpp (``meta.n_ctx``), Ollama (``context_length``),
    vLLM (``max_model_len``), and OpenAI-style (``context_window``).
    """
    candidates: list[Any] = []
    if isinstance(entry, dict):
        candidates.append(entry.get("context_window"))
        candidates.append(entry.get("context_length"))
        candidates.append(entry.get("max_model_len"))
        meta = entry.get("meta")
        if isinstance(meta, dict):
            candidates.append(meta.get("n_ctx"))
            candidates.append(meta.get("n_ctx_train"))
    for value in candidates:
        try:
            ivalue = int(value)
        except (TypeError, ValueError):
            continue
        if ivalue > 0:
            return ivalue
    return None


def resolve_context_tokens(
    model_name: str,
    *,
    config_contexts: dict[str, Any] | None = None,
    global_override: int | None = None,
    backend_context: int | None = None,
) -> int:
    """Resolve the effective context-window capacity for *model_name*.

    Priority: config override (per-model, then global) > backend probe >
    built-in catalog > DEFAULT_CONTEXT_TOKENS.

    *config_contexts* is the ``model_contexts`` map from config.json:
    keys are name fragments (longest match wins) and values are token counts.
    """
    # 1. Per-model config override
    if config_contexts:
        best: tuple[int, int] | None = None
        for fragment, raw in config_contexts.items():
            if not isinstance(raw, int) and not isinstance(raw, str):
                continue
            try:
                tokens = int(raw)
            except (TypeError, ValueError):
                continue
            if tokens <= 0:
                continue
            if fragment.lower() in normalize_model_name(model_name) and (
                best is None or len(fragment) > best[0]
            ):
                best = (len(fragment), tokens)
        if best:
            return best[1]

    # 2. Global config override
    if global_override and global_override > 0:
        return global_override

    # 3. Live backend probe
    if backend_context and backend_context > 0:
        return backend_context

    # 4. Built-in catalog
    catalog = _catalog_context(model_name)
    if catalog:
        return catalog

    # 5. Fallback
    return DEFAULT_CONTEXT_TOKENS
