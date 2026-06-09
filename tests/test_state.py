"""Tests for ChatState, TokenUsage, and token estimation."""

from __future__ import annotations

from pathlib import Path

from rova.state import (
    DEFAULT_CONTEXT_TOKENS,
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
    TokenUsage,
    estimate_tokens,
    token_usage,
)


class TestChatState:
    """Tests for ChatState defaults and fields."""

    def test_defaults(self):
        state = ChatState()
        assert state.profile is None
        assert state.rag is None
        assert state.quality is None
        assert state.max_tokens is None
        assert state.json_mode is False
        assert state.auto_compact is True
        assert state.theme == "rova"
        assert state.active_skills == []
        assert state.skill_params == {}
        assert state.context_tokens == DEFAULT_CONTEXT_TOKENS
        assert state.history == []

    def test_custom_values(self):
        state = ChatState(
            profile="coding",
            rag=True,
            quality="best",
            max_tokens=4096,
            json_mode=True,
            auto_compact=False,
            theme="dracula",
            active_skills=["concise"],
            skill_params={"concise": {"key": "val"}},
        )
        assert state.profile == "coding"
        assert state.rag is True
        assert state.quality == "best"
        assert state.max_tokens == 4096
        assert state.json_mode is True
        assert state.auto_compact is False
        assert state.theme == "dracula"
        assert "concise" in state.active_skills
        assert state.skill_params == {"concise": {"key": "val"}}

    def test_history_is_mutable(self):
        state = ChatState()
        state.history.append({"role": "user", "content": "hi"})
        assert len(state.history) == 1


class TestTokenUsage:
    """Tests for TokenUsage dataclass."""

    def test_percent_zero_context(self):
        usage = TokenUsage(used_tokens=100, context_tokens=0)
        assert usage.percent == 0.0

    def test_percent_normal(self):
        usage = TokenUsage(used_tokens=500, context_tokens=1000)
        assert usage.percent == 50.0

    def test_percent_capped(self):
        usage = TokenUsage(used_tokens=2000, context_tokens=1000)
        assert usage.percent == 100.0

    def test_percent_negative_context(self):
        usage = TokenUsage(used_tokens=100, context_tokens=-1)
        assert usage.percent == 0.0


class TestEstimateTokens:
    """Tests for the simple token estimator."""

    def test_empty_string(self):
        assert estimate_tokens("") == 0

    def test_simple_text(self):
        tokens = estimate_tokens("hello world")
        assert tokens > 0

    def test_code_text(self):
        tokens = estimate_tokens("def foo(x): return x + 1")
        assert tokens > 0

    def test_punctuation(self):
        # Should count punctuation as separate tokens
        tokens = estimate_tokens("a, b; c: d.")
        assert tokens > 0


class TestTokenUsageWithHistory:
    """Tests for token_usage() with history."""

    def test_empty_state(self):
        state = ChatState()
        usage = token_usage(state)
        assert usage.used_tokens >= 0
        assert usage.context_tokens == DEFAULT_CONTEXT_TOKENS

    def test_with_messages(self):
        state = ChatState()
        state.history = [
            {"role": "user", "content": "hello world"},
            {"role": "assistant", "content": "hi there"},
        ]
        usage = token_usage(state)
        assert usage.used_tokens > 0
        assert usage.percent > 0

    def test_with_skills(self, tmp_path):
        # Create a temp skill file
        skill_dir = Path(str(tmp_path))
        (skill_dir / "test-skill.md").write_text("Be concise and direct.")
        state = ChatState(
            skills_dir=skill_dir,
            active_skills=["test-skill"],
        )
        usage = token_usage(state)
        # Skills contribute to token count
        assert usage.used_tokens > 0


class TestValidConstants:
    """Tests for constant values."""

    def test_valid_profiles(self):
        assert "simple" in VALID_PROFILES
        assert "coding" in VALID_PROFILES
        assert "tool_agent" in VALID_PROFILES

    def test_valid_qualities(self):
        assert "fast" in VALID_QUALITIES
        assert "balanced" in VALID_QUALITIES
        assert "best" in VALID_QUALITIES
