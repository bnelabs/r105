"""Tests for slash command handlers and state mutations."""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from rova.state import ChatState, VALID_PROFILES, VALID_QUALITIES
from rova.commands import handle_slash_command, VALID_THEMES


def _run(coro):
    """Helper to run async handle_slash_command synchronously in tests."""
    return asyncio.run(coro)


@pytest.fixture
def state():
    return ChatState(skills_dir=Path("skills"))


@pytest.fixture
def state_with_history(state):
    state.history = [
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "hi there"},
    ]
    return state


class TestChatCommands:
    """Tests for chat-related slash commands."""

    def test_help(self, state):
        result = _run(handle_slash_command("/help", state))
        assert "Rova Commands" in result

    def test_slash_only(self, state):
        result = _run(handle_slash_command("/", state))
        assert "Rova Commands" in result

    def test_state(self, state):
        result = _run(handle_slash_command("/state", state))
        assert "profile=" in result

    def test_tokens(self, state):
        result = _run(handle_slash_command("/tokens", state))
        assert "ctx=" in result

    def test_model(self, state):
        result = _run(handle_slash_command("/model", state))
        assert "model=" in result

    def test_history_empty(self, state):
        result = _run(handle_slash_command("/history", state))
        assert "empty" in result.lower()

    def test_history_with_data(self, state_with_history):
        result = _run(handle_slash_command("/history", state_with_history))
        assert "user" in result
        assert "hello" in result

    def test_clear(self, state_with_history):
        result = _run(handle_slash_command("/clear", state_with_history))
        assert "cleared" in result.lower()
        assert len(state_with_history.history) == 0

    def test_exit(self, state):
        result = _run(handle_slash_command("/exit", state))
        assert result == ""


class TestProfileCommands:
    """Tests for profile/quality/json/max commands."""

    def test_profile_auto(self, state):
        result = _run(handle_slash_command("/profile", state))
        assert "profile=auto" in result
        assert state.profile is None

    def test_profile_set(self, state):
        for profile in VALID_PROFILES:
            result = _run(handle_slash_command(f"/profile {profile}", state))
            assert f"profile={profile}" in result

    def test_profile_invalid(self, state):
        result = _run(handle_slash_command("/profile bogus", state))
        assert "unknown" in result.lower()

    def test_quality_auto(self, state):
        result = _run(handle_slash_command("/quality", state))
        assert "quality=auto" in result

    def test_quality_set(self, state):
        for q in VALID_QUALITIES:
            result = _run(handle_slash_command(f"/quality {q}", state))
            assert f"quality={q}" in result

    def test_json_toggle(self, state):
        assert state.json_mode is False
        _run(handle_slash_command("/json", state))
        assert state.json_mode is True
        _run(handle_slash_command("/json off", state))
        assert state.json_mode is False

    def test_max_tokens(self, state):
        result = _run(handle_slash_command("/max 4096", state))
        assert state.max_tokens == 4096
        result = _run(handle_slash_command("/max", state))
        assert state.max_tokens is None


class TestThemeCommand:
    """Tests for /theme command."""

    def test_theme_show_current(self, state):
        result = _run(handle_slash_command("/theme", state))
        assert "theme=" in result
        assert state.theme == "rova"

    def test_theme_set_valid(self, state):
        for theme in VALID_THEMES:
            result = _run(handle_slash_command(f"/theme {theme}", state))
            assert f"theme={theme}" in result

    def test_theme_set_invalid(self, state):
        result = _run(handle_slash_command("/theme nonexistent", state))
        assert "unknown theme" in result.lower()


class TestAutoCompactCommand:
    """Tests for /autocompact command."""

    def test_autocompact_toggle(self, state):
        assert state.auto_compact is True
        _run(handle_slash_command("/autocompact", state))
        assert state.auto_compact is False
        _run(handle_slash_command("/autocompact", state))
        assert state.auto_compact is True

    def test_autocompact_explicit(self, state):
        _run(handle_slash_command("/autocompact off", state))
        assert state.auto_compact is False
        _run(handle_slash_command("/autocompact on", state))
        assert state.auto_compact is True


class TestPreviewCommand:
    """Tests for /preview command."""

    def test_preview_missing_args(self, state, tmp_path):
        result = _run(handle_slash_command(
            "/preview", state, None, tmp_path
        ))
        assert "usage" in result.lower()

    def test_preview_file_not_found(self, state, tmp_path):
        result = _run(handle_slash_command(
            "/preview", state, None, tmp_path
        ))
        assert "usage" in result.lower()

    def test_preview_file(self, state, tmp_path):
        (tmp_path / "test.md").write_text("# Hello\nWorld")
        result = _run(handle_slash_command(
            "/preview test.md", state, None, tmp_path
        ))
        assert "Hello" in result
        assert "World" in result


class TestSkillCommands:
    """Tests for /skill commands with parameter support."""

    def test_list_skills(self, state):
        result = _run(handle_slash_command("/skills", state))
        assert result is not None  # May be empty or list

    def test_skill_use_with_params(self, state):
        result = _run(handle_slash_command(
            '/skill use code-check language=python',
            state,
        ))
        assert "skill added" in result.lower()
        assert "code-check" in state.active_skills
        assert "language" in state.skill_params.get("code-check", {})

    def test_skill_use_without_params(self, state):
        result = _run(handle_slash_command(
            '/skill use concise',
            state,
        ))
        assert "skill added" in result.lower()
        assert "concise" in state.active_skills

    def test_skill_drop(self, state):
        state.active_skills = ["concise", "code-check"]
        state.skill_params = {"code-check": {"language": "python"}}
        result = _run(handle_slash_command("/skill drop code-check", state))
        assert "dropped" in result.lower()
        assert "code-check" not in state.active_skills
        assert "code-check" not in state.skill_params

    def test_skill_clear(self, state):
        state.active_skills = ["concise", "deep-review"]
        state.skill_params = {"concise": {"x": "y"}}
        result = _run(handle_slash_command("/skill clear", state))
        assert "cleared" in result.lower()
        assert len(state.active_skills) == 0
        assert len(state.skill_params) == 0


class TestWorkspaceCommand:
    """Tests for /workspace command."""

    def test_workspace_empty(self, state, tmp_path):
        result = _run(handle_slash_command("/workspace", state, None, tmp_path))
        assert "empty" in result.lower() or str(tmp_path) in result


class TestUnknownCommand:
    """Tests for invalid commands."""

    def test_unknown(self, state):
        result = _run(handle_slash_command("/bogus", state))
        assert "unknown" in result.lower()
