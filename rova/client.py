"""Backend-agnostic HTTP client — OpenAI-compatible, Router, and Ollama.

Rova can connect to any OpenAI-compatible API (OpenAI, Ollama, vLLM, etc.),
to a llama-router backend (with profiles + RAG), or to Ollama's native API.

Auto-detection order:
1. ROVA_URL / --url set → RouterClient (has profiles + RAG)
2. OPENAI_API_KEY set → DirectClient (OpenAI-compatible)
3. otherwise → check local Ollama, then fall back to DirectClient

All backends share the same core chat methods:
- send() / async_send()        — one-shot
- async_continue()             — tool-loop continuation
- async_send_streaming()       — SSE streaming
- compact() / async_compact()  — summarization
"""

from __future__ import annotations

import abc
import json
import os
import time
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import httpx

from rova.constants import (
    COMPACT_PROFILE,
    COMPACT_PROMPT_TEMPLATE,
    COMPACT_RECENT_FRACTION,
    COMPACT_SUMMARY_TOKENS,
    DEFAULT_HTTP_TIMEOUT,
)
from rova.errors import RouterAPIError
from rova.logging import info as log_info
from rova.logging import error as log_error
from rova.skills import skill_messages
from rova.state import (
    DEFAULT_MODEL,
    ChatResult,
    ChatState,
)

# ---------------------------------------------------------------------------
# Shared helpers (used by all backends)
# ---------------------------------------------------------------------------


def _metadata_from_state(state: ChatState) -> dict[str, Any]:
    metadata: dict[str, Any] = {}
    if state.profile:
        metadata["profile"] = state.profile
    if state.rag is not None:
        metadata["rag"] = state.rag
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
        return content.strip()
    return str(content).strip()


def _extract_tool_calls(raw: dict[str, Any]) -> list[dict[str, Any]]:
    choices = raw.get("choices") or []
    if not choices:
        return []
    message = choices[0].get("message") or {}
    calls = message.get("tool_calls") or []
    return [call for call in calls if isinstance(call, dict)]


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
    return payload


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


# ---------------------------------------------------------------------------
# Backend support info
# ---------------------------------------------------------------------------


@dataclass
class BackendCapabilities:
    """What features a backend supports. Used to gate commands."""

    profiles: bool = False
    rag: bool = False
    metadata: bool = False  # profile/quality/rag metadata in payload


# ---------------------------------------------------------------------------
# Abstract base
# ---------------------------------------------------------------------------


