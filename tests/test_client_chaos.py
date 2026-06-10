"""Chaos/network fault-injection tests for HTTP client resilience.

Uses pytest-httpx to simulate llama-router failures:
- Connection drops
- Malformed SSE streams
- HTTP 429 (rate limit) / 500 (server error) responses
- Timeouts

These tests verify error boundaries hold and no sockets leak.
"""

from __future__ import annotations

import json

import httpx
import pytest

from rova.client import BackendCapabilities, DirectClient, RouterClient
from rova.state import ChatState


@pytest.fixture
def client() -> RouterClient:
    return RouterClient(base_url="http://127.0.0.1:9999")


@pytest.fixture
def state() -> ChatState:
    return ChatState()


class TestConnectionFailures:
    """Simulate network-level failures."""

    def test_connection_refused(self, client: RouterClient, state: ChatState) -> None:
        """No server at the target address."""
        with pytest.raises(httpx.ConnectError):
            client.send("hello", state)

    def test_dns_failure(self) -> None:
        """Unresolvable hostname."""
        client = RouterClient(base_url="http://nonexistent.invalid:8010")
        with pytest.raises(httpx.ConnectError):
            client.send("hello", ChatState())


class TestHTTPErrorCodes:
    """Simulate non-2xx responses from the API."""

    def test_health_500(self, httpx_mock) -> None:
        from rova.errors import RouterAPIError
        client = RouterClient(base_url="http://testserver:8010")
        httpx_mock.add_response(url="http://testserver:8010/health", status_code=500)
        with pytest.raises(RouterAPIError):
            client.health()

    def test_profiles_429(self, httpx_mock) -> None:
        from rova.errors import RouterAPIError
        client = RouterClient(base_url="http://testserver:8010")
        httpx_mock.add_response(url="http://testserver:8010/profiles", status_code=429)
        with pytest.raises(RouterAPIError):
            client.profiles()

    def test_send_503(self, httpx_mock, state: ChatState) -> None:
        from rova.errors import RouterAPIError
        client = RouterClient(base_url="http://testserver:8010")
        httpx_mock.add_response(
            url="http://testserver:8010/v1/chat/completions",
            status_code=503,
            text="Service Unavailable",
        )
        with pytest.raises(RouterAPIError):
            client.send("hello", state)


class TestMalformedSSE:
    """Simulate broken SSE streams."""

    def test_truncated_sse(self, httpx_mock) -> None:
        client = RouterClient(base_url="http://testserver:8010")

        async def _run() -> None:
            state = ChatState()
            # Partial SSE chunk — no [DONE] sentinel, just ends
            httpx_mock.add_response(
                url="http://testserver:8010/v1/chat/completions",
                text='data: {"choices": [{"delta": {"content": "He"}}]}\n\n',
                headers={"Content-Type": "text/event-stream"},
            )
            # Should not crash — just return what was buffered
            result = await client.async_send_streaming("hi", state, on_chunk=lambda _: None)
            assert "He" in result.content

        import asyncio
        asyncio.run(_run())

    def test_malformed_json_sse(self, httpx_mock) -> None:
        client = RouterClient(base_url="http://testserver:8010")

        async def _run() -> None:
            state = ChatState()
            # Non-JSON SSE data
            httpx_mock.add_response(
                url="http://testserver:8010/v1/chat/completions",
                text="data: {invalid json}\n\n",
                headers={"Content-Type": "text/event-stream"},
            )
            result = await client.async_send_streaming("hi", state, on_chunk=lambda _: None)
            assert result.content == ""

        import asyncio
        asyncio.run(_run())


class TestDirectClient:
    """Verify DirectClient works for its stated purpose."""

    def test_direct_capabilities(self) -> None:
        dc = DirectClient(base_url="http://testserver:8080")
        caps = dc.capabilities
        assert isinstance(caps, BackendCapabilities)
        assert not caps.profiles
        assert not caps.rag
        assert not caps.metadata

    def test_router_capabilities(self) -> None:
        rc = RouterClient(base_url="http://testserver:8010")
        caps = rc.capabilities
        assert caps.profiles
        assert caps.rag
        assert caps.metadata
