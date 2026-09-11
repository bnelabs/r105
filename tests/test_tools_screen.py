"""Tests for the Ctrl+T tool-call inspection screen (pure logic)."""

from __future__ import annotations

from typing import Any

from r105.state import ChatState
from r105.tui.screens.tools_screen import ToolsScreen, _collect_tool_calls


def _history() -> list[dict[str, Any]]:
    return [
        {"role": "user", "content": "what is 1+1?"},
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "calculate",
                        "arguments": '{"expression": "1+1"}',
                    },
                }
            ],
        },
        {
            "role": "tool",
            "tool_call_id": "call_1",
            "name": "calculate",
            "content": "2",
        },
    ]


def test_collect_pairs_results_by_id() -> None:
    calls = _collect_tool_calls(_history())
    assert len(calls) == 1
    assert calls[0]["name"] == "calculate"
    assert calls[0]["result"] == "2"


def test_collect_missing_result_is_empty() -> None:
    history = [
        {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_9",
                    "type": "function",
                    "function": {"name": "web_search", "arguments": "{}"},
                }
            ],
        }
    ]
    calls = _collect_tool_calls(history)
    assert len(calls) == 1
    assert calls[0]["result"] == ""


def test_render_empty_state() -> None:
    screen = ToolsScreen(ChatState())
    assert "No tool calls yet" in screen._render_tools()


def test_render_lists_calls_with_results() -> None:
    state = ChatState()
    state.history.extend(_history())
    text = ToolsScreen(state)._render_tools()
    assert "calculate" in text
    assert "1+1" in text
    assert "1 tool call(s) shown" in text


def test_render_filter_no_match() -> None:
    state = ChatState()
    state.history.extend(_history())
    screen = ToolsScreen(state)
    screen._filter = "zzz-no-such-tool"
    assert "No matching tool calls" in screen._render_tools()
