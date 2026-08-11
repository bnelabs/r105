"""Virtualized chat transcript with interactive collapsible thinking panels.

The transcript is kept as lightweight message *records*; only the records
overlapping the viewport (plus an overscan window) are materialized as widgets,
so very long sessions stay fast and memory-bounded. Scrolling mounts/unmounts
widgets on demand — the full transcript is always preserved in the record list.

Thinking blocks (``<|channel|>thought ... <channel|>``) are captured and
rendered as an interactive collapsible ``THINKING`` panel: click the panel (or
press ``t`` / ``enter`` / ``space`` while it is focused) to expand or fold it.
``show_thinking`` controls whether thinking is rendered at all, and
``thinking_default_expanded`` controls the initial state of each panel.
"""

from __future__ import annotations

import re
import time
from dataclasses import dataclass, field
from typing import Any

from rich.console import Console
from rich.markdown import Markdown
from rich.panel import Panel
from rich.style import Style
from rich.text import Text
from textual.binding import Binding
from textual.containers import VerticalScroll
from textual.message import Message
from textual.widgets import Static

# Gemma 4 thinking blocks: <|channel|>thought ... <channel|> (or <channel|>thought opener)
_THINKING_BLOCK_RE = re.compile(r"<\|?channel\|?>thought(.*?)<channel\|?>", re.DOTALL)
# Stray Gemma 4 token artifacts (bare channel markers, tool-call/result markers)
_STRAY_TOKENS_RE = re.compile(r"<\|?(?:channel|tool_call|tool_result)\|?>")

# Hex-color dim style used for secondary text (theme-neutral).
_DIM = Style(dim=True, color="#6c7086")
_DIM_WARM = Style(dim=True, color="#f9e2af")


def _thinking_renderable(text: str, expanded: bool) -> Panel:
    """Render a thinking block as an interactive collapsible panel."""
    if expanded:
        return Panel(
            Text(text, style="dim"),
            title="💭 THINKING",
            border_style="cyan",
            padding=(0, 1),
        )
    preview = text if len(text) <= 300 else text[:300] + "…"
    content = Text()
    content.append(
        f"💭 {len(text)} chars of thinking — folded (click or press t to expand)",
        style="dim",
    )
    content.append("\n")
    content.append(preview, style=_DIM)
    return Panel(
        content,
        title="💭 THINKING",
        border_style="cyan",
        padding=(0, 1),
    )


def _renderable_height(renderable: Any, width: int) -> int:
    """Return the number of terminal lines *renderable* occupies at *width*."""
    if width < 1:
        width = 80
    console = Console(width=width, file=None)
    try:
        lines = console.render_lines(renderable)
    except Exception:
        # Never let a render error take down the TUI during measurement.
        lines = console.render_lines(Text(str(renderable)))
    return max(1, len(lines))


@dataclass
class _Message:
    """One transcript entry — pure data until materialized as a widget."""

    kind: str  # user | assistant | thinking | system | error | tool_call | tool_result | tool_status | source
    text: str = ""
    meta: dict[str, Any] = field(default_factory=dict)
    height: int | None = None  # cached measured height (at self.width)
    width: int | None = None  # width the height was measured at
    widget: Static | None = None  # materialized widget while in the viewport window
    expanded: bool | None = None  # thinking panels only


class ThinkingPanel(Static):
    """An interactive collapsible thinking panel.

    Clicking the panel toggles it; ``t`` / ``enter`` / ``space`` do the same
    when the panel is focused. A ``Toggled`` message is posted on every change
    so the owner can re-layout (the panel's height changes when expanded).
    """

    DEFAULT_CSS = """
    ThinkingPanel {
        height: auto;
    }
    """

    class Toggled(Message):
        """Posted when the panel is expanded or folded."""

        def __init__(self, panel: ThinkingPanel) -> None:
            self.panel = panel
            super().__init__()

    BINDINGS = [
        Binding("t", "toggle_thinking", "Toggle thinking panel", show=False),
        Binding("enter", "toggle_thinking", "Toggle thinking panel", show=False),
        Binding("space", "toggle_thinking", "Toggle thinking panel", show=False),
    ]

    def __init__(
        self,
        text: str,
        expanded: bool = False,
        *,
        name: str | None = None,
        id: str | None = None,
    ) -> None:
        super().__init__(content=_thinking_renderable(text, expanded), name=name, id=id)
        self.thinking_text = text
        self.expanded = expanded

    def action_toggle_thinking(self) -> None:
        self.toggle()

    def toggle(self) -> None:
        """Expand if folded, fold if expanded; re-render and notify owner."""
        self.expanded = not self.expanded
        self.update(_thinking_renderable(self.thinking_text, self.expanded))
        self.post_message(self.Toggled(self))
        self.refresh()

    def on_click(self, event: Any) -> None:
        self.toggle()


