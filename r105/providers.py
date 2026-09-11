"""Native multi-provider adapters — Anthropic Messages API + Google Gemini.

r105 is backend-agnostic via OpenAI-compatible endpoints, but native adapters
unlock provider-specific features (Anthropic prompt caching, Gemini safety
settings) and expand the user base beyond OpenAI-compatible hosts.

Each adapter translates between the provider's native wire format and the
OpenAI-compatible ``ChatResult`` used internally, so the TUI / tool loop
works unchanged. Adapters are optional: they only require ``httpx`` (already
a dependency) and activate when the corresponding API key is present:

- ``ANTHROPIC_API_KEY`` (+ optional ``ANTHROPIC_BASE_URL``) → AnthropicAdapter
- ``GEMINI_API_KEY`` / ``GOOGLE_API_KEY`` (+ optional ``GEMINI_BASE_URL``) → GeminiAdapter

Usage::

    from r105.providers import detect_provider, AnthropicAdapter, GeminiAdapter
    adapter = detect_provider()  # None when no provider keys are set
"""

from __future__ import annotations

import os
import time
from typing import Any

import httpx

from r105.constants import DEFAULT_HTTP_TIMEOUT
from r105.state import DEFAULT_MODEL, ChatResult, ChatState


def _native_usage(data: dict[str, Any]) -> tuple[int | None, int | None, int | None]:
    """Normalize Anthropic/Gemini usage metadata to r105's usage fields."""
    usage = data.get("usage")
    if not isinstance(usage, dict):
        usage = data.get("usageMetadata")
    if not isinstance(usage, dict):
        return None, None, None

    def _int(value: Any) -> int | None:
        if isinstance(value, bool):
            return None
        try:
            parsed = int(value)
        except (TypeError, ValueError):
            return None
        return parsed if parsed >= 0 else None

    prompt = _int(usage.get("prompt_tokens", usage.get("input_tokens", usage.get("promptTokenCount"))))
    completion = _int(
        usage.get("completion_tokens", usage.get("output_tokens", usage.get("candidatesTokenCount")))
    )
    total = _int(usage.get("total_tokens", usage.get("totalTokenCount")))
    if total is None and prompt is not None and completion is not None:
        total = prompt + completion
    return prompt, completion, total


def _record_provider_usage(state: ChatState, result: ChatResult) -> None:
    state.last_backend_total_tokens = result.total_tokens
    state.last_backend_history_length = len(state.history)
    state.last_backend_model = state.model if result.total_tokens is not None else None


def _extract_text_from_anthropic(data: dict[str, Any]) -> tuple[str, list[dict[str, Any]]]:
    """Extract (content, tool_calls) from an Anthropic Messages response."""
    blocks = data.get("content") or []
    texts: list[str] = []
    tool_calls: list[dict[str, Any]] = []
    for i, block in enumerate(blocks):
        if not isinstance(block, dict):
            continue
        btype = block.get("type")
        if btype == "text":
            t = block.get("text", "")
            if isinstance(t, str):
                texts.append(t)
        elif btype == "tool_use":
            tool_calls.append({
                "id": block.get("id", f"call_anthropic_{i}"),
                "type": "function",
                "function": {
                    "name": block.get("name", "unknown"),
                    "arguments": str(block.get("input", "{}")),
                },
            })
    return "\n".join(texts).strip(), tool_calls


def _openai_tools_to_anthropic(tools: list[dict[str, Any]] | None) -> list[dict[str, Any]] | None:
    if not tools:
        return None
    out: list[dict[str, Any]] = []
    for t in tools:
        fn = t.get("function", t)
        out.append({
            "name": fn.get("name", "unknown"),
            "description": fn.get("description", ""),
            "input_schema": fn.get("parameters", {"type": "object"}),
        })
    return out


