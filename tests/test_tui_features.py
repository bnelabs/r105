"""Tests for TUI regressions and new UI features.

Covers the HelpScreen escape-dismiss fix and the ChatView
thinking capture-and-fold behavior.
"""

from __future__ import annotations

import asyncio

from textual.app import App
from textual.widgets import Static

from r105.tui.screens.help_screen import HelpScreen
from r105.tui.widgets.chat_view import ChatView


class _HelpShell(App[None]):
    """Minimal app that pushes the HelpScreen on mount."""

    def compose(self):
        yield Static("base screen")

    def on_mount(self) -> None:
        self.push_screen(HelpScreen("r105 Commands"))


class TestHelpScreenEscapeBinding:
    """The F1 help modal must be dismissible with Escape (v0.5.0 fix)."""

    def test_escape_binding_present(self) -> None:
        actions = {key: action for key, action, _desc in HelpScreen.BINDINGS}
        assert actions["escape"] == "dismiss"

    def test_q_binding_present(self) -> None:
        actions = {key: action for key, action, _desc in HelpScreen.BINDINGS}
        assert actions["q"] == "dismiss"

    def test_escape_dismisses_modal(self) -> None:
        async def scenario() -> None:
            app = _HelpShell()
            async with app.run_test(size=(80, 24)) as pilot:
                await pilot.pause()
                assert isinstance(app.screen, HelpScreen)
                await pilot.press("escape")
                await pilot.pause()
                assert not isinstance(app.screen, HelpScreen)

        asyncio.run(scenario())


class TestChatViewThinkingCapture:
    """Thinking blocks are captured and rendered as a panel, not lost."""

    def test_blocks_split_across_chunks_are_captured(self) -> None:
        view = ChatView(show_thinking=True, thinking_default_expanded=False)
        view.start_streaming()
        view.stream_chunk("answer. <|channel|>thought")
        view.stream_chunk("half of the reasoning ")
        view.stream_chunk("here<channel|> done")
        assert view._thinking_parts == ["half of the reasoning here"]
        assert view._stream_buffer == "answer.  done"
        # Display text must not contain thinking markers
        assert "<|channel|>" not in view._stream_buffer
        assert "<channel|>" not in view._stream_buffer

    def test_pending_held_across_chunks(self) -> None:
        view = ChatView(show_thinking=True)
        view.start_streaming()
        view.stream_chunk("start <|channel|>thought")
        assert view._pending_thinking == "<|channel|>thought"
        view.finish_streaming()
        # Unterminated block discarded at stream end
        assert view._pending_thinking == ""

    def test_complete_block_single_chunk(self) -> None:
        view = ChatView(show_thinking=True)
        view.add_assistant("result <|channel|>thought reasoning<channel|> visible")
        assert view._pending_thinking == ""

    def test_show_thinking_false_strips(self) -> None:
        view = ChatView(show_thinking=False)
        view.add_assistant("result <|channel|>thought hidden<channel|> visible")
        # No crash; pending is empty
        assert view._pending_thinking == ""

    def test_stray_tokens_stripped_from_user_text(self) -> None:
        view = ChatView()
        view.add_user("<|tool_call|>nope")
        assert view._pending_thinking == ""

    def test_defaults(self) -> None:
        view = ChatView()
        assert view.show_thinking is True
        assert view.thinking_default_expanded is False
