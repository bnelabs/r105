"""Tests for session diffing (what changed since the session was saved)."""

from __future__ import annotations

import pytest

import r105.sessions as sessions
from r105.sessions import diff_session, load_session, save_session
from r105.state import ChatState


@pytest.fixture(autouse=True)
def _isolated_session_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(sessions, "SESSION_DIR", tmp_path / "sessions")


def _state_with_messages(n: int = 2) -> ChatState:
    state = ChatState()
    for i in range(n):
        state.history.append({"role": "user", "content": f"question {i}"})
        state.history.append({"role": "assistant", "content": f"answer {i}"})
    return state


def test_diff_no_change() -> None:
    state = _state_with_messages()
    save_session(state, "s1")
    summary = diff_session(state, "s1")
    assert "no change" in summary


def test_diff_unsaved_messages() -> None:
    state = _state_with_messages()
    save_session(state, "s1")
    state.history.append({"role": "user", "content": "a brand new question"})
    summary = diff_session(state, "s1")
    assert "+1 unsaved" in summary
    assert "a brand new question" in summary


def test_diff_settings_change() -> None:
    state = _state_with_messages()
    save_session(state, "s1")
    state.profile = "coding"
    summary = diff_session(state, "s1")
    assert "profile" in summary
    assert "coding" in summary


def test_diff_missing_session() -> None:
    with pytest.raises(FileNotFoundError):
        diff_session(ChatState(), "does-not-exist")


def test_load_still_works_after_diff() -> None:
    state = _state_with_messages()
    save_session(state, "s1")
    fresh = ChatState()
    assert load_session(fresh, "s1") == 4
    assert len(fresh.history) == 4