class AnthropicAdapter:
    """Native Anthropic Messages API adapter (https://docs.anthropic.com)."""

    DEFAULT_BASE = "https://api.anthropic.com"
    DEFAULT_VERSION = "2023-06-01"

    def __init__(
        self,
        api_key: str | None = None,
        base_url: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        self.api_key = api_key or os.environ.get("ANTHROPIC_API_KEY", "")
        self.base_url = (base_url or os.environ.get("ANTHROPIC_BASE_URL", self.DEFAULT_BASE)).rstrip("/")
        self.timeout = httpx.Timeout(connect=2.0, read=timeout, write=60.0, pool=10.0)

    def _headers(self) -> dict[str, str]:
        return {
            "Content-Type": "application/json",
            "x-api-key": self.api_key,
            "anthropic-version": self.DEFAULT_VERSION,
        }

    def _payload(self, message: str, state: ChatState, tools: list[dict[str, Any]] | None = None) -> dict[str, Any]:
        from r105.skills import skill_messages as _sm
        messages: list[dict[str, Any]] = [*_sm(state), *state.history, {"role": "user", "content": message}]
        # Anthropic separates system prompts from the message list.
        system_parts = [m["content"] for m in messages if m.get("role") == "system"]
        chat_messages = [m for m in messages if m.get("role") != "system"]
        payload: dict[str, Any] = {
            "model": state.model or DEFAULT_MODEL,
            "messages": chat_messages,
            "max_tokens": state.max_tokens or 2048,
        }
        if system_parts:
            payload["system"] = "\n\n".join(str(s) for s in system_parts)
        native_tools = _openai_tools_to_anthropic(tools)
        if native_tools:
            payload["tools"] = native_tools
        return payload

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        started = time.perf_counter()
        payload = self._payload(message, state, tools)
        own_client = client is None
        http = client or httpx.AsyncClient(timeout=self.timeout)
        try:
            resp = await http.post(f"{self.base_url}/v1/messages", json=payload, headers=self._headers())
            resp.raise_for_status()
            data = resp.json()
        finally:
            if own_client:
                await http.aclose()
        content, tool_calls = _extract_text_from_anthropic(data)
        prompt_tokens, completion_tokens, total_tokens = _native_usage(data)
        result = ChatResult(
            content=content, wall_seconds=time.perf_counter() - started,
            prompt_tps=None, generation_tps=None, raw=data, tool_calls=tool_calls,
            prompt_tokens=prompt_tokens, completion_tokens=completion_tokens,
            total_tokens=total_tokens,
        )
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": content}
        if tool_calls:
            assistant_msg["tool_calls"] = tool_calls
        state.history.extend([{"role": "user", "content": message}, assistant_msg])
        _record_provider_usage(state, result)
        return result


def _extract_text_from_gemini(data: dict[str, Any]) -> tuple[str, list[dict[str, Any]]]:
    cands = data.get("candidates") or []
    if not cands:
        return "", []
    parts = ((cands[0].get("content") or {}).get("parts")) or []
    texts: list[str] = []
    tool_calls: list[dict[str, Any]] = []
    for i, part in enumerate(parts):
        if not isinstance(part, dict):
            continue
        if "text" in part and isinstance(part["text"], str):
            texts.append(part["text"])
        if "functionCall" in part and isinstance(part["functionCall"], dict):
            fc = part["functionCall"]
            import json as _json
            tool_calls.append({
                "id": f"call_gemini_{i}",
                "type": "function",
                "function": {
                    "name": fc.get("name", "unknown"),
                    "arguments": _json.dumps(fc.get("args", {})),
                },
            })
    return "\n".join(texts).strip(), tool_calls


class GeminiAdapter:
    """Native Google Gemini (Generative Language) adapter."""

    DEFAULT_BASE = "https://generativelanguage.googleapis.com"

    def __init__(
        self,
        api_key: str | None = None,
        base_url: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        self.api_key = api_key or os.environ.get("GEMINI_API_KEY", "") or os.environ.get("GOOGLE_API_KEY", "")
        self.base_url = (base_url or os.environ.get("GEMINI_BASE_URL", self.DEFAULT_BASE)).rstrip("/")
        self.timeout = httpx.Timeout(connect=2.0, read=timeout, write=60.0, pool=10.0)

    def _payload(self, message: str, state: ChatState, tools: list[dict[str, Any]] | None = None) -> dict[str, Any]:
        from r105.skills import skill_messages as _sm
        history: list[dict[str, Any]] = [*_sm(state), *state.history, {"role": "user", "content": message}]
        contents = [
            {"role": "model" if m.get("role") == "assistant" else "user",
             "parts": [{"text": str(m.get("content", ""))}]}
            for m in history if m.get("role") in ("user", "assistant")
        ]
        payload: dict[str, Any] = {"contents": contents}
        if tools:
            payload["tools"] = [{"function_declarations": [
                {"name": (t.get("function", t).get("name", "unknown")),
                 "description": (t.get("function", t).get("description", "")),
                 "parameters": (t.get("function", t).get("parameters", {"type": "object"}))}
                for t in tools
            ]}]
        return payload

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        import json as _json  # noqa: F401  (kept local to avoid top-level cycle)
        started = time.perf_counter()
        model = state.model or "gemini-2.0-flash"
        payload = self._payload(message, state, tools)
        url = f"{self.base_url}/v1beta/models/{model}:generateContent?key={self.api_key}"
        own_client = client is None
        http = client or httpx.AsyncClient(timeout=self.timeout)
        try:
            resp = await http.post(url, json=payload)
            resp.raise_for_status()
            data = resp.json()
        finally:
            if own_client:
                await http.aclose()
        content, tool_calls = _extract_text_from_gemini(data)
        prompt_tokens, completion_tokens, total_tokens = _native_usage(data)
        result = ChatResult(
            content=content, wall_seconds=time.perf_counter() - started,
            prompt_tps=None, generation_tps=None, raw=data, tool_calls=tool_calls,
            prompt_tokens=prompt_tokens, completion_tokens=completion_tokens,
            total_tokens=total_tokens,
        )
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": content}
        if tool_calls:
            assistant_msg["tool_calls"] = tool_calls
        state.history.extend([{"role": "user", "content": message}, assistant_msg])
        _record_provider_usage(state, result)
        return result


def detect_provider() -> str | None:
    """Return 'anthropic' / 'gemini' when native keys are set, else None."""
    if os.environ.get("ANTHROPIC_API_KEY"):
        return "anthropic"
    if os.environ.get("GEMINI_API_KEY") or os.environ.get("GOOGLE_API_KEY"):
        return "gemini"
    return None
