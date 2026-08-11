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
    """Thinking blocks are captured and rendered as a panel, not lost.

    Capture only applies when ``gemma4_channel_syntax`` is enabled (Gemma-4
    family models). The default is OFF — model-agnostic passthrough.
    """

    def test_blocks_split_across_chunks_are_captured(self) -> None:
        view = ChatView(
            show_thinking=True,
            thinking_default_expanded=False,
            gemma4_channel_syntax=True,
        )
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
        view = ChatView(show_thinking=True, gemma4_channel_syntax=True)
        view.start_streaming()
        view.stream_chunk("start <|channel|>thought")
        assert view._pending_thinking == "<|channel|>thought"
        view.finish_streaming()
        # Unterminated block discarded at stream end
        assert view._pending_thinking == ""

    def test_complete_block_single_chunk(self) -> None:
        view = ChatView(show_thinking=True, gemma4_channel_syntax=True)
        view.add_assistant("result <|channel|>thought reasoning<channel|> visible")
        assert view._pending_thinking == ""

    def test_show_thinking_false_strips(self) -> None:
        view = ChatView(show_thinking=False, gemma4_channel_syntax=True)
        view.add_assistant("result <|channel|>thought hidden<channel|> visible")
        # No crash; pending is empty
        assert view._pending_thinking == ""

    def test_stray_tokens_stripped_from_user_text(self) -> None:
        view = ChatView(gemma4_channel_syntax=True)
        view.add_user("<|tool_call|>nope")
        assert view._pending_thinking == ""

    def test_defaults(self) -> None:
        view = ChatView()
        assert view.show_thinking is True
        assert view.thinking_default_expanded is False
        # Model-agnostic safety: channel-syntax handling is OFF by default
        assert view.gemma4_channel_syntax is False


class TestModelAgnosticPassthrough:
    """Non-Gemma-4 models: content is opaque text, never rewritten.

    This is the model-agnostic guarantee: with ``gemma4_channel_syntax``
    off (the default for every non-Gemma-4 model), markers such as
    ``<|tool_call|>`` and ``<|channel|>thought`` pass through verbatim —
    no capture, no stripping, no interpretation.
    """

    def test_streamed_content_passes_through_verbatim(self) -> None:
        view = ChatView()  # default: gemma4_channel_syntax=False
        view.start_streaming()
        raw = 'answer <|tool_call|>{"name":"x","arguments":{}}<|tool_result|> <|channel|>thought hi<channel|>'
        view.stream_chunk(raw)
        assert view._stream_buffer == raw
        assert view._thinking_parts == []

    def test_add_assistant_passes_through_verbatim(self) -> None:
        view = ChatView()
        raw = 'result <|tool_call|>{"name":"x"}<|tool_result|> done'
        view.add_assistant(raw)
        assert view._pending_thinking == ""

    def test_add_user_passes_through_verbatim(self) -> None:
        view = ChatView()
        raw = "<|tool_call|>not stripped for opaque models"
        view.add_user(raw)
        assert view._pending_thinking == ""

    def test_capability_is_per_instance(self) -> None:
        opaque = ChatView()                       # default off
        gemma = ChatView(gemma4_channel_syntax=True)
        assert opaque.gemma4_channel_syntax is False
        assert gemma.gemma4_channel_syntax is True


