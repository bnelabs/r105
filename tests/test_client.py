"""Unit tests for RouterClient parsing helpers and payload building."""

from __future__ import annotations

import time

from rova.client import (
    RouterClient,
    _build_payload,
    _extract_assistant_content,
    _extract_tool_calls,
    _maybe_float,
    _metadata_from_state,
    _parse_response,
)
from rova.state import DEFAULT_MODEL, ChatState


class TestParseHelpers:
    """Tests for response parsing helper functions."""

    def test_extract_assistant_content_normal(self, sample_raw_response: dict) -> None:
        result = _extract_assistant_content(sample_raw_response)
        assert result == "Hello! How can I help you today?"

    def test_extract_assistant_content_empty(self) -> None:
        assert _extract_assistant_content({}) == ""
        assert _extract_assistant_content({"choices": []}) == ""

    def test_extract_assistant_content_no_message(self) -> None:
        raw = {"choices": [{}]}
        assert _extract_assistant_content(raw) == ""

    def test_extract_assistant_content_none_content(self) -> None:
        raw = {"choices": [{"message": {"content": None}}]}
        assert _extract_assistant_content(raw) == "None"

    def test_extract_tool_calls_present(self, sample_raw_tool_response: dict) -> None:
        calls = _extract_tool_calls(sample_raw_tool_response)
        assert len(calls) == 1
        assert calls[0]["id"] == "call_abc"

    def test_extract_tool_calls_empty(self) -> None:
        assert _extract_tool_calls({}) == []
        assert _extract_tool_calls({"choices": []}) == []

    def test_extract_tool_calls_no_tool_calls(self, sample_raw_response: dict) -> None:
        assert _extract_tool_calls(sample_raw_response) == []

    def test_maybe_float_valid(self) -> None:
        assert _maybe_float("3.14") == 3.14
        assert _maybe_float(42) == 42.0
        assert _maybe_float(0) == 0.0

    def test_maybe_float_invalid(self) -> None:
        assert _maybe_float("nope") is None
        assert _maybe_float(None) is None
        assert _maybe_float([1, 2]) is None


class TestBuildPayload:
    """Tests for _build_payload."""

    def test_basic_payload(self, chat_state: ChatState) -> None:
        payload = _build_payload("hello", chat_state)
        assert payload["model"] == DEFAULT_MODEL
        assert payload["stream"] is False
        assert payload["messages"][-1] == {"role": "user", "content": "hello"}

    def test_payload_with_tools(self, chat_state: ChatState) -> None:
        tools = [{"type": "function", "function": {"name": "test"}}]
        payload = _build_payload("hello", chat_state, tools=tools)
        assert payload["tools"] == tools
        assert payload["tool_choice"] == "auto"

    def test_payload_with_max_tokens(self, chat_state: ChatState) -> None:
        chat_state.max_tokens = 2048
        payload = _build_payload("hello", chat_state)
        assert payload["max_tokens"] == 2048

    def test_payload_with_json_mode(self, chat_state: ChatState) -> None:
        chat_state.json_mode = True
        payload = _build_payload("hello", chat_state)
        assert payload["response_format"] == {"type": "json_object"}

    def test_payload_with_metadata(self, chat_state: ChatState) -> None:
        chat_state.profile = "coding"
        chat_state.quality = "best"
        payload = _build_payload("hello", chat_state)
        assert payload["metadata"] == {"profile": "coding", "quality": "best"}

    def test_payload_includes_history(self, chat_state: ChatState) -> None:
        chat_state.history = [{"role": "user", "content": "prior"}]
        payload = _build_payload("new", chat_state)
        assert len(payload["messages"]) == 2  # history + new user message
        assert payload["messages"][0] == {"role": "user", "content": "prior"}


class TestMetadataFromState:
    """Tests for _metadata_from_state."""

    def test_all_empty(self, chat_state: ChatState) -> None:
        assert _metadata_from_state(chat_state) == {}

    def test_profile_only(self, chat_state: ChatState) -> None:
        chat_state.profile = "coding"
        assert _metadata_from_state(chat_state) == {"profile": "coding"}

    def test_rag_only(self, chat_state: ChatState) -> None:
        chat_state.rag = True
        assert _metadata_from_state(chat_state) == {"rag": True}

    def test_quality_only(self, chat_state: ChatState) -> None:
        chat_state.quality = "fast"
        assert _metadata_from_state(chat_state) == {"quality": "fast"}

    def test_all_set(self, chat_state: ChatState) -> None:
        chat_state.profile = "coding"
        chat_state.rag = False
        chat_state.quality = "balanced"
        result = _metadata_from_state(chat_state)
        assert result == {"profile": "coding", "rag": False, "quality": "balanced"}


class TestParseResponse:
    """Tests for _parse_response."""

    def test_basic_response(self, sample_raw_response: dict) -> None:
        started = time.perf_counter() - 2.0  # simulate 2s elapsed
        result = _parse_response(sample_raw_response, started)
        assert result.content == "Hello! How can I help you today?"
        assert result.wall_seconds > 0
        assert result.prompt_tps == 150.0
        assert result.generation_tps == 45.0
        assert result.tool_calls == []

    def test_response_with_tools(self, sample_raw_tool_response: dict) -> None:
        started = time.perf_counter() - 1.0
        result = _parse_response(sample_raw_tool_response, started)
        assert len(result.tool_calls) == 1
        assert result.tool_calls[0]["id"] == "call_abc"
        assert result.content == "Let me search for that."

    def test_response_without_timings(self) -> None:
        raw = {"choices": [{"message": {"content": "Hi"}}]}
        started = time.perf_counter() - 0.5
        result = _parse_response(raw, started)
        assert result.content == "Hi"
        assert result.prompt_tps is None
        assert result.generation_tps is None


class TestRouterClientInit:
    """Tests for RouterClient construction."""

    def test_url_stripping(self) -> None:
        client = RouterClient(base_url="http://127.0.0.1:8010/")
        assert client.base_url == "http://127.0.0.1:8010"

    def test_default_url(self) -> None:
        client = RouterClient()
        assert client.base_url == "http://127.0.0.1:8010"

    def test_custom_timeout(self) -> None:
        client = RouterClient(timeout=60.0)
        assert client.timeout == 60.0
