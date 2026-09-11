"""Backend-agnostic HTTP client — OpenAI-compatible, llama.cpp, and Router.

r105 can connect to any OpenAI-compatible API (OpenAI, Ollama, vLLM, etc.),
to a llama-router backend (with profiles), or to Ollama's native API.

Auto-detection order:
1. R105_URL / --url set → RouterClient (has profiles)
2. OPENAI_API_KEY set → DirectClient (OpenAI-compatible)
3. otherwise → check local Ollama, then fall back to DirectClient

All backends share the same core chat methods:
- send() / async_send()        — one-shot
- async_continue()             — tool-loop continuation
- async_send_streaming()       — SSE streaming
- compact() / async_compact()  — summarization

``Client`` is the supported application-facing facade. The implementation
lives in :mod:`r105.backends` (client classes), :mod:`r105.payload` (wire
format), and :mod:`r105.sse` (streaming core); the concrete classes and parser
helpers remain re-exported for integrations that already use them.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any

import httpx

from r105.backends import (
    BACKEND_DIRECT,
    BACKEND_ROUTER,
    BackendCapabilities,
    BaseClient,
    DirectClient,
    RouterClient,
)
from r105.backends import create_client as _create_backend_client
from r105.constants import DEFAULT_HTTP_TIMEOUT
from r105.payload import (
    _build_payload,
    _extract_assistant_content,
    _extract_tool_calls,
    _inject_prompt_cache,
    _inject_reasoning_effort,
    _maybe_float,
    _metadata_from_state,
    _parse_gemma4_tool_calls,
    _parse_response,
)
from r105.state import ChatResult, ChatState


class Client:
    """Stable, backend-agnostic entry point for r105 API interactions.

    The application can use this small interface without knowing whether the
    request is handled by llama-router or a direct OpenAI-compatible server.
    The concrete backend remains available through :attr:`backend` for
    backend-specific operations such as router profiles.

    ``chat`` is the synchronous one-shot API.  Streaming and tool-loop
    continuation are async because they are driven by SSE and the Textual
    event loop.
    """

    def __init__(
        self,
        backend: BaseClient | None = None,
        *,
        base_url: str | None = None,
        backend_name: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        self.backend = backend or _create_backend_client(
            base_url=base_url,
            backend=backend_name,
            timeout=timeout,
        )

    @property
    def base_url(self) -> str:
        """The configured backend base URL."""
        return self.backend.base_url

    @property
    def capabilities(self) -> BackendCapabilities:
        """Capabilities reported by the selected backend."""
        return self.backend.capabilities

    def chat(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> ChatResult:
        """Send one message and return the parsed result."""
        return self.backend.send(message, state, tools)

    # ``send`` is retained as a source-compatible alias for older callers.
    send = chat

    async def chat_async(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> ChatResult:
        """Async one-shot chat API for event-loop callers."""
        return await self.backend.async_send(message, state, tools, client)

    async def stream_chat(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
        on_status: Callable[[str], None] | None = None,
    ) -> ChatResult:
        """Stream one message through the backend's SSE interface."""
        return await self.backend.async_send_streaming(
            message,
            state,
            tools,
            client,
            on_chunk=on_chunk,
            on_status=on_status,
        )

    async def continue_chat(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        client: httpx.AsyncClient | None = None,
        on_chunk: Callable[[str], None] | None = None,
        on_status: Callable[[str], None] | None = None,
    ) -> ChatResult:
        """Continue a conversation after local tool results are appended."""
        return await self.backend.async_continue(
            state,
            tools,
            client,
            on_chunk=on_chunk,
            on_status=on_status,
        )

    def list_models(self) -> dict[str, Any]:
        """Return the backend's available model catalog."""
        return self.backend.list_models()

    async def list_models_async(
        self, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        """Async model catalog lookup."""
        return await self.backend.async_list_models(client)

    def health(self) -> dict[str, Any]:
        """Run a synchronous backend health check."""
        return self.backend.health()

    async def health_async(
        self, client: httpx.AsyncClient | None = None
    ) -> dict[str, Any]:
        """Run an async backend health check."""
        return await self.backend.async_health(client)

    def compact(self, state: ChatState) -> ChatResult:
        """Compact the current conversation through the selected backend."""
        return self.backend.compact(state)

    async def compact_async(
        self, state: ChatState, client: httpx.AsyncClient | None = None
    ) -> ChatResult:
        """Async conversation compaction."""
        return await self.backend.async_compact(state, client)

    def probe_context(self, model_name: str) -> int | None:
        """Best-effort lookup of a model's backend context capacity."""
        return self.backend.probe_context(model_name)

    async def probe_context_async(
        self, model_name: str, client: httpx.AsyncClient | None = None
    ) -> int | None:
        """Async best-effort context-capacity lookup."""
        return await self.backend.async_probe_context(model_name, client)

    def __getattr__(self, name: str) -> Any:
        """Expose explicitly backend-specific APIs during the migration.

        Common operations above are the supported facade.  Delegation keeps
        existing router integrations such as ``profiles()`` working while
        callers migrate away from concrete backend checks.
        """
        return getattr(self.backend, name)


def create_client(
    base_url: str | None = None,
    backend: str | None = None,
    timeout: float = DEFAULT_HTTP_TIMEOUT,
) -> Client:
    """Create the high-level client facade with automatic backend selection."""
    return Client(
        backend=_create_backend_client(
            base_url=base_url,
            backend=backend,
            timeout=timeout,
        )
    )

__all__ = [
    "BACKEND_DIRECT",
    "BACKEND_ROUTER",
    "BackendCapabilities",
    "BaseClient",
    "Client",
    "DirectClient",
    "RouterClient",
    "_build_payload",
    "_extract_assistant_content",
    "_extract_tool_calls",
    "_inject_prompt_cache",
    "_inject_reasoning_effort",
    "_maybe_float",
    "_metadata_from_state",
    "_parse_gemma4_tool_calls",
    "_parse_response",
    "create_client",
]
