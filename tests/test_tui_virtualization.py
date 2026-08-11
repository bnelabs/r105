"""Tests for transcript virtualization and interactive thinking panels.

Virtualization: the ChatView keeps the full transcript as lightweight records
and materializes only the viewport window (+ overscan) as widgets, so long
sessions stay bounded in widget count without ever truncating history.

Thinking panels: captured ``<|channel|>thought ... <channel|>`` blocks render
as interactive collapsible panels — folded by default, expandable/foldable via
``ThinkingPanel.toggle()`` (click or ``t``/``enter``/``space``).
"""

from __future__ import annotations

import asyncio

from textual.app import App

from r105.tui.widgets.chat_view import ChatView, ThinkingPanel

# Overscan (24 rows) on both sides of a 24-row viewport; each message panel is
# a few rows tall, so a sane bound is comfortably below the full 400 records.
_MAX_EXPECTED_WINDOW = 60


class _ChatShell(App[None]):
    """Minimal app hosting a ChatView."""

    def __init__(self, view: ChatView) -> None:
        super().__init__()
        self.view = view

    def compose(self):
        yield self.view


def _mounted_count(view: ChatView) -> int:
    return sum(1 for m in view._messages if m.widget is not None and m.widget.is_mounted)


def _fill(view: ChatView, pairs: int = 200) -> None:
    for i in range(pairs):
        view.add_user(f"user message number {i}")
        view.add_assistant(f"assistant reply number {i} — " + "content " * 20)


class TestTranscriptVirtualization:
    """Only the viewport window (+ overscan) is materialized as widgets."""

    def test_bounded_widget_window(self) -> None:
        async def scenario() -> None:
            view = ChatView()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                _fill(view)
                await pilot.pause()
                await pilot.pause(0.1)
                assert len(view._messages) == 400
                mounted = _mounted_count(view)
                assert 0 < mounted <= _MAX_EXPECTED_WINDOW, (
                    f"expected a bounded window, got {mounted}"
                )
                assert len(view._window) == mounted

        asyncio.run(scenario())

    def test_scroll_moves_window_and_preserves_transcript(self) -> None:
        async def scenario() -> None:
            view = ChatView()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                _fill(view)
                await pilot.pause(0.1)
                tail = view._window[-1]  # last record while pinned at the bottom
                view.scroll_to(y=0, animate=False, immediate=True)
                await pilot.pause(0.1)
                assert view._window[0] == 0, "top of transcript must be materialized"
                assert 0 < _mounted_count(view) <= _MAX_EXPECTED_WINDOW
                # The full transcript survives scrolling — records are never dropped.
                assert len(view._messages) == 400
                view.scroll_end(animate=False)
                await pilot.pause(0.1)
                assert view._window[-1] == tail, "scrolling back down reaches the end"

        asyncio.run(scenario())

    def test_auto_follow_only_at_bottom(self) -> None:
        async def scenario() -> None:
            view = ChatView()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                _fill(view, 50)
                await pilot.pause(0.1)
                # Scrolled up: a new message must NOT yank the view.
                view.scroll_to(y=0, animate=False, immediate=True)
                await pilot.pause(0.05)
                view.add_user("while scrolled up")
                await pilot.pause(0.1)
                assert view.scroll_y == 0, "no auto-follow while scrolled up"
                # Pinned at the bottom: new messages follow the stream.
                view.scroll_end(animate=False)
                await pilot.pause(0.05)
                before = view.scroll_y
                view.add_assistant("followed reply")
                await pilot.pause(0.1)
                assert view.scroll_y >= before, "auto-follow at the bottom"

        asyncio.run(scenario())

    def test_spacer_heights_cover_transcript(self) -> None:
        async def scenario() -> None:
            view = ChatView()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                _fill(view, 40)
                await pilot.pause(0.1)
                view.scroll_to(y=100, animate=False, immediate=True)
                await pilot.pause(0.1)
                top = int(view._top_spacer.styles.height.value)
                bottom = int(view._bottom_spacer.styles.height.value)
                window = sum(m.height for m in view._messages if m.widget is not None)
                total = sum(m.height for m in view._messages)
                assert top + window + bottom == total, "spacers must span the transcript"

        asyncio.run(scenario())


class TestThinkingPanelCollapsible:
    """Thinking panels are interactive: folded by default, toggleable."""

    @staticmethod
    def _view(**kwargs: object) -> ChatView:
        return ChatView(gemma4_channel_syntax=True, **kwargs)

    def test_collapsed_by_default(self) -> None:
        async def scenario() -> None:
            view = self._view()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                view.add_assistant("a <|channel|>thought secret<channel|> b")
                await pilot.pause(0.1)
                tmsg = next(m for m in view._messages if m.kind == "thinking")
                assert tmsg.expanded is False
                assert isinstance(tmsg.widget, ThinkingPanel)

        asyncio.run(scenario())

    def test_toggle_expands_and_folds(self) -> None:
        async def scenario() -> None:
            view = self._view()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                view.add_assistant("a <|channel|>thought secret<channel|> b")
                await pilot.pause(0.1)
                tmsg = next(m for m in view._messages if m.kind == "thinking")
                collapsed_height = tmsg.height
                tmsg.widget.toggle()
                await pilot.pause(0.1)
                assert tmsg.expanded is True
                assert tmsg.height != collapsed_height, "expanded panel re-measured"
                tmsg.widget.toggle()
                await pilot.pause(0.1)
                assert tmsg.expanded is False
                assert tmsg.height == collapsed_height

        asyncio.run(scenario())

    def test_thinking_default_expanded_setting(self) -> None:
        async def scenario() -> None:
            view = self._view(thinking_default_expanded=True)
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                view.add_assistant("a <|channel|>thought secret<channel|> b")
                await pilot.pause(0.1)
                tmsg = next(m for m in view._messages if m.kind == "thinking")
                assert tmsg.expanded is True

        asyncio.run(scenario())

    def test_toggle_state_survives_scroll_away(self) -> None:
        """The expanded flag lives in the record, not the (unmounted) widget."""

        async def scenario() -> None:
            view = self._view()
            app = _ChatShell(view)
            async with app.run_test(size=(80, 24)) as pilot:
                _fill(view, 50)
                view.add_assistant("a <|channel|>thought secret<channel|> b")
                await pilot.pause(0.1)
                view.scroll_to(y=0, animate=False, immediate=True)
                await pilot.pause(0.1)
                tmsg = next(m for m in view._messages if m.kind == "thinking")
                assert not (tmsg.widget is not None and tmsg.widget.is_mounted)
                # Expand while the panel is unmounted (off-screen).
                tmsg.expanded = True
                tmsg.height = None
                view._schedule_sync()
                await pilot.pause(0.1)
                view.scroll_end(animate=False)
                await pilot.pause(0.1)
                tmsg = next(m for m in view._messages if m.kind == "thinking")
                assert tmsg.expanded is True

        asyncio.run(scenario())
