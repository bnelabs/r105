"""Scrollable chat history widget with rich formatting and debounced streaming."""

from __future__ import annotations

import re
import time

from rich.markdown import Markdown
from rich.panel import Panel
from rich.text import Text
from textual.widgets import RichLog

# Gemma 4 thinking blocks: <|channel|>thought ... <channel|> (or <channel|>thought opener)
_THINKING_BLOCK_RE = re.compile(r"<\|?channel\|?>thought(.*?)<channel\|?>", re.DOTALL)
# Stray Gemma 4 token artifacts (bare channel markers, tool-call/result markers)
_STRAY_TOKENS_RE = re.compile(r"<\|?(?:channel|tool_call|tool_result)\|?>")


class ChatView(RichLog):
    """A scrollable chat history rendered with Rich formatting.

    Streaming is debounced: incoming tokens are buffered and the Markdown
    widget is updated every ~50ms, preventing CPU thrashing on long responses.

    Thinking blocks (``<|channel|>thought ... <channel|>``) are captured and
    rendered as a collapsible ``THINKING`` panel instead of being silently
    stripped — controlled by ``show_thinking`` (render at all) and
    ``thinking_default_expanded`` (show the full text vs. a folded preview).
    """

    _DEBOUNCE_MS = 0.05  # 50ms

    def __init__(
        self,
        show_thinking: bool = True,
        thinking_default_expanded: bool = False,
        gemma4_channel_syntax: bool = False,
        **kwargs,
    ) -> None:
        super().__init__(highlight=True, markup=True, **kwargs)
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

    @property
    def received_content(self) -> bool:
        """True if any displayable content has been streamed this session."""
        return self._received_content

    # -- Thinking capture --------------------------------------------------

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

    def _render_thinking(self, text: str) -> None:
        """Render a captured thinking block as a collapsible panel."""
        text = _STRAY_TOKENS_RE.sub("", text).strip()
        if not text:
            return
        if self.thinking_default_expanded:
            content = f"[dim]{text}[/dim]"
        else:
            preview = text if len(text) <= 300 else text[:300] + "…"
            content = (
                f"[dim]💭 {len(text)} chars of thinking — folded[/dim]\n"
                f"[dim #6c7086]{preview}[/dim #6c7086]"
            )
        self.write(Panel(content, title="💭 THINKING", border_style="cyan", padding=(0, 1)))

    def flush_thinking(self) -> None:
        """Render any captured thinking fragments and reset the accumulator."""
        if self._thinking_parts and self.show_thinking:
            self._render_thinking(" ".join(self._thinking_parts))
        self._thinking_parts = []
        # Discard an unterminated trailing block at stream end
        self._pending_thinking = ""

    # -- Message rendering -------------------------------------------------

    def add_user(self, text: str) -> None:
        if self.gemma4_channel_syntax:
            text = _STRAY_TOKENS_RE.sub("", text).strip()
        panel = Panel(
            Text(text, style="bold"),
            title="YOU",
            border_style="blue",
            padding=(0, 1),
        )
        self.write(panel)

    def add_assistant(self, text: str, wall_seconds: float | None = None) -> None:
        if self.gemma4_channel_syntax:
            clean, thinking = self._extract_thinking(text)
            clean = _STRAY_TOKENS_RE.sub("", clean).strip()
        else:
            # Model-agnostic path: content is opaque text, never rewritten.
            clean, thinking = text, ""
        if thinking and self.show_thinking:
            self._render_thinking(thinking)
        if not clean:
            self.write(Panel("(empty response)", title="ASSISTANT", border_style="yellow"))
            return
        markdown = Markdown(clean, code_theme="monokai")
        timing = f"\n[dim][wall={wall_seconds:.2f}s][/dim]" if wall_seconds is not None else ""
        content = Panel(
            markdown,
            title="ASSISTANT",
            border_style="green",
            padding=(0, 1),
            subtitle=timing,
        )
        self.write(content)

    def add_system(self, text: str) -> None:
        self.write(Panel(text, title="r105", border_style="magenta", padding=(0, 1)))

    def add_error(self, text: str) -> None:
        self.write(Panel(text, title="ERROR", border_style="red", padding=(0, 1)))

    def add_tool_call(self, name: str, args: str) -> None:
        content = f"[bold yellow]🔧 {name}[/bold yellow]\n[dim]{args}[/dim]"
        self.write(Panel(content, title="TOOL CALL", border_style="yellow", padding=(0, 1)))

    def add_tool_result(self, result: str) -> None:
        preview = result[:500] + ("…" if len(result) > 500 else "")
        self.write(
            Panel(
                preview,
                title="TOOL RESULT",
                border_style="cyan",
                padding=(0, 1),
            )
        )

    def add_tool_status(self, text: str) -> None:
        """Show an inline tool progress message (e.g. 'Executing Python...')."""
        self.write(f"[dim #f9e2af]🔧 {text}[/dim #f9e2af]")

    def add_source_attribution(self, source_tag: str, snippet: str = "") -> None:
        """Render a RAG source citation with optional snippet preview."""
        if snippet:
            self.write(
                Panel(
                    f"[bold cyan]{source_tag}[/bold cyan]\n[dim]{snippet[:300]}[/dim]",
                    title="SOURCE",
                    border_style="cyan",
                    padding=(0, 1),
                )
            )
        else:
            self.write(f"[bold cyan]📎 {source_tag}[/bold cyan]")

    # -- Debounced streaming -----------------------------------------------

    def start_streaming(self) -> None:
        """Begin a streaming assistant response."""
        self._streaming = True
        self._stream_buffer = ""
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
        if not getattr(self, "_streaming", False):
            self.start_streaming()
        self._stream_buffer += clean
        self._received_content = True

        now = time.monotonic()
        if now - self._last_render >= self._DEBOUNCE_MS:
            # Flush complete lines to the log
            while "\n" in self._stream_buffer:
                line, self._stream_buffer = self._stream_buffer.split("\n", 1)
                if line.strip():
                    self.write(line.strip())
            self._last_render = now

    def finish_streaming(self) -> None:
        """Flush any remaining buffered text and end the streaming session."""
        if not getattr(self, "_streaming", False):
            return
        # Flush remaining line
        remaining = self._stream_buffer.strip()
        if remaining:
            self.write(remaining)
        self._streaming = False
        self._stream_buffer = ""
        self.flush_thinking()
