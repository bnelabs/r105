"""Direct backend — any OpenAI-compatible API."""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable
from typing import Any, cast

import httpx

from r105.backends.base import BackendCapabilities, BaseClient
from r105.constants import DEFAULT_HTTP_TIMEOUT
from r105.errors import RouterAPIError
from r105.model_catalog import _extract_context_from_model_entry
from r105.payload import (
    _build_payload,
    _inject_prompt_cache,
    _inject_reasoning_effort,
    _parse_response,
)
from r105.skills import skill_messages
from r105.state import (
    DEFAULT_MODEL,
    ChatResult,
    ChatState,
)


class DirectClient(BaseClient):
    """Client for any OpenAI-compatible API.

    Supports: OpenAI, vLLM, Ollama (OpenAI mode), Groq, Together, etc.
    Does NOT support profiles or metadata — those are RouterClient-only.

    Environment variables:
        OPENAI_API_KEY     — API key (optional, for authenticated endpoints)
        OPENAI_BASE_URL    — base URL (default: https://api.openai.com/v1)
    """

    _DEFAULT_BASE = "https://api.openai.com/v1"

    def __init__(
        self,
        base_url: str | None = None,
        api_key: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        resolved_base = base_url or os.environ.get("OPENAI_BASE_URL", self._DEFAULT_BASE)
        resolved_key = api_key or os.environ.get("OPENAI_API_KEY")
        super().__init__(resolved_base, resolved_key, timeout)

    @property
    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities()

    # -- Payload hooks (Template Method) ------------------------------------

    def _prepare_payload(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        """Build the chat-completion payload for a one-shot send.

        Subclasses override this hook instead of copy-pasting ``send()`` /
        ``async_send()`` / ``async_send_streaming()``. The base implementation
        returns the backend-agnostic OpenAI-compatible payload.
        """
        return _build_payload(message, state, tools)

    def _prepare_continue_payload(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        stream: bool = False,
    ) -> dict[str, Any]:
        """Build the payload for a tool-loop continuation.

        Subclasses override this hook to inject backend-specific metadata
        without duplicating the ``async_continue()`` control flow.
        """
        messages = [*skill_messages(state), *state.history]
        payload: dict[str, Any] = {
            "model": state.model if state.model else DEFAULT_MODEL,
            "messages": messages,
            "stream": stream,
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

    @staticmethod
    def _record_backend_usage(state: ChatState, result: ChatResult) -> None:
        """Keep exact provider usage valid only for the history it measured."""
        state.last_backend_total_tokens = result.total_tokens
        state.last_backend_history_length = len(state.history)
        state.last_backend_model = state.model if result.total_tokens is not None else None

    @staticmethod
    def _record_send(message: str, state: ChatState, result: ChatResult) -> None:
        """Append user + assistant messages after a one-shot send."""
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        DirectClient._record_backend_usage(state, result)

    @staticmethod
    def _record_continue(state: ChatState, result: ChatResult) -> None:
        """Append the assistant message after a continuation."""
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.append(assistant_msg)
        DirectClient._record_backend_usage(state, result)

    # -- Sync API -----------------------------------------------------------

    def send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        payload = self._prepare_payload(message, state, tools)
        started = time.perf_counter()
        raw = self._sync_request("POST", "/v1/chat/completions", json=payload).json()
        result = _parse_response(raw, started)
        self._record_send(message, state, result)
        return result

    # -- Async API ----------------------------------------------------------

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        payload = self._prepare_payload(message, state, tools)
        started = time.perf_counter()
        response = await self._async_request(
            "POST", "/v1/chat/completions", client=client, json=payload
        )
        response.raise_for_status()
        raw = response.json()
        result = _parse_response(raw, started)
        self._record_send(message, state, result)
        return result

    async def async_continue(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
        on_status: Callable[[str], None] | None = None,
    ) -> ChatResult:
        payload = self._prepare_continue_payload(state, tools, stream=on_chunk is not None)

        if on_chunk is not None:
            result = await self._stream_sse(
                payload,
                client,
                on_chunk,
                on_status=on_status,
                config_families=state.model_families,
            )
        else:
            started = time.perf_counter()
            response = await self._async_request(
                "POST", "/v1/chat/completions", client=client, json=payload
            )
            response.raise_for_status()
            raw = response.json()
            result = _parse_response(raw, started)

        self._record_continue(state, result)
        return result

    async def async_send_streaming(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
        on_status: Callable[[str], None] | None = None,
    ) -> ChatResult:
        payload = self._prepare_payload(message, state, tools)
        payload["stream"] = True
        result = await self._stream_sse(
            payload,
            client,
            on_chunk or (lambda _: None),
            on_status=on_status,
            config_families=state.model_families,
        )
        self._record_send(message, state, result)
        return result

    # -- Health / models ----------------------------------------------------

    async def async_health(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        """Check backend health by listing models."""
        try:
            data = await self.async_list_models(client)
            return {"ok": True, "models_available": len(data.get("data", []))}
        except (httpx.HTTPError, RouterAPIError, OSError, TimeoutError) as exc:
            return {"ok": False, "error": str(exc)}

    def health(self) -> dict[str, Any]:
        try:
            data = self.list_models()
            return {"ok": True, "models_available": len(data.get("data", []))}
        except (httpx.HTTPError, RouterAPIError, OSError, TimeoutError) as exc:
            return {"ok": False, "error": str(exc)}

    def list_models(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/v1/models", timeout=10.0)
        response.raise_for_status()
        return cast(dict[str, Any], response.json())

    async def async_list_models(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        response = await self._async_request("GET", "/v1/models", client=client, timeout=10.0)
        response.raise_for_status()
        return cast(dict[str, Any], response.json())

    # -- Model context probing ----------------------------------------------

    def probe_context(self, model_name: str) -> int | None:
        """Probe the backend for the active model's context-window capacity.

        Sources tried, in order:
          1. llama.cpp ``/props`` -> ``default_generation_settings.n_ctx``
          2. ``/v1/models`` entry metadata (``meta.n_ctx``, ``context_window``,
             ``context_length``, ``max_model_len``)
        Returns None if the backend exposes no context metadata.

        Probing is best-effort with short (connect, read) timeouts so a slow
        or half-dead backend can never stall startup for more than ~2s total.
        """
        # 1. llama.cpp-style /props probe
        try:
            response = self._sync_request("GET", "/props", timeout=(1.0, 1.0))
            if response.status_code == 200:
                data = response.json()
                n_ctx = (data.get("default_generation_settings") or {}).get("n_ctx")
                try:
                    ivalue = int(n_ctx) if n_ctx is not None else 0
                except (TypeError, ValueError):
                    ivalue = 0
                if ivalue > 0:
                    return ivalue
        except (httpx.HTTPError, ValueError, OSError):
            pass

        # 2. /v1/models metadata probe (direct request: list_models() uses a
        # long timeout that would stall startup on an unresponsive backend)
        try:
            response = self._sync_request("GET", "/v1/models", timeout=(1.0, 1.0))
            if response.status_code != 200:
                return None
            data = response.json()
            for entry in data.get("data") or []:
                entry_id = str(entry.get("id", ""))
                if entry_id and (entry_id == model_name or entry_id in model_name):
                    context = _extract_context_from_model_entry(entry)
                    if context:
                        return context
        except (httpx.HTTPError, ValueError, OSError, json.JSONDecodeError):
            pass

        return None

    async def async_probe_context(
        self, model_name: str, client: httpx.AsyncClient | None = None
    ) -> int | None:
        """Async variant of :meth:`probe_context`."""
        # 1. llama.cpp-style /props probe
        try:
            response = await self._async_request("GET", "/props", client=client, timeout=(1.0, 1.0))
            if response.status_code == 200:
                data = response.json()
                n_ctx = (data.get("default_generation_settings") or {}).get("n_ctx")
                try:
                    ivalue = int(n_ctx) if n_ctx is not None else 0
                except (TypeError, ValueError):
                    ivalue = 0
                if ivalue > 0:
                    return ivalue
        except (httpx.HTTPError, ValueError, OSError):
            pass

        # 2. /v1/models metadata probe (direct request, short timeout)
        try:
            response = await self._async_request("GET", "/v1/models", client=client, timeout=(1.0, 1.0))
            if response.status_code != 200:
                return None
            data = response.json()
            for entry in data.get("data") or []:
                entry_id = str(entry.get("id", ""))
                if entry_id and (entry_id == model_name or entry_id in model_name):
                    context = _extract_context_from_model_entry(entry)
                    if context:
                        return context
        except (httpx.HTTPError, ValueError, OSError, json.JSONDecodeError):
            pass

        return None
