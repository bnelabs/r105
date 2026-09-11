"""Smoke tests for native provider adapters (payload construction + parsing).

No network access: only pure payload builders and response extractors run.
"""

import json

from r105.providers import (
    AnthropicAdapter,
    GeminiAdapter,
    _extract_text_from_anthropic,
    _extract_text_from_gemini,
    _native_usage,
    _openai_tools_to_anthropic,
    detect_provider,
)
from r105.state import ChatState


def _tools():
    return [{
        "type": "function",
        "function": {
            "name": "get_time",
            "description": "Current time",
            "parameters": {"type": "object", "properties": {}},
        },
    }]


class TestAnthropicAdapter:
    def test_payload_moves_system_out_of_messages(self):
        adapter = AnthropicAdapter(api_key="k")
        state = ChatState(model="claude-test")
        state.history = [
            {"role": "system", "content": "be concise"},
            {"role": "user", "content": "hi"},
        ]
        payload = adapter._payload("hello", state, _tools())
        assert payload["model"] == "claude-test"
        assert payload["max_tokens"] == 2048
        assert "be concise" in payload["system"]
        assert all(m["role"] != "system" for m in payload["messages"])
        assert payload["messages"][-1] == {"role": "user", "content": "hello"}
        assert payload["tools"][0]["name"] == "get_time"
        assert "input_schema" in payload["tools"][0]

    def test_payload_without_tools_has_no_tools_key(self):
        payload = AnthropicAdapter(api_key="k")._payload("hi", ChatState(model="m"))
        assert "tools" not in payload
        assert "system" not in payload

    def test_extract_text_and_tool_use(self):
        data = {
            "content": [
                {"type": "text", "text": "The time is"},
                {"type": "tool_use", "id": "toolu_1", "name": "get_time",
                 "input": {}},
            ],
        }
        text, calls = _extract_text_from_anthropic(data)
        assert text == "The time is"
        assert len(calls) == 1
        assert calls[0]["function"]["name"] == "get_time"
        assert json.loads(calls[0]["function"]["arguments"]) == {}

    def test_openai_tools_conversion_empty(self):
        assert _openai_tools_to_anthropic(None) is None
        assert _openai_tools_to_anthropic([]) is None


class TestGeminiAdapter:
    def test_payload_role_mapping(self):
        adapter = GeminiAdapter(api_key="k")
        state = ChatState(model="gemini-test")
        state.history = [
            {"role": "assistant", "content": "previous answer"},
            {"role": "system", "content": "dropped for gemini"},
        ]
        payload = adapter._payload("next?", state, _tools())
        roles = [c["role"] for c in payload["contents"]]
        assert roles == ["model", "user"]
        assert payload["contents"][0]["parts"] == [{"text": "previous answer"}]
        decls = payload["tools"][0]["function_declarations"]
        assert decls[0]["name"] == "get_time"

    def test_extract_text_and_function_call(self):
        data = {"candidates": [{"content": {"parts": [
            {"text": "result:"},
            {"functionCall": {"name": "get_time", "args": {}}},
        ]}}]}
        text, calls = _extract_text_from_gemini(data)
        assert text == "result:"
        assert calls[0]["function"]["name"] == "get_time"

    def test_extract_empty_candidates(self):
        assert _extract_text_from_gemini({}) == ("", [])
        assert _extract_text_from_gemini({"candidates": []}) == ("", [])


class TestDetectProvider:
    def test_none_without_keys(self, monkeypatch):
        monkeypatch.delenv("ANTHROPIC_API_KEY", raising=False)
        monkeypatch.delenv("GEMINI_API_KEY", raising=False)
        monkeypatch.delenv("GOOGLE_API_KEY", raising=False)
        assert detect_provider() is None

    def test_anthropic_preferred(self, monkeypatch):
        monkeypatch.setenv("ANTHROPIC_API_KEY", "x")
        monkeypatch.setenv("GEMINI_API_KEY", "y")
        assert detect_provider() == "anthropic"


def test_native_usage_normalization():
    assert _native_usage({"usage": {"input_tokens": 9, "output_tokens": 4}}) == (9, 4, 13)
    assert _native_usage({
        "usageMetadata": {
            "promptTokenCount": 20,
            "candidatesTokenCount": 5,
            "totalTokenCount": 25,
        }
    }) == (20, 5, 25)