class BaseClient(abc.ABC):
    """Abstract base for all backends.

    Subclasses must implement the core HTTP methods; the chat lifecycle
    methods are built on top.
    """

    def __init__(
        self,
        base_url: str,
        api_key: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        self.base_url = base_url.rstrip("/")
        self.api_key = api_key
        self.timeout = timeout

    @property
    @abc.abstractmethod
    def capabilities(self) -> BackendCapabilities:
        ...

    # -- HTTP helpers -------------------------------------------------------

    def _url(self, path: str) -> str:
        return f"{self.base_url}{path}"

    def _headers(self) -> dict[str, str]:
        h = {"Content-Type": "application/json"}
        if self.api_key:
            h["Authorization"] = f"Bearer {self.api_key}"
        return h

    def _sync_request(
        self,
        method: str,
        path: str,
        *,
        json: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> httpx.Response:
        """Issue a synchronous HTTP request."""
        req = getattr(httpx, method.lower())
        kwargs: dict[str, Any] = {
            "timeout": timeout if timeout is not None else self.timeout,
            "headers": self._headers(),
        }
        if json is not None:
            kwargs["json"] = json
        response = req(self._url(path), **kwargs)
        return self._check_response(response)

    async def _async_request(
        self,
        method: str,
        path: str,
        *,
        client: httpx.AsyncClient | None = None,
        json: dict[str, Any] | None = None,
        timeout: float | None = None,
    ) -> httpx.Response:
        """Issue an asynchronous HTTP request."""
        t = timeout if timeout is not None else self.timeout
        url = self._url(path)
        kwargs: dict[str, Any] = {
            "timeout": t,
            "headers": self._headers(),
        }
        if json is not None:
            kwargs["json"] = json

        if client is not None:
            req = getattr(client, method.lower())
            response = await req(url, **kwargs)
        else:
            async with httpx.AsyncClient() as ac:
                req = getattr(ac, method.lower())
                response = await req(url, **kwargs)
        return response

    @staticmethod
    def _check_response(response: httpx.Response) -> httpx.Response:
        """Raise RouterAPIError on non-2xx, otherwise return the response."""
        if response.is_error:
            log_error("api_error", status_code=response.status_code, url=str(response.url))
        try:
            response.raise_for_status()
        except httpx.HTTPStatusError as exc:
            raise RouterAPIError(
                f"API error {exc.response.status_code}: {exc.response.reason_phrase}",
                status_code=exc.response.status_code,
                response_body=exc.response.text[:500],
            ) from exc
        return response

    # -- Compact helpers ----------------------------------------------------

    @staticmethod
    def _make_empty_compact_result() -> ChatResult:
        return ChatResult(
            content="No conversation history to compact.",
            wall_seconds=0,
            prompt_tps=None,
            generation_tps=None,
            raw={},
        )

    @staticmethod
    def _compact_split(state: ChatState) -> tuple[list[dict[str, str]], list[dict[str, str]]]:
        keep_count = max(1, int(len(state.history) * COMPACT_RECENT_FRACTION))
        split = max(1, len(state.history) - keep_count)
        return state.history[:split], state.history[split:]

    def _build_compact_prompt(self, messages: list[dict[str, str]]) -> str:
        transcript = "\n\n".join(
            f"{m['role']}: {m['content']}" for m in messages
        )
        return COMPACT_PROMPT_TEMPLATE.format(transcript=transcript)

    def _make_compact_state(self, state: ChatState) -> ChatState:
        return ChatState(
            profile=COMPACT_PROFILE,
            quality="balanced",
            max_tokens=COMPACT_SUMMARY_TOKENS,
            skills_dir=state.skills_dir,
            context_tokens=state.context_tokens,
        )

    # -- Chat methods (must be implemented) ---------------------------------

    @abc.abstractmethod
    def send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        """Send a message synchronously and return the result."""
        ...

    @abc.abstractmethod
    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        """Send a message asynchronously."""
        ...

    @abc.abstractmethod
    async def async_continue(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        """Continue conversation after tool results."""
        ...

    @abc.abstractmethod
    async def async_send_streaming(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        """Send a message with SSE streaming."""
        ...

    @abc.abstractmethod
    async def async_health(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        """Check backend health."""
        ...

    @abc.abstractmethod
    async def async_list_models(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        """List available models."""
        ...

    # -- Non-abstract chat methods ------------------------------------------

    def compact(self, state: ChatState) -> ChatResult:
        """Summarize conversation history and replace it with the summary."""
        if not state.history:
            return self._make_empty_compact_result()

        older, recent = self._compact_split(state)
        prompt = self._build_compact_prompt(older)
        result = self.send(prompt, self._make_compact_state(state))

        state.history = [
            {"role": "system", "content": f"Conversation summary so far:\n{result.content}"},
            *recent,
        ]
        return result

    async def async_compact(
        self, state: ChatState, client: httpx.AsyncClient | None = None
    ) -> ChatResult:
        if not state.history:
            return self._make_empty_compact_result()

        older, recent = self._compact_split(state)
        prompt = self._build_compact_prompt(older)
        result = await self.async_send(prompt, self._make_compact_state(state), client=client)

        state.history = [
            {"role": "system", "content": f"Conversation summary so far:\n{result.content}"},
            *recent,
        ]
        return result

    # -- SSE streaming (shared) ---------------------------------------------

    async def _stream_sse(
        self,
        payload: dict[str, Any],
        client: httpx.AsyncClient | None,
        on_chunk: Callable[[str], None],
    ) -> ChatResult:
        """Shared SSE streaming core used by DirectClient and RouterClient."""
        started = time.perf_counter()

        content_parts: list[str] = []
        tool_call_deltas: dict[int, dict[str, Any]] = {}

        async def _read(http: httpx.AsyncClient) -> None:
            nonlocal content_parts, tool_call_deltas
            async with http.stream(
                "POST",
                self._url("/v1/chat/completions"),
                json=payload,
                headers=self._headers(),
                timeout=self.timeout,
            ) as response:
                response.raise_for_status()
                async for line in response.aiter_lines():
                    if not line.startswith("data: "):
                        continue
                    data = line[6:]
                    if data == "[DONE]":
                        break

                    try:
                        chunk = json.loads(data)
                    except json.JSONDecodeError:
                        continue

                    choices = chunk.get("choices") or []
                    if not choices:
                        continue
                    delta = choices[0].get("delta") or {}

                    content_delta = delta.get("content", "")
                    if isinstance(content_delta, str) and content_delta:
                        content_parts.append(content_delta)
                        on_chunk(content_delta)

                    tc_deltas = delta.get("tool_calls") or []
                    for tc in tc_deltas:
                        idx = tc.get("index", 0)
                        if idx not in tool_call_deltas:
                            tool_call_deltas[idx] = {
                                "id": tc.get("id", ""),
                                "type": "function",
                                "function": {"name": "", "arguments": ""},
                            }
                        entry = tool_call_deltas[idx]
                        if tc.get("id"):
                            entry["id"] = tc["id"]
                        func = tc.get("function") or {}
                        if func.get("name"):
                            entry["function"]["name"] += func["name"]
                        if func.get("arguments"):
                            entry["function"]["arguments"] += func["arguments"]

        if client is not None:
            await _read(client)
        else:
            async with httpx.AsyncClient() as ac:
                await _read(ac)

        content = "".join(content_parts)
        tool_calls = [tool_call_deltas[i] for i in sorted(tool_call_deltas)]

        raw: dict[str, Any] = {
            "choices": [{
                "message": {
                    "content": content,
                    "tool_calls": tool_calls if tool_calls else None,
                }
            }],
            "timings": {},
        }
        return _parse_response(raw, started)


# ---------------------------------------------------------------------------
# DirectClient — OpenAI-compatible API
# ---------------------------------------------------------------------------


class DirectClient(BaseClient):
    """Client for any OpenAI-compatible API.

    Supports: OpenAI, vLLM, Ollama (OpenAI mode), Groq, Together, etc.
    Does NOT support profiles, RAG, or metadata — those are RouterClient-only.

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

    # -- Sync API -----------------------------------------------------------

    def send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        started = time.perf_counter()
        raw = self._sync_request("POST", "/v1/chat/completions", json=payload).json()
        result = _parse_response(raw, started)
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    # -- Async API ----------------------------------------------------------

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        started = time.perf_counter()
        response = await self._async_request(
            "POST", "/v1/chat/completions", client=client, json=payload
        )
        response.raise_for_status()
        raw = response.json()
        result = _parse_response(raw, started)
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    async def async_continue(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        messages = [*skill_messages(state), *state.history]
        payload: dict[str, Any] = {
            "model": state.model if state.model else DEFAULT_MODEL,
            "messages": messages,
            "stream": on_chunk is not None,
        }
        if state.max_tokens is not None:
            payload["max_tokens"] = state.max_tokens
        if state.json_mode:
            payload["response_format"] = {"type": "json_object"}
        if tools:
            payload["tools"] = tools
            payload["tool_choice"] = "auto"

        if on_chunk is not None:
            result = await self._stream_sse(payload, client, on_chunk)
        else:
            started = time.perf_counter()
            response = await self._async_request(
                "POST", "/v1/chat/completions", client=client, json=payload
            )
            response.raise_for_status()
            raw = response.json()
            result = _parse_response(raw, started)

        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.append(assistant_msg)
        return result

    async def async_send_streaming(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        payload["stream"] = True
        result = await self._stream_sse(payload, client, on_chunk or (lambda _: None))
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    # -- Health / models ----------------------------------------------------

    async def async_health(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        """Check backend health by listing models."""
        try:
            data = await self.async_list_models(client)
            return {"ok": True, "models_available": len(data.get("data", []))}
        except Exception as exc:
            return {"ok": False, "error": str(exc)}

    def health(self) -> dict[str, Any]:
        try:
            data = self.list_models()
            return {"ok": True, "models_available": len(data.get("data", []))}
        except Exception as exc:
            return {"ok": False, "error": str(exc)}

    def list_models(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/v1/models", timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_list_models(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        response = await self._async_request("GET", "/v1/models", client=client, timeout=10.0)
        response.raise_for_status()
        return response.json()


# ---------------------------------------------------------------------------
# RouterClient — llama-router backend (profiles + RAG + metadata)
# ---------------------------------------------------------------------------


class RouterClient(DirectClient):
    """Client for llama-router — adds profiles, RAG, and metadata to DirectClient.

    The llama-router wraps any OpenAI-compatible endpoint and adds:
    - Task profiles (coding, creative, tool_agent, etc.)
    - RAG (Retrieval-Augmented Generation) with ingest/search/delete
    - Quality hints and throughput metrics in responses

    This is the *optional* upgrade backend. Without it, DirectClient works fine.
    """

    _DEFAULT_BASE = "http://127.0.0.1:8010"

    def __init__(
        self,
        base_url: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        resolved = base_url or os.environ.get("ROVA_URL", self._DEFAULT_BASE)
        # RouterClient doesn't use an API key — authentication is via the router
        super().__init__(resolved, api_key=None, timeout=timeout)

    @property
    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities(profiles=True, rag=True, metadata=True)

    # -- Metadata injection (overrides _build_payload style) -----------------

    @staticmethod
    def _inject_metadata(payload: dict[str, Any], state: ChatState) -> None:
        metadata = _metadata_from_state(state)
        if metadata:
            payload["metadata"] = metadata

    # -- Router-specific sync endpoints -------------------------------------

    def profiles(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/profiles", timeout=10.0)
        response.raise_for_status()
        return response.json()

    def ingest(self, paths: list[str] | None = None, urls: list[str] | None = None) -> dict[str, Any]:
        response = self._sync_request(
            "POST", "/rag/ingest",
            json={"paths": paths or [], "urls": urls or []},
        )
        response.raise_for_status()
        return response.json()

    def search(self, query: str, top_k: int = 5) -> dict[str, Any]:
        response = self._sync_request(
            "POST", "/rag/search",
            json={"query": query, "top_k": top_k},
        )
        response.raise_for_status()
        return response.json()

    def list_rag_documents(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/rag/documents", timeout=10.0)
        response.raise_for_status()
        return response.json()

    def delete_rag_document(self, doc_id: str) -> dict[str, Any]:
        response = self._sync_request("DELETE", f"/rag/documents/{doc_id}", timeout=10.0)
        response.raise_for_status()
        return response.json()

    # -- Router-specific async endpoints ------------------------------------

    async def async_profiles(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        response = await self._async_request("GET", "/profiles", client=client, timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_ingest(
        self,
        paths: list[str] | None = None,
        urls: list[str] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> dict[str, Any]:
        response = await self._async_request(
            "POST", "/rag/ingest", client=client,
            json={"paths": paths or [], "urls": urls or []},
        )
        response.raise_for_status()
        return response.json()

    async def async_search(
        self, query: str, top_k: int = 5, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        response = await self._async_request(
            "POST", "/rag/search", client=client,
            json={"query": query, "top_k": top_k},
        )
        response.raise_for_status()
        return response.json()

    async def async_list_rag_documents(
        self, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        response = await self._async_request("GET", "/rag/documents", client=client, timeout=10.0)
        response.raise_for_status()
        return response.json()

    async def async_delete_rag_document(
        self, doc_id: str, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        response = await self._async_request(
            "DELETE", f"/rag/documents/{doc_id}", client=client, timeout=10.0
        )
        response.raise_for_status()
        return response.json()

    # -- Override chat methods with metadata injection -----------------------

    def send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        self._inject_metadata(payload, state)
        started = time.perf_counter()
        raw = self._sync_request("POST", "/v1/chat/completions", json=payload).json()
        result = _parse_response(raw, started)
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    async def async_send(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        self._inject_metadata(payload, state)
        started = time.perf_counter()
        response = await self._async_request(
            "POST", "/v1/chat/completions", client=client, json=payload
        )
        response.raise_for_status()
        raw = response.json()
        result = _parse_response(raw, started)
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    async def async_continue(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        messages = [*skill_messages(state), *state.history]
        payload: dict[str, Any] = {
            "model": state.model if state.model else DEFAULT_MODEL,
            "messages": messages,
            "stream": on_chunk is not None,
        }
        self._inject_metadata(payload, state)
        if state.max_tokens is not None:
            payload["max_tokens"] = state.max_tokens
        if state.json_mode:
            payload["response_format"] = {"type": "json_object"}
        if tools:
            payload["tools"] = tools
            payload["tool_choice"] = "auto"

        if on_chunk is not None:
            result = await self._stream_sse(payload, client, on_chunk)
        else:
            started = time.perf_counter()
            response = await self._async_request(
                "POST", "/v1/chat/completions", client=client, json=payload
            )
            response.raise_for_status()
            raw = response.json()
            result = _parse_response(raw, started)

        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.append(assistant_msg)
        return result

    async def async_send_streaming(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
    ) -> ChatResult:
        payload = _build_payload(message, state, tools)
        self._inject_metadata(payload, state)
        payload["stream"] = True
        result = await self._stream_sse(payload, client, on_chunk or (lambda _: None))
        assistant_msg: dict[str, Any] = {"role": "assistant", "content": result.content}
        if result.tool_calls:
            assistant_msg["tool_calls"] = result.tool_calls
        state.history.extend([
            {"role": "user", "content": message},
            assistant_msg,
        ])
        return result

    # -- Health uses router's /health endpoint -------------------------------

    async def async_health(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        response = await self._async_request("GET", "/health", client=client, timeout=10.0)
        response.raise_for_status()
        return response.json()

    def health(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/health", timeout=10.0)
        response.raise_for_status()
        return response.json()


# ---------------------------------------------------------------------------
# Factory — auto-detect the best backend
# ---------------------------------------------------------------------------

BACKEND_ROUTER = "router"
BACKEND_DIRECT = "direct"


def create_client(
    base_url: str | None = None,
    backend: str | None = None,
    timeout: float = DEFAULT_HTTP_TIMEOUT,
) -> BaseClient:
    """Create the best available backend client.

    Detection order (when *backend* is not specified):
    1. If *base_url* is explicitly provided → RouterClient at that URL
    2. If ROVA_URL env var is set → RouterClient
    3. If OPENAI_API_KEY env var is set → DirectClient
    4. Otherwise → DirectClient at OPENAI_BASE_URL or default

    When *backend* is ``"router"``, force RouterClient (fails if unreachable).
    When *backend* is ``"direct"``, force DirectClient (no profiles/RAG).
    """
    explicit_url = base_url is not None and base_url != ""

    if backend == "router" or (backend is None and explicit_url):
        return RouterClient(base_url=base_url, timeout=timeout)

    if backend == "direct":
        return DirectClient(base_url=base_url, timeout=timeout)

    # Auto-detect
    router_url = base_url or os.environ.get("ROVA_URL")
    if router_url:
        return RouterClient(base_url=router_url, timeout=timeout)

    # Fallback to OpenAI-compatible
    return DirectClient(base_url=base_url, timeout=timeout)