class TestStartupFocus:
    """ChatInput must hold focus on mount (Textual 8 RichLog-focus regression).

    Without the explicit focus in ChatScreen.on_mount, ChatView (a RichLog)
    steals startup focus and the TUI silently drops printable keyboard input
    until the user clicks or tabs — only app-level bindings (Ctrl+Q) work.
    """

    @staticmethod
    def _make_app():
        from pathlib import Path

        from r105.client import DirectClient
        from r105.state import ChatState
        from r105.tui.app import R105App

        client = DirectClient(base_url="http://127.0.0.1:8090", timeout=5.0)
        return R105App(client, ChatState(model="test-model"), Path("/tmp"))

    def test_chat_input_has_focus_on_mount(self) -> None:
        from r105.tui.widgets.input_area import ChatInput

        async def scenario() -> None:
            app = self._make_app()
            async with app.run_test(size=(80, 24)) as pilot:
                await pilot.pause()
                assert isinstance(app.focused, ChatInput), (
                    f"expected ChatInput to have startup focus, got {app.focused!r}"
                )

        asyncio.run(scenario())

    def test_typing_lands_in_input(self) -> None:
        from r105.tui.screens.chat import ChatScreen
        from r105.tui.widgets.input_area import ChatInput

        async def scenario() -> None:
            app = self._make_app()
            async with app.run_test(size=(80, 24)) as pilot:
                # Wait for the ChatScreen push (app on_mount) to complete
                for _ in range(50):
                    await pilot.pause()
                    if isinstance(app.screen, ChatScreen):
                        break
                assert isinstance(app.screen, ChatScreen)
                input_widget = app.screen.query_one("#chat-input", ChatInput)
                await pilot.press("h", "e", "l", "l", "o")
                await pilot.pause()
                assert input_widget.text == "hello"

        asyncio.run(scenario())


class TestEditKeybindings:
    """Readline edit bindings (Ctrl+U/W/K) must not crash on Textual 8.

    Regression: action_clear_line / action_delete_word_backward /
    action_kill_to_end called ``self.document.replace(...)``, an API that
    does not exist on Textual 8's Document (it is ``replace_range``), so any
    of the three keys crashed the TUI into the error screen whenever the
    input held text.
    """

    @staticmethod
    def _make_app():
        from pathlib import Path

        from r105.client import DirectClient
        from r105.state import ChatState
        from r105.tui.app import R105App

        client = DirectClient(base_url="http://127.0.0.1:8090", timeout=5.0)
        return R105App(client, ChatState(model="test-model"), Path("/tmp"))

    def test_ctrl_u_clears_line_without_crash(self) -> None:
        from r105.tui.screens.chat import ChatScreen
        from r105.tui.widgets.input_area import ChatInput

        async def scenario() -> None:
            app = self._make_app()
            async with app.run_test(size=(80, 24)) as pilot:
                for _ in range(50):
                    await pilot.pause()
                    if isinstance(app.screen, ChatScreen):
                        break
                input_widget = app.screen.query_one("#chat-input", ChatInput)
                await pilot.press("h", "e", "l", "l", "o")
                await pilot.pause()
                assert input_widget.text == "hello"
                await pilot.press("ctrl+u")
                await pilot.pause()
                assert input_widget.text == ""

        asyncio.run(scenario())

    def test_ctrl_w_deletes_word_backward_without_crash(self) -> None:
        from r105.tui.screens.chat import ChatScreen
        from r105.tui.widgets.input_area import ChatInput

        async def scenario() -> None:
            app = self._make_app()
            async with app.run_test(size=(80, 24)) as pilot:
                for _ in range(50):
                    await pilot.pause()
                    if isinstance(app.screen, ChatScreen):
                        break
                input_widget = app.screen.query_one("#chat-input", ChatInput)
                for ch in "one two three":
                    await pilot.press(ch)
                await pilot.pause()
                await pilot.press("ctrl+w")
                await pilot.pause()
                # last word ("three") deleted, trailing space trimmed by the action
                assert input_widget.text == "one two "

        asyncio.run(scenario())

    def test_ctrl_k_kills_to_end_without_crash(self) -> None:
        from r105.tui.screens.chat import ChatScreen
        from r105.tui.widgets.input_area import ChatInput

        async def scenario() -> None:
            app = self._make_app()
            async with app.run_test(size=(80, 24)) as pilot:
                for _ in range(50):
                    await pilot.pause()
                    if isinstance(app.screen, ChatScreen):
                        break
                input_widget = app.screen.query_one("#chat-input", ChatInput)
                for ch in "hello":
                    await pilot.press(ch)
                await pilot.pause()
                await pilot.press("ctrl+a")   # cursor to line start
                await pilot.pause()
                await pilot.press("ctrl+k")   # kill to end of line
                await pilot.pause()
                assert input_widget.text == ""

        asyncio.run(scenario())
