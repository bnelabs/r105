"""Regression tests for the public architecture layers added after the review."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import httpx
import pytest

from r105.client import Client, DirectClient
from r105.command_parser import CommandParser, CommandRegistry
from r105.config import validate_config_schema
from r105.logging import current_trace_id, trace_context
from r105.registry import ToolRegistry
from r105.sessions import SessionManager
from r105.state import ChatResult, ChatState


class _FakeBackend:
    base_url = "http://fake"
    capabilities = object()

    def send(self, message: str, state: ChatState, tools=None) -> ChatResult:
        return ChatResult(message, 0.1, None, None, {})

    async def async_send(self, message: str, state: ChatState, tools=None, client=None) -> ChatResult:
        return ChatResult(f"async:{message}", 0.1, None, None, {})

    async def async_send_streaming(
        self, message: str, state: ChatState, tools=None, client=None, **kwargs: Any
    ) -> ChatResult:
        callback = kwargs.get("on_chunk")
        if callback:
            callback(message)
        return ChatResult(f"stream:{message}", 0.1, None, None, {})

    async def async_continue(self, state: ChatState, tools=None, client=None, **kwargs: Any) -> ChatResult:
        return ChatResult("continued", 0.1, None, None, {})

    def list_models(self) -> dict[str, Any]:
        return {"data": [{"id": "fake-model"}]}

    async def async_list_models(self, client=None) -> dict[str, Any]:
        return self.list_models()

    def health(self) -> dict[str, Any]:
        return {"ok": True, "models_available": 1}

    async def async_health(self, client=None) -> dict[str, Any]:
        return self.health()

    def compact(self, state: ChatState) -> ChatResult:
        return ChatResult("compact", 0.1, None, None, {})

    async def async_compact(self, state: ChatState, client=None) -> ChatResult:
        return self.compact(state)

    def probe_context(self, model_name: str) -> int | None:
        return 4096

    async def async_probe_context(self, model_name: str, client=None) -> int | None:
        return 4096


def test_client_facade_delegates_stable_methods() -> None:
    client = Client(backend=_FakeBackend())  # type: ignore[arg-type]
    state = ChatState()

    assert client.chat("hello", state).content == "hello"
    assert client.list_models()["data"][0]["id"] == "fake-model"
    assert client.health()["ok"] is True
    assert asyncio.run(client.chat_async("hello", state)).content == "async:hello"
    assert asyncio.run(client.stream_chat("hello", state)).content == "stream:hello"


def test_direct_client_propagates_trace_header() -> None:
    state = ChatState(trace_id="trace-123")
    seen: list[str] = []

    def handler(request: httpx.Request) -> httpx.Response:
        seen.append(request.headers["X-R105-Trace-ID"])
        return httpx.Response(
            200,
            json={"choices": [{"message": {"content": "ok"}}]},
            request=request,
        )

    async def run() -> None:
        backend = DirectClient(base_url="http://testserver")
        transport = httpx.MockTransport(handler)
        async with httpx.AsyncClient(transport=transport) as http:
            result = await backend.async_send("hi", state, client=http)
        assert result.content == "ok"

    asyncio.run(run())
    assert seen == ["trace-123"]


def test_command_parser_and_registry_are_independent() -> None:
    parsed = CommandParser().parse('/session search "two words"')
    assert parsed is not None
    assert parsed.name == "/session"
    assert parsed.args == ["search", "two words"]

    registry: CommandRegistry[str] = CommandRegistry({"/help": "help"})
    registry.register("/health", "health")
    assert registry.get("/health") == "health"
    assert registry.suggest("/healt") == "/health"
    with pytest.raises(ValueError, match="already registered"):
        registry.register("/help", "replacement")


def test_registered_tool_exposes_protocol_metadata() -> None:
    registry = ToolRegistry()

    @registry.register(
        name="echo",
        description="Echo input",
        parameters={"value": {"type": "string"}},
        required=["value"],
    )
    def echo(arguments: dict[str, Any]) -> str:
        return str(arguments["value"])

    tool = registry.get("echo")
    assert tool is not None
    assert tool.parameters_schema["required"] == ["value"]
    assert registry.get_tools()[0].to_definition()["function"]["name"] == "echo"


def test_session_manager_uses_atomic_json_and_restores_trace(tmp_path: Path) -> None:
    manager = SessionManager(tmp_path)
    state = ChatState(trace_id="trace-session")
    state.history.append({"role": "user", "content": "hello"})

    path = manager.save_session(state, "demo")
    assert json.loads(path.read_text(encoding="utf-8"))["state"]["trace_id"] == "trace-session"
    assert not list(tmp_path.glob("*.tmp"))


def test_schema_and_trace_context_are_available() -> None:
    validate_config_schema()
    assert current_trace_id() is None
    with trace_context("trace-test"):
        assert current_trace_id() == "trace-test"
    assert current_trace_id() is None
