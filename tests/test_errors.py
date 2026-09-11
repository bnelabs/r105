"""Actionable formatting for backend failures."""

from __future__ import annotations

import httpx

from r105.errors import RouterAPIError, format_request_error


def test_auth_error_is_actionable() -> None:
    text = format_request_error(RouterAPIError("no", status_code=401), action="Send")
    assert "authentication" in text and "API key" in text


def test_rate_limit_error_is_actionable() -> None:
    text = format_request_error(RouterAPIError("slow down", status_code=429))
    assert "rate limited" in text and "retry" in text


def test_server_error_mentions_backend() -> None:
    text = format_request_error(RouterAPIError("down", status_code=503))
    assert "server error 503" in text and "llama-router" in text


def test_connection_error_mentions_url() -> None:
    request = httpx.Request("GET", "http://127.0.0.1:8010")
    text = format_request_error(httpx.ConnectError("refused", request=request))
    assert "unreachable" in text and "URL" in text
