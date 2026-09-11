"""Router backend — llama-router with profiles and metadata."""

from __future__ import annotations

import os
from typing import Any, cast

import httpx

from r105.backends.base import BackendCapabilities
from r105.backends.direct import DirectClient
from r105.constants import DEFAULT_HTTP_TIMEOUT
from r105.payload import _metadata_from_state
from r105.state import ChatState


class RouterClient(DirectClient):
    """Client for llama-router — adds profiles and metadata to DirectClient.

    The llama-router wraps any OpenAI-compatible endpoint and adds:
    - Task profiles (coding, creative, tool_agent, etc.)
    - Quality hints and throughput metrics in responses

    This is the *optional* upgrade backend. Without it, DirectClient works fine.
    """

    _DEFAULT_BASE = "http://127.0.0.1:8010"

    def __init__(
        self,
        base_url: str | None = None,
        timeout: float = DEFAULT_HTTP_TIMEOUT,
    ) -> None:
        resolved = base_url or os.environ.get("R105_URL", self._DEFAULT_BASE)
        # RouterClient doesn't use an API key — authentication is via the router
        super().__init__(resolved, api_key=None, timeout=timeout)

    @property
    def capabilities(self) -> BackendCapabilities:
        return BackendCapabilities(profiles=True, metadata=True)

    # -- Payload hooks (Template Method: only override payload building) ----

    @staticmethod
    def _inject_metadata(payload: dict[str, Any], state: ChatState) -> None:
        metadata = _metadata_from_state(state)
        if metadata:
            payload["metadata"] = metadata

    def _prepare_payload(
        self,
        message: str,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        payload = super()._prepare_payload(message, state, tools)
        self._inject_metadata(payload, state)
        return payload

    def _prepare_continue_payload(
        self,
        state: ChatState,
        tools: list[dict[str, Any]] | None = None,
        stream: bool = False,
    ) -> dict[str, Any]:
        payload = super()._prepare_continue_payload(state, tools, stream=stream)
        self._inject_metadata(payload, state)
        return payload

    # -- Router-specific sync endpoints -------------------------------------

    def profiles(self) -> dict[str, Any]:
        response = self._sync_request("GET", "/profiles", timeout=10.0)
        response.raise_for_status()
        return cast(dict[str, Any], response.json())

    # -- Router-specific async endpoints ------------------------------------

    async def async_profiles(self, client: httpx.AsyncClient | None = None) -> dict[str, Any]:
        response = await self._async_request("GET", "/profiles", client=client, timeout=10.0)
        response.raise_for_status()
        return cast(dict[str, Any], response.json())

    # -- Health uses router's /health endpoint -------------------------------

    async def async_health(
        self,
        client: httpx.AsyncClient | None = None,
        *,
        trace_id: str | None = None,
    ) -> dict[str, Any]:
        response = await self._async_request(
            "GET", "/health", client=client, timeout=10.0, trace_id=trace_id
        )
        response.raise_for_status()
        return cast(dict[str, Any], response.json())

    def health(self, *, trace_id: str | None = None) -> dict[str, Any]:
        response = self._sync_request("GET", "/health", timeout=10.0, trace_id=trace_id)
        response.raise_for_status()
        return cast(dict[str, Any], response.json())
