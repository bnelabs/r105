"""Tests for fuzzy matching and command palette logic."""

from __future__ import annotations

from rova.tui.widgets.command_palette import (
    COMMAND_DEFS,
    CommandPalette,
    _fuzzy_score,
)


class TestFuzzyScore:
    """Tests for the fuzzy matching algorithm."""

    def test_empty_query(self):
        assert _fuzzy_score("anything", "") == 0

    def test_exact_prefix(self):
        score = _fuzzy_score("/help", "/h")
        assert score >= 1000  # Prefix bonus

    def test_substring_match(self):
        score = _fuzzy_score("/health", "heal")
        assert score > 0

    def test_contiguous_bonus(self):
        contiguous = _fuzzy_score("/history", "his")
        scattered = _fuzzy_score("/history", "hsy")
        assert contiguous > scattered

    def test_no_match(self):
        score = _fuzzy_score("/help", "xyz")
        assert score == 0

    def test_case_insensitive(self):
        lower = _fuzzy_score("/HELP", "/h")
        upper = _fuzzy_score("/help", "/H")
        assert lower == upper
        assert lower > 0

    def test_ordered_chars(self):
        """Characters must appear in order."""
        score = _fuzzy_score("/profile", "pof")
        assert score > 0  # p, o, f appear in order
        score = _fuzzy_score("/profile", "fop")
        assert score == 0  # f before o in query, but o before f in candidate


class TestCommandPalette:
    """Tests for CommandPalette widget logic."""

    def test_show_all_commands(self):
        palette = CommandPalette()
        palette.show_commands("/")
        assert palette.item_count == len(COMMAND_DEFS)
        assert palette.is_visible

    def test_filter_commands(self):
        palette = CommandPalette()
        palette.show_commands("/h")
        # /help, /health, /history should match
        assert palette.item_count > 0
        for item in palette._items:
            cmd = item[1]
            assert "h" in cmd.lower()

    def test_no_match(self):
        palette = CommandPalette()
        palette.show_commands("/zzz_nonexistent")
        assert palette.item_count == 0
        assert palette.is_visible

    def test_hide(self):
        palette = CommandPalette()
        palette.show_commands("/")
        palette.hide()
        assert not palette.is_visible

    def test_select_next_wraps(self):
        palette = CommandPalette()
        palette.show_commands("/")
        count = palette.item_count
        for _ in range(count + 2):
            palette.select_next()
        # After wrapping, should be valid
        assert 0 <= palette.selected_index < count

    def test_select_prev_wraps(self):
        palette = CommandPalette()
        palette.show_commands("/")
        palette.select_prev()
        # Should wrap to last item
        assert palette.selected_index == palette.item_count - 1

    def test_get_selected_returns_tuple(self):
        palette = CommandPalette()
        palette.show_commands("/h")
        selected = palette.get_selected()
        assert selected is not None
        assert len(selected) == 4  # (category, cmd, usage, desc)

    def test_get_selected_command(self):
        palette = CommandPalette()
        palette.show_commands("/help")
        cmd = palette.get_selected_command()
        assert cmd is not None
        assert cmd.startswith("/help")

    def test_initial_selection_is_first(self):
        palette = CommandPalette()
        palette.show_commands("/")
        assert palette.selected_index == 0

    def test_category_in_commands(self):
        """All COMMAND_DEFS entries now have a category."""
        for entry in COMMAND_DEFS:
            assert len(entry) == 4
            category, cmd, usage, desc = entry
            assert category in ("Chat", "RAG", "Skills", "Workspace", "System")
            assert cmd.startswith("/")
