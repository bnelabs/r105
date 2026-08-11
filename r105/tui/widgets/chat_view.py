"""Scrollable chat history widget with rich formatting and debounced streaming."""

from __future__ import annotations

import re
import time

from rich.markdown import Markdown
from rich.panel import Panel
from rich.text import Text
from textual.widgets import RichLog

# Strip Gemma 4 thinking blocks: <|channel|>thought ... <channel|>
_THINKING_BLOCK = re.compile(r"<\|channel\|?>thought.*?<channel\|?>", re.DOTALL)
# Strip stray Gemma 4 token artifacts
_STRAY_TOKENS = re.compile(r"<\|?(?:channel|tool_call|tool_result)\|?>")


class ChatView(RichLog):
    """A scrollable chat history rendered with Rich formatting.

    Streaming is debounced: incoming tokens are buffered and the Markdown
    widget is updated every ~50ms, preventing CPU thrashing on long responses.
    """

    _DEBOUNCE_MS = 0.05  # 50ms

    def __init__(self, **kwargs) -> None:
        super().__init__(highlight=True, markup=True, **kwargs)
        self._streaming = False
        self._stream_buffer = ""
        self._last_render = 0.0

    @staticmethod
    def _sanitize(text: str) -> str:
        """Strip Gemma 4 thinking blocks and stray special tokens from output.

        Removes:
          - <|channel|>thought...<channel|>  (thinking blocks)
          - <|channel>, <channel|>, <|tool_call|>, <|tool_result|>  (stray tokens)
        """
        text = _THINKING_BLOCK.sub("", text)
        text = _STRAY_TOKENS.sub("", text)
        return text.strip()

    def add_user(self, text: str) -> None:
        text = self._sanitize(text)
        panel = Panel(
            Text(text, style="bold"),
            title="YOU",
            border_style="blue",
            padding=(0, 1),
        )
        self.write(panel)

    def add_assistant(self, text: str, wall_seconds: float | None = None) -> None:
        text = self._sanitize(text)
        if not text:
            self.write(Panel("(empty response)", title="ASSISTANT", border_style="yellow"))
            return
        markdown = Markdown(text, code_theme="monokai")
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
        UI thread saturation during fast SSE streams.
        """
        text = self._sanitize(text)
        if not text:
            return
        if not getattr(self, "_streaming", False):
            self.start_streaming()
        self._stream_buffer += text

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
        remaining = self._sanitize(self._stream_buffer)
        if remaining:
            self.write(remaining)
        self._streaming = False
        self._stream_buffer = ""
