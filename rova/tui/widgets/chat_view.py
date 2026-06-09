"""Scrollable chat history widget with rich formatting."""

from __future__ import annotations

from rich.markdown import Markdown
from rich.panel import Panel
from rich.text import Text
from textual.widgets import RichLog


class ChatView(RichLog):
    """A scrollable chat history rendered with Rich formatting."""

    def __init__(self, **kwargs) -> None:
        super().__init__(highlight=True, markup=True, **kwargs)

    def add_user(self, text: str) -> None:
        panel = Panel(
            Text(text, style="bold"),
            title="YOU",
            border_style="blue",
            padding=(0, 1),
        )
        self.write(panel)

    def add_assistant(self, text: str, wall_seconds: float | None = None) -> None:
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
        self.write(Panel(text, title="ROVA", border_style="magenta", padding=(0, 1)))

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

    # -- Streaming support -------------------------------------------------

    def start_streaming(self) -> None:
        """Begin a streaming assistant response (line-buffered rendering)."""
        self._streaming = True
        self._stream_buffer = ""

    def stream_chunk(self, text: str) -> None:
        """Accumulate a content delta, flushing complete lines to the log."""
        if not getattr(self, "_streaming", False):
            self.start_streaming()
        self._stream_buffer += text
        while "\n" in self._stream_buffer:
            line, self._stream_buffer = self._stream_buffer.split("\n", 1)
            if line.strip():
                self.write(line.strip())

    def finish_streaming(self) -> None:
        """Flush any remaining buffered text and end the streaming session."""
        if not getattr(self, "_streaming", False):
            return
        if self._stream_buffer.strip():
            self.write(self._stream_buffer.strip())
        self._streaming = False
        self._stream_buffer = ""
