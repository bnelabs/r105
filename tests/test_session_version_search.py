"""Session format versioning/migration and full-text search."""

from __future__ import annotations

import json

import pytest

from r105.sessions import (
    SESSION_FORMAT_VERSION,
    SessionManager,
    list_sessions,
    load_session,
    migrate_session_data,
    save_session,
    search_sessions,
)
from r105.state import ChatState


@pytest.fixture()
def manager(tmp_path, monkeypatch):
    import r105.sessions as sessions

    monkeypatch.setattr(sessions, "SESSION_DIR", tmp_path / "sessions")
    m = SessionManager(tmp_path / "sessions")
    monkeypatch.setattr(sessions, "_session_path", m._session_path)
    monkeypatch.setattr(sessions, "_ensure_dir", m._ensure_dir)
    return m


def _state_with(*texts: str) -> ChatState:
    st = ChatState()
    for i, text in enumerate(texts):
        st.history.append({"role": "user" if i % 2 == 0 else "assistant", "content": text})
    return st


def test_save_stamps_format_version(manager, tmp_path):
    st = _state_with("hello")
    path = save_session(st, "v")
    data = json.loads(path.read_text())
    assert data["version"] == SESSION_FORMAT_VERSION


def test_legacy_unversioned_file_loads(manager):
    st = _state_with("legacy content here")
    path = save_session(st, "legacy")
    data = json.loads(path.read_text())
    del data["version"]  # simulate a pre-versioning file
    path.write_text(json.dumps(data))
    fresh = ChatState()
    assert load_session(fresh, "legacy") == 1
    assert fresh.history[0]["content"] == "legacy content here"


def test_cache_prompt_round_trips_with_session(manager):
    st = _state_with("cached context")
    st.cache_prompt = True
    st.model = "second-model"
    save_session(st, "cache")

    fresh = ChatState()
    load_session(fresh, "cache")
    assert fresh.cache_prompt is True
    assert fresh.model == "second-model"


def test_migrate_stamps_v0_to_v1():
    out = migrate_session_data({"history": [], "state": {}})
    assert out["version"] == 1


def test_migrate_rejects_newer_format():
    with pytest.raises(ValueError, match="newer r105"):
        migrate_session_data({"version": SESSION_FORMAT_VERSION + 1, "history": []})


def test_migrate_rejects_garbage():
    with pytest.raises(ValueError):
        migrate_session_data({"version": "one"})
    with pytest.raises(ValueError):
        migrate_session_data(["not", "a", "dict"])


def test_load_newer_format_surfaces_value_error(manager):
    st = _state_with("x")
    path = save_session(st, "future")
    data = json.loads(path.read_text())
    data["version"] = 999
    path.write_text(json.dumps(data))
    with pytest.raises(ValueError, match="newer r105"):
        load_session(ChatState(), "future")


def test_search_finds_content_case_insensitive(manager):
    save_session(_state_with("the quick brown fox", "nothing relevant"), "a")
    save_session(_state_with("something else entirely"), "b")
    hits = search_sessions("QUICK brown")
    assert [h["name"] for h in hits] == ["a"]
    assert hits[0]["matches"][0]["role"] == "user"
    assert "quick brown fox" in hits[0]["matches"][0]["snippet"]


def test_search_empty_query_returns_no_hits(manager):
    save_session(_state_with("hello"), "a")
    assert search_sessions("   ") == []


def test_search_skips_corrupt_files(manager, tmp_path):
    save_session(_state_with("findable needle here"), "good")
    (tmp_path / "sessions" / "broken.json").write_text("{not json")
    hits = search_sessions("needle")
    assert [h["name"] for h in hits] == ["good"]


def test_session_search_command(manager):
    from r105.commands import _handle_session_command

    save_session(_state_with("the hidden giraffe"), "zoo")
    out = _handle_session_command(["search", "giraffe"], ChatState())
    assert "zoo" in out and "giraffe" in out
    assert _handle_session_command(["search"], ChatState()) == "usage: /session search <query>"
    assert _handle_session_command(["search", "zzz-no-match"], ChatState()) == "no sessions match"
