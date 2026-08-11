"""Shared pytest fixtures for r105 tests."""

from __future__ import annotations

from pathlib import Path

import pytest

from r105.client import RouterClient
from r105.state import ChatState


@pytest.fixture(autouse=True)
def isolated_config(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Redirect r105 config writes to a temp dir so tests never touch
    ``~/.config/r105/config.json``.

    Slash commands that persist settings (``/theme``, ``/reasoning``,
    ``/permissions``, …) call ``save_config`` which writes through the
    module-level ``CONFIG_DIR``/``CONFIG_PATH`` globals.
    """
    from r105 import config as r105_config

    config_dir = tmp_path / "r105-config"
    monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
    monkeypatch.setattr(r105_config, "CONFIG_PATH", config_dir / "config.json")


@pytest.fixture
def chat_state() -> ChatState:
    """Return a fresh ChatState with defaults and a temp skills dir."""
    return ChatState(
        skills_dir=Path("/tmp/r105-test-skills"),
        context_tokens=262144,
    )


@pytest.fixture
def chat_state_with_history(chat_state: ChatState) -> ChatState:
    """Return a ChatState with a short conversation already in history."""
    chat_state.history = [
        {"role": "user", "content": "Hello, who are you?"},
        {"role": "assistant", "content": "I am an AI assistant."},
        {"role": "user", "content": "Search for Python 3.12 release notes."},
        {
            "role": "assistant",
            "content": "Let me search for that.",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "arguments": '{"query": "Python 3.12 release notes"}',
                    },
                }
            ],
        },
        {
            "role": "tool",
            "tool_call_id": "call_1",
            "name": "web_search",
            "content": '{"title": "Python 3.12 Release", "url": "...", "snippet": "..."}',
        },
    ]
    return chat_state


@pytest.fixture
def router_client() -> RouterClient:
    """Return a RouterClient pointed at a local URL."""
    return RouterClient(base_url="http://127.0.0.1:8010", timeout=30.0)


@pytest.fixture
def sample_raw_response() -> dict:
    """A representative non-streaming chat completion response from llama-router."""
    return {
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you today?",
                },
                "finish_reason": "stop",
            }
        ],
        "timings": {
            "prompt_per_second": 150.0,
            "predicted_per_second": 45.0,
        },
    }


@pytest.fixture
def sample_raw_tool_response() -> dict:
    """A representative response with tool_calls."""
    return {
        "choices": [
            {
                "message": {
                    "role": "assistant",
                    "content": "Let me search for that.",
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {
                                "name": "web_search",
                                "arguments": '{"query": "latest news"}',
                            },
                        }
                    ],
                },
                "finish_reason": "tool_calls",
            }
        ],
        "timings": {
            "prompt_per_second": 120.0,
            "predicted_per_second": 38.0,
        },
    }
