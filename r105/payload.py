"""Wire-format helpers — payload building and response parsing.

Shared by all backends (``r105.backends``). OpenAI-compatible chat-completion
format with template hooks for backend-specific metadata.
"""

from __future__ import annotations

import json
import re
import time
from typing import Any

from r105.skills import skill_messages
from r105.state import (
    DEFAULT_MODEL,
    ChatResult,
    ChatState,
)

# Gemma 4 native tool-call format: <|tool_call|>{"name":"...","arguments":{...}}
_GEMMA4_TOOL_CALL_RE = re.compile(
    r"<\|tool_call\|>\s*(\{.*?\})\s*(?:<\|tool_result\|>|$)", re.DOTALL
)


def _metadata_from_state(state: ChatState) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    if state.profile:
        metadata["profile"] = state.profile
    if state.quality:
        metadata["quality"] = state.quality
    return metadata


def _extract_assistant_content(raw: dict[str, Any]) -> str:
    choices = raw.get("choices") or []
    if not choices:
        return ""
    message = choices[0].get("message") or {}
    content = message.get("content", "")
    if isinstance(content, str):
        content = content.strip()
    else:
        content = str(content).strip()
    # Model-agnostic fallback: thinking models (Qwen3, DeepSeek, etc.) emit
    # their output in `reasoning_content`; when they run out of tokens during
    # reasoning, `content` stays empty. Surface the reasoning instead of a
    # blank reply.
    if not content:
        reasoning = message.get("reasoning_content")
        if isinstance(reasoning, str):
            content = reasoning.strip()
    return str(content) if not isinstance(content, str) else content


def _extract_tool_calls(raw: dict[str, Any]) -> list[dict[str, Any]]:
    choices = raw.get("choices") or []
    if not choices:
        return []
    message = choices[0].get("message") or {}
    calls = message.get("tool_calls") or []
    return [call for call in calls if isinstance(call, dict)]


def _parse_gemma4_tool_calls(content: str) -> list[dict[str, Any]] | None:
    """Parse Gemma 4 native tool-call format from content text.

    Some backends (llama.cpp without --jinja) return tool calls embedded
    in the content as <|tool_call|> JSON blocks rather than in the
    OpenAI-compatible tool_calls field.  This fallback extracts them.
    """
    matches = _GEMMA4_TOOL_CALL_RE.findall(content)
    if not matches:
        return None
    tool_calls: list[dict[str, Any]] = []
    for i, json_str in enumerate(matches):
        try:
            call_data = json.loads(json_str.strip())
            tool_calls.append({
                "id": f"call_gemma4_{i}",
                "type": "function",
                "function": {
                    "name": call_data.get("name", "unknown"),
                    "arguments": json.dumps(call_data.get("arguments", {})),
                },
            })
        except (json.JSONDecodeError, TypeError):
            continue
    return tool_calls if tool_calls else None


def _maybe_float(value: Any) -> float | None:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def _build_payload(message: str, state: ChatState, tools: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    """Build the request payload for a chat completion.

    Uses OpenAI-compatible format — shared by all backends.
    """
    messages = [*skill_messages(state), *state.history, {"role": "user", "content": message}]
    payload: dict[str, Any] = {
        "model": state.model if state.model else DEFAULT_MODEL,
        "messages": messages,
        "stream": False,
    }
    if state.max_tokens is not None:
        payload["max_tokens"] = state.max_tokens
    if state.json_mode:
        payload["response_format"] = {"type": "json_object"}
    if tools:
        payload["tools"] = tools
        payload["tool_choice"] = "auto"
    _inject_reasoning_effort(payload, state)
    _inject_prompt_cache(payload, state)
    return payload


def _inject_reasoning_effort(payload: dict[str, Any], state: ChatState) -> None:
    """Add the ``reasoning_effort`` field when an explicit level is set.

    ``auto`` omits the field (the backend decides) and ``off`` omits it too
    (backends that default to reasoning simply respond normally).
    """
    effort = getattr(state, "reasoning_effort", "auto")
    if effort in {"low", "medium", "high"}:
        payload["reasoning_effort"] = effort


def _inject_prompt_cache(payload: dict[str, Any], state: ChatState) -> None:
    """Opt into llama.cpp prompt-prefix caching when explicitly enabled."""
    if getattr(state, "cache_prompt", False):
        payload["cache_prompt"] = True


def _parse_response(raw: dict[str, Any], started: float) -> ChatResult:
    """Parse an API response into a ChatResult."""
    wall_seconds = time.perf_counter() - started
    content = _extract_assistant_content(raw)
    tool_calls = _extract_tool_calls(raw)
    timings = raw.get("timings") or {}
    return ChatResult(
        content=content,
        wall_seconds=wall_seconds,
        prompt_tps=_maybe_float(timings.get("prompt_per_second")),
        generation_tps=_maybe_float(timings.get("predicted_per_second")),
        raw=raw,
        tool_calls=tool_calls,
    )