class ChatView(VerticalScroll):
    """A virtualized, scrollable chat transcript with rich formatting.

    Streaming is debounced: incoming tokens are buffered and the Markdown
    widget is updated every ~50ms, preventing CPU thrashing on long responses.

    Thinking blocks (``<|channel|>thought ... <channel|>``) are captured and
    rendered as interactive collapsible ``THINKING`` panels instead of being
    silently stripped — controlled by ``show_thinking`` (render at all) and
    ``thinking_default_expanded`` (initial state of each panel).

    Virtualization: the full transcript lives in ``_messages`` as records;
    ``_sync_visible`` materializes only the records overlapping the viewport
    (plus overscan) and unmounts the rest. Long sessions therefore hold a
    bounded number of widgets while the transcript itself is never truncated.
    """

    _DEBOUNCE_MS = 0.05  # 50ms
    _OVERSCAN_ROWS = 24  # extra rows rendered above/below the viewport

    DEFAULT_CSS = """
    ChatView {
        background: $surface;
    }
    """

    def __init__(
        self,
        show_thinking: bool = True,
        thinking_default_expanded: bool = False,
        gemma4_channel_syntax: bool = False,
        **kwargs,
    ) -> None:
        super().__init__(**kwargs)
        self.show_thinking = show_thinking
        self.thinking_default_expanded = thinking_default_expanded
        # Gemma-4 channel syntax (<|tool_call|>, <|channel|>thought, ...) is a
        # model-family capability. Default OFF: for any other model, content
        # is opaque text and is never regex-interpreted or rewritten.
        self.gemma4_channel_syntax = gemma4_channel_syntax
        self._streaming = False
        self._stream_buffer = ""
        self._thinking_parts: list[str] = []
        self._pending_thinking = ""
        self._received_content = False
        self._last_render = 0.0

        # Transcript data model (never truncated — this is the source of truth).
        self._messages: list[_Message] = []
        self._stream_record: _Message | None = None

        # Virtualization state.
        self._window: list[int] = []  # record indices currently materialized
        self._sync_pending = False
        self._last_width: int | None = None
        self._sticky_follow = True  # pinned to the end until the user scrolls up
        self._follow_pending = False
        self._top_spacer = Static("", classes="r105-spacer")
        self._bottom_spacer = Static("", classes="r105-spacer")

    # -- Public API (safe to call whether or not the widget is mounted) ------

    @property
    def received_content(self) -> bool:
        """True if any displayable content has been streamed this session."""
        return self._received_content

    def set_gemma4_channel_syntax(self, enabled: bool) -> None:
        """Update the channel-syntax capability (e.g. after ``/model``)."""
        self.gemma4_channel_syntax = enabled

    # -- Thinking capture ----------------------------------------------------

    def _extract_thinking(self, text: str) -> tuple[str, str]:
        """Split *text* into (displayable, thinking).

        Complete thinking blocks are removed from the display text and
        returned separately. A trailing unterminated block opener is held
        in ``_pending_thinking`` so blocks split across streaming chunks
        are captured correctly.
        """
        combined = self._pending_thinking + text
        display_parts: list[str] = []
        thinking_parts: list[str] = []
        last_end = 0
        for match in _THINKING_BLOCK_RE.finditer(combined):
            display_parts.append(combined[last_end : match.start()])
            thinking_parts.append(match.group(1))
            last_end = match.end()
        tail = combined[last_end:]
        # Find the *last* unterminated opener marker in the tail
        tail_openers = list(re.finditer(r"<\|?channel\|?>thought", tail))
        if tail_openers and "<channel|>" not in tail[tail_openers[-1].start() :]:
            # Unterminated block — hold everything from the opener marker
            start = tail_openers[-1].start()
            self._pending_thinking = tail[start:]
            display_parts.append(tail[:start])
            return "".join(display_parts), "".join(thinking_parts)
        self._pending_thinking = ""
        display_parts.append(tail)
        return "".join(display_parts), "".join(thinking_parts)

    def flush_thinking(self) -> None:
        """Materialize any captured thinking fragments and reset the accumulator."""
        if self._thinking_parts and self.show_thinking:
            text = _STRAY_TOKENS_RE.sub("", " ".join(self._thinking_parts)).strip()
            if text:
                self._append("thinking", text)
        self._thinking_parts = []
        # Discard an unterminated trailing block at stream end
        self._pending_thinking = ""

    # -- Message rendering (data layer — no DOM access) ----------------------

    def add_user(self, text: str) -> None:
        if self.gemma4_channel_syntax:
            text = _STRAY_TOKENS_RE.sub("", text).strip()
        self._append("user", text)

    def add_assistant(self, text: str, wall_seconds: float | None = None) -> None:
        if self.gemma4_channel_syntax:
            clean, thinking = self._extract_thinking(text)
            clean = _STRAY_TOKENS_RE.sub("", clean).strip()
        else:
            # Model-agnostic path: content is opaque text, never rewritten.
            clean, thinking = text, ""
        if thinking and self.show_thinking:
            thinking = _STRAY_TOKENS_RE.sub("", thinking).strip()
            if thinking:
                self._append("thinking", thinking)
        if not clean:
            self._append("assistant", "(empty response)", {"empty": True})
            return
        self._append("assistant", clean, {"wall": wall_seconds})

    def add_system(self, text: str) -> None:
        self._append("system", text)

    def add_error(self, text: str) -> None:
        self._append("error", text)

    def add_tool_call(self, name: str, args: str) -> None:
        self._append("tool_call", "", {"name": name, "args": args})

    def add_tool_result(self, result: str) -> None:
        self._append("tool_result", result)

    def add_tool_status(self, text: str) -> None:
        """Show an inline tool progress message (e.g. 'Executing Python...')."""
        self._append("tool_status", text)

    def add_source_attribution(self, source_tag: str, snippet: str = "") -> None:
        """Render a RAG source citation with optional snippet preview."""
        self._append("source", source_tag, {"snippet": snippet})

    def _append(self, kind: str, text: str, meta: dict[str, Any] | None = None) -> None:
        msg = _Message(kind=kind, text=text, meta=meta or {})
        if kind == "thinking":
            msg.expanded = self.thinking_default_expanded
        self._messages.append(msg)
        self._scroll_to_end_if_at_bottom()
        self._schedule_sync()

    # -- Debounced streaming -------------------------------------------------

    def start_streaming(self) -> None:
        """Begin a streaming assistant response."""
        self._streaming = True
        self._stream_buffer = ""
        self._received_content = False
        self._last_render = time.monotonic()

    def stream_chunk(self, text: str) -> None:
        """Accumulate a content delta and debounce the widget update.

        The Markdown widget is updated at most every 50ms, preventing
        UI thread saturation during fast SSE streams. Thinking blocks are
        captured separately and rendered on flush — only for models with
        Gemma-4 channel syntax; all other content is opaque text.
        """
        if self.gemma4_channel_syntax:
            clean, thinking = self._extract_thinking(text)
            if thinking:
                self._thinking_parts.append(thinking)
        else:
            clean, thinking = text, ""
        if not clean:
            return
        if not self._streaming:
            self.start_streaming()
        if self._stream_record is None:
            self._stream_record = _Message(kind="assistant", text="", meta={"streaming": True})
            self._messages.append(self._stream_record)
            self._schedule_sync()
        self._stream_record.text += clean
        self._stream_buffer += clean
        self._received_content = True

        now = time.monotonic()
        if now - self._last_render >= self._DEBOUNCE_MS:
            self._last_render = now
            self._refresh_stream()

    def finish_streaming(self) -> None:
        """Flush any remaining buffered text and end the streaming session."""
        if not self._streaming:
            return
        remaining = self._stream_buffer.strip()
        if self._stream_record is not None:
            if remaining:
                self._stream_record.text = remaining
                self._stream_record.meta["streaming"] = False
                self._stream_record.height = None
                self._refresh_stream()
            else:
                # Nothing displayable was streamed (e.g. reasoning-only reply);
                # drop the placeholder record — the caller renders the fallback.
                self._drop_stream_record()
        self._streaming = False
        self._stream_buffer = ""
        self._stream_record = None
        self.flush_thinking()
        self._scroll_to_end_if_at_bottom()

    def _refresh_stream(self) -> None:
        """Re-render the live streaming record (debounced by the caller)."""
        if self._stream_record is None or not self.is_mounted:
            return
        record = self._stream_record
        record.height = None  # re-measure on next sync
        if record.widget is not None and record.widget.is_mounted:
            record.widget.update(self._renderable(record))
        self._schedule_sync()
        self._scroll_to_end_if_at_bottom()

    def _drop_stream_record(self) -> None:
        record = self._stream_record
        if record is None or record not in self._messages:
            return
        index = self._messages.index(record)
        del self._messages[index]
        # Fix up the materialized-window bookkeeping.
        self._window = [i - 1 if i > index else i for i in self._window if i != index]
        if record.widget is not None:
            widget = record.widget
            record.widget = None
            if widget.is_mounted:
                widget.remove()  # fire-and-forget; next sync reconciles
        self._invalidate_heights()
        self._schedule_sync()

    # -- Rendering -----------------------------------------------------------

    def _renderable(self, msg: _Message) -> Any:
        """Build the rich renderable for a message record."""
        kind = msg.kind
        if kind == "thinking":
            return _thinking_renderable(msg.text, bool(msg.expanded))
        if kind == "user":
            return Panel(
                Text(msg.text, style="bold"),
                title="YOU",
                border_style="blue",
                padding=(0, 1),
            )
        if kind == "assistant":
            markdown = Markdown(msg.text, code_theme="monokai")
            subtitle = None
            wall = msg.meta.get("wall")
            if wall is not None:
                subtitle = Text.from_markup(f"[dim]wall={wall:.2f}s[/dim]")
            border_style = "yellow" if msg.meta.get("empty") else "green"
            return Panel(
                markdown,
                title="ASSISTANT",
                border_style=border_style,
                padding=(0, 1),
                subtitle=subtitle,
            )
        if kind == "system":
            return Panel(
                Text.from_markup(msg.text),
                title="r105",
                border_style="magenta",
                padding=(0, 1),
            )
        if kind == "error":
            return Panel(
                Text.from_markup(msg.text),
                title="ERROR",
                border_style="red",
                padding=(0, 1),
            )
        if kind == "tool_call":
            content = Text()
            content.append(f"🔧 {msg.meta.get('name', '')}", style="bold yellow")
            content.append("\n")
            content.append(msg.meta.get("args", ""), style="dim")
            return Panel(content, title="TOOL CALL", border_style="yellow", padding=(0, 1))
        if kind == "tool_result":
            preview = msg.text[:500] + ("…" if len(msg.text) > 500 else "")
            return Panel(
                Text(preview),
                title="TOOL RESULT",
                border_style="cyan",
                padding=(0, 1),
            )
        if kind == "tool_status":
            content = Text()
            content.append(f"🔧 {msg.text}", style=_DIM_WARM)
            return content
        if kind == "source":
            snippet = msg.meta.get("snippet", "")
            if snippet:
                content = Text()
                content.append(msg.text, style="bold cyan")
                content.append("\n")
                content.append(snippet[:300], style="dim")
                return Panel(content, title="SOURCE", border_style="cyan", padding=(0, 1))
            content = Text()
            content.append(f"📎 {msg.text}", style="bold cyan")
            return content
        return Panel(Text(str(msg.text)), title=kind, border_style="white", padding=(0, 1))

    def _build_widget(self, msg: _Message) -> Static:
        if msg.kind == "thinking":
            return ThinkingPanel(msg.text, bool(msg.expanded))
        return Static(self._renderable(msg))

    # -- Virtualization ------------------------------------------------------

    def on_mount(self) -> None:
        self.mount(self._top_spacer)
        self.mount(self._bottom_spacer)
        self._schedule_sync()

    def on_resize(self, event: Any) -> None:
        width = self.content_size.width
        if width != self._last_width:
            self._last_width = width
            self._invalidate_heights()
            self._schedule_sync()

    def watch_scroll_y(self, old_value: float, new_value: float) -> None:
        # A decrease means the user scrolled up: stop following the stream end
        # until they return to the bottom (or new content pins them again).
        if new_value < old_value:
            self._sticky_follow = False
        self._schedule_sync()

    def _schedule_sync(self) -> None:
        if self._sync_pending:
            return
        self._sync_pending = True
        self.call_later(self._sync_visible)

    def _invalidate_heights(self) -> None:
        for msg in self._messages:
            msg.height = None
            msg.width = None

    def _measure(self, msg: _Message, width: int) -> int:
        """Return the cached (or freshly measured) height of *msg* at *width*."""
        if msg.height is not None and msg.width == width:
            return msg.height
        if msg.kind == "thinking":
            height = _renderable_height(_thinking_renderable(msg.text, bool(msg.expanded)), width)
        else:
            height = _renderable_height(self._renderable(msg), width)
        msg.height = height
        msg.width = width
        return height

    def _scroll_to_end_if_at_bottom(self) -> None:
        """Follow the stream end when the user is already pinned to it.

        The actual scroll is deferred until after a refresh: ``scroll_y`` is
        clamped to ``max_scroll_y``, which only reflects the new content size
        after the layout pass has applied the spacer heights.
        """
        if not self.is_mounted:
            return
        try:
            at_bottom = self.scroll_y >= (self.max_scroll_y - 1)
        except Exception:
            at_bottom = True
        if at_bottom:
            self._sticky_follow = True
            self._schedule_follow_end()

    def _schedule_follow_end(self) -> None:
        if not self.is_mounted or self._follow_pending:
            return
        self._follow_pending = True
        self.call_after_refresh(self._follow_end)

    def _follow_end(self) -> None:
        self._follow_pending = False
        if not self.is_mounted:
            return
        width = self.content_size.width or 80
        total = sum(self._measure(msg, width) for msg in self._messages)
        viewport = max(1, self.content_size.height)
        self.scroll_to(y=max(0, total - viewport), animate=False, immediate=True)

    async def _sync_visible(self) -> None:
        """Reconcile the materialized widget window with the viewport.

        Keeps a bounded window of message widgets mounted (viewport + overscan)
        and sizes two invisible spacers so the scroll range always reflects the
        full transcript. Idempotent; safe to call on every scroll tick.
        """
        self._sync_pending = False
        if not self.is_mounted:
            return
        width = self.content_size.width or 80
        if width != self._last_width:
            self._last_width = width
            self._invalidate_heights()

        if not self._messages:
            self._top_spacer.styles.height = 0
            self._bottom_spacer.styles.height = 0
            self._window = []
            return

        heights = [self._measure(msg, width) for msg in self._messages]
        total = sum(heights)
        viewport = max(1, self.content_size.height)
        scroll_y = max(0, int(self.scroll_y))
        overscan = max(self._OVERSCAN_ROWS, viewport)

        # Prefix sums over cached heights.
        prefix = [0] * (len(heights) + 1)
        for i, height in enumerate(heights):
            prefix[i + 1] = prefix[i] + height

        win_start_y = max(0, scroll_y - overscan)
        win_end_y = scroll_y + viewport + overscan

        first = 0
        for i in range(len(heights)):
            if prefix[i + 1] > win_start_y:
                first = i
                break
        last = len(heights) - 1
        for i in range(len(heights) - 1, -1, -1):
            if prefix[i] < win_end_y:
                last = i
                break
        if last < first:
            last = first
        window = list(range(first, last + 1))

        # Unmount records that left the window.
        for index in list(self._window):
            if index < first or index > last:
                await self._unmount_message(index)
        # Materialize records that entered the window (kept in order).
        to_mount: list[Static] = []
        for index in window:
            msg = self._messages[index]
            if msg.widget is None:
                widget = self._build_widget(msg)
                msg.widget = widget
                to_mount.append(widget)
        if to_mount:
            await self.mount(*to_mount, before=self._bottom_spacer)

        self._window = window
        self._top_spacer.styles.height = prefix[first]
        self._bottom_spacer.styles.height = total - prefix[last + 1]

        # Keep the view pinned to the end while following. This covers content
        # that grew faster than layout/scroll state could be updated (e.g. a
        # synchronous burst of appends) — the scroll itself is deferred until
        # after the refresh so ``max_scroll_y`` reflects the new content size.
        target = max(0, total - viewport)
        if self.scroll_y >= target - 1:
            self._sticky_follow = True
        if self._sticky_follow:
            self._schedule_follow_end()

    async def _unmount_message(self, index: int) -> None:
        msg = self._messages[index]
        widget = msg.widget
        msg.widget = None
        if widget is not None and widget.is_mounted:
            await widget.remove()

    # -- Interaction ---------------------------------------------------------

    def on_thinking_panel_toggled(self, event: ThinkingPanel.Toggled) -> None:
        """A thinking panel was expanded/folded — re-measure and re-layout."""
        panel = event.panel
        for msg in self._messages:
            if msg.widget is panel:
                msg.expanded = panel.expanded
                msg.height = None
                break
        self._schedule_sync()

    def action_toggle_last_thinking(self) -> None:
        """Toggle the most recent thinking panel (keyboard fallback)."""
        for msg in reversed(self._messages):
            if msg.kind == "thinking":
                if isinstance(msg.widget, ThinkingPanel) and msg.widget.is_mounted:
                    msg.widget.toggle()
                else:
                    msg.expanded = not bool(msg.expanded)
                    msg.height = None
                    self._schedule_sync()
                return
