"""Tests for reasoning-effort and permission-posture features."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from r105.client import _build_payload, _inject_reasoning_effort
from r105.commands import handle_slash_command
from r105.sandbox import (
    posture_allows_tool,
    set_posture,
    current_posture,
)
from r105.state import (
    VALID_PERMISSION_POSTURES,
    VALID_REASONING_EFFORTS,
    ChatState,
)


def _run(coro):
    """Run an async slash command synchronously in tests."""
    return asyncio.run(coro)


class TestReasoningEffortPayload:
    def test_explicit_level_injected(self) -> None:
        state = ChatState(reasoning_effort="high")
        payload = _build_payload("hello", state)
        assert payload["reasoning_effort"] == "high"

    def test_medium_and_low_injected(self) -> None:
        for level in ("low", "medium"):
            state = ChatState(reasoning_effort=level)
            payload = _build_payload("hello", state)
            assert payload["reasoning_effort"] == level

    def test_auto_omits_field(self) -> None:
        state = ChatState(reasoning_effort="auto")
        payload = _build_payload("hello", state)
        assert "reasoning_effort" not in payload

    def test_off_omits_field(self) -> None:
        state = ChatState(reasoning_effort="off")
        payload = _build_payload("hello", state)
        assert "reasoning_effort" not in payload

    def test_inject_helper_direct(self) -> None:
        payload: dict[str, object] = {}
        state = ChatState(reasoning_effort="medium")
        _inject_reasoning_effort(payload, state)
        assert payload["reasoning_effort"] == "medium"

    def test_valid_efforts(self) -> None:
        assert VALID_REASONING_EFFORTS == {"auto", "off", "low", "medium", "high"}


class TestReasoningCommand:
    @pytest.fixture
    def state(self):
        return ChatState(skills_dir=Path("skills"))

    def test_show_current(self, state) -> None:
        result = _run(handle_slash_command("/reasoning", state))
        assert "reasoning_effort=auto" in result
        assert "high" in result

    def test_set_high(self, state) -> None:
        result = _run(handle_slash_command("/reasoning high", state))
        assert "reasoning_effort=high" in result
        assert state.reasoning_effort == "high"

    def test_set_low(self, state) -> None:
        _run(handle_slash_command("/reasoning low", state))
        assert state.reasoning_effort == "low"

    def test_invalid_rejected(self, state) -> None:
        result = _run(handle_slash_command("/reasoning turbo", state))
        assert "unknown reasoning effort" in result
        assert state.reasoning_effort == "auto"


class TestPermissionsCommand:
    @pytest.fixture
    def state(self):
        return ChatState(skills_dir=Path("skills"))

    def test_show_current(self, state) -> None:
        result = _run(handle_slash_command("/permissions", state))
        assert "permission_posture=sandboxed" in result

    def test_set_full_access(self, state) -> None:
        result = _run(handle_slash_command("/permissions full-access", state))
        assert "permission_posture=full-access" in result
        assert state.permission_posture == "full-access"

    def test_set_off(self, state) -> None:
        _run(handle_slash_command("/permissions off", state))
        assert state.permission_posture == "off"

    def test_invalid_rejected(self, state) -> None:
        result = _run(handle_slash_command("/permissions everything", state))
        assert "unknown permission posture" in result
        assert state.permission_posture == "sandboxed"

    def test_valid_postures(self) -> None:
        assert VALID_PERMISSION_POSTURES == {"full-access", "restricted", "sandboxed", "off"}


class TestPostureMapping:
    def test_default_posture_is_sandboxed(self) -> None:
        assert current_posture() == "sandboxed"

    def test_full_access_allows_all(self) -> None:
        set_posture("full-access")
        assert posture_allows_tool("full-access", "execute_python") == (True, "")
        assert posture_allows_tool("full-access", "web_search") == (True, "")

    def test_sandboxed_allows_all(self) -> None:
        set_posture("sandboxed")
        assert posture_allows_tool("sandboxed", "execute_python") == (True, "")
        assert posture_allows_tool("sandboxed", "write_file") == (True, "")

    def test_restricted_blocks_network_and_code(self) -> None:
        for tool in ("execute_python", "web_search", "web_fetch"):
            allowed, reason = posture_allows_tool("restricted", tool)
            assert allowed is False
            assert "blocked" in reason
        for tool in ("read_file", "write_file", "list_files", "get_time"):
            assert posture_allows_tool("restricted", tool) == (True, "")

    def test_off_blocks_everything(self) -> None:
        allowed, reason = posture_allows_tool("off", "read_file")
        assert allowed is False
        assert "disabled" in reason

    def test_invalid_posture_rejected(self) -> None:
        with pytest.raises(ValueError):
            set_posture("bogus")
