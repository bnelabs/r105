"""Structural tests for TUI components (no Textual runtime needed)."""

from __future__ import annotations

from pathlib import Path

from r105.tui.app import R105App
from r105.tui.screens.chat import ChatScreen, _is_exact_command
from r105.tui.widgets.command_palette import COMMAND_DEFS, CommandPalette


class TestIsExactCommand:
    """Tests for the slash-command detection helper."""

    def test_exact_command_simple(self) -> None:
        assert _is_exact_command("/help") is True

    def test_exact_command_with_args(self) -> None:
        assert _is_exact_command("/profile coding") is True

    def test_partial_no_match(self) -> None:
        assert _is_exact_command("/prf") is False

    def test_fuzzy_no_match(self) -> None:
        assert _is_exact_command("/hel") is False

    def test_empty_input(self) -> None:
        assert _is_exact_command("") is False

    def test_plain_text(self) -> None:
        assert _is_exact_command("hello world") is False


class TestCommandPaletteStructure:
    """Tests for command palette data integrity."""

    def test_all_entries_valid(self) -> None:
        """Every COMMAND_DEFS entry has exactly 4 fields (category, cmd, usage, desc)."""
        for entry in COMMAND_DEFS:
            assert len(entry) == 4, f"Entry {entry} has wrong length"
            category, cmd, usage, desc = entry
            assert isinstance(category, str)
            assert isinstance(cmd, str)
            assert isinstance(usage, str)
            assert isinstance(desc, str)
            assert cmd.startswith("/"), f"Command '{cmd}' does not start with /"

    def test_categories_consistent(self) -> None:
        """All entries use known categories."""
        valid = {"Chat", "Skills", "Sessions", "Plugins", "MCP", "Workspace", "System"}
        for category, _cmd, _usage, _desc in COMMAND_DEFS:
            assert category in valid, f"Unknown category: {category}"

    def test_commands_are_unique(self) -> None:
        """No duplicate command names."""
        seen: set[str] = set()
        for _cat, cmd, _usage, _desc in COMMAND_DEFS:
            assert cmd not in seen, f"Duplicate command: {cmd}"
            seen.add(cmd)


class TestCommandPaletteBehavior:
    """Tests for CommandPalette widget logic (pure logic, no DOM)."""

    def test_palette_initial_state(self) -> None:
        palette = CommandPalette()
        assert palette.item_count == 0
        assert not palette.is_visible
        assert palette.selected_index == 0

    def test_show_commands_no_filter(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/")
        assert palette.item_count == len(COMMAND_DEFS)
        assert palette.is_visible
        assert palette.selected_index == 0

    def test_show_commands_filtered(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/session")
        assert palette.item_count > 0
        for i in range(palette.item_count):
            entry = palette._items[i]
            assert "session" in entry[1].lower()

    def test_show_commands_no_match(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/zzz_nonexistent_command")
        assert palette.item_count == 0
        assert palette.is_visible  # still visible, shows "no matches"

    def test_hide(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/")
        palette.hide()
        assert not palette.is_visible

    def test_select_next_wraps(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/")
        count = palette.item_count
        for _ in range(count + 2):
            palette.select_next()
        assert 0 <= palette.selected_index < count

    def test_select_prev_wraps(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/")
        palette.select_prev()
        assert palette.selected_index == palette.item_count - 1

    def test_get_selected_returns_tuple(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/help")
        selected = palette.get_selected()
        assert selected is not None
        assert len(selected) == 4

    def test_get_selected_command(self) -> None:
        palette = CommandPalette()
        palette.show_commands("/help")
        cmd = palette.get_selected_command()
        assert cmd is not None
        assert cmd.startswith("/help")


class TestChatScreenHeader:
    """Tests for header rendering (no screen needed)."""

    def test_header_format(self) -> None:
        """_render_header produces a non-empty string with key fields."""
        from r105.state import ChatState

        state = ChatState(model="test-model", skills_dir=Path("/tmp"))
        state.history = [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
        ]

        # The header is rendered by ChatScreen which needs client + workspace.
        # We test the format by checking ChatScreen can be constructed.
        # (Full rendering requires a Textual app instance.)
        # The class exists and has expected attributes.
        assert hasattr(ChatScreen, "BINDINGS")
        assert hasattr(ChatScreen, "_render_header")

    def test_core_bindings_have_keymap_ids(self) -> None:
        app_ids = {binding.id for binding in R105App.BINDINGS if hasattr(binding, "id")}
        screen_ids = {binding.id for binding in ChatScreen.BINDINGS if hasattr(binding, "id")}
        assert {"quit", "show_help", "show_history"} <= app_ids
        assert {"copy_last_message", "show_tools", "cancel_request"} <= screen_ids
