"""Abstract backend base — HTTP plumbing, compaction, and SSE entry point.

Concrete backends (``direct.py``, ``router.py``) implement the chat lifecycle
via the Template Method hooks ``_prepare_payload`` / ``_prepare_continue_payload``.
"""

from __future__ import annotations

import abc
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, cast

import httpx

from r105.constants import (
    COMPACT_PROFILE,
    COMPACT_PROMPT_TEMPLATE,
    COMPACT_RECENT_FRACTION,
    COMPACT_SUMMARY_TOKENS,
    DEFAULT_HTTP_TIMEOUT,
)
from r105.errors import RouterAPIError
from r105.logging import error as log_error
from r105.sse import stream_sse
from r105.state import (
    ChatResult,
    ChatState,
)


@dataclass
class BackendCapabilities:
    """What features a backend supports. Used to gate commands."""

    profiles: bool = False
    metadata: bool = False  # profile/quality metadata in payload


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
        # Per-phase timeouts: short connect/pool so a dead or half-dead
        # backend fails fast (seconds, not the full read budget), while
        # the read phase keeps the full budget for long generations
        # (default 300s).
        self.timeout = httpx.Timeout(
            connect=2.0,
            read=timeout,
            write=60.0,
            pool=10.0,
        )

    @property
    @abc.abstractmethod
    def capabilities(self) -> BackendCapabilities:
        ...

    # -- Model context probing ----------------------------------------------

    def probe_context(self, model_name: str) -> int | None:
        """Best-effort: resolve the model's context-window capacity.

        Returns None when the backend exposes no context metadata; callers
        fall back to the model catalog. Subclasses may override.
        """
        return None

    async def async_probe_context(
        self, model_name: str, client: httpx.AsyncClient | None = None
    ) -> int | None:
        """Async variant of :meth:`probe_context`."""
        del client  # unused in the base implementation
        return self.probe_context(model_name)

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
        timeout: float | tuple[float, float] | None = None,
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
        timeout: float | tuple[float, float] | None = None,
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
            raw_response = await req(url, **kwargs)
            response = cast(httpx.Response, raw_response)
        else:
            async with httpx.AsyncClient() as ac:
                req = getattr(ac, method.lower())
                raw_response = await req(url, **kwargs)
                response = cast(httpx.Response, raw_response)
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
        on_status: Callable[[str], None] | None = None,
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
        on_status: Callable[[str], None] | None = None,
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
        on_status: Callable[[str], None] | None = None,
        config_families: dict[str, str | None] | None = None,
    ) -> ChatResult:
        """Shared SSE streaming core used by DirectClient and RouterClient."""
        return await stream_sse(
            payload,
            url=self._url("/v1/chat/completions"),
            headers=self._headers(),
            timeout=self.timeout,
            client=client,
            on_chunk=on_chunk,
            on_status=on_status,
            config_families=config_families,
        )
