"""Bottom status bar showing profile, quality, context usage, and streaming status."""

from __future__ import annotations

import time

from textual.widgets import Static

from r105.state import ChatState, TokenUsage


class StatusBarWidget(Static):
    """Footer bar showing current settings, context usage, and busy/streaming state."""

    _STREAMING_FRAMES = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂"]
    _STREAMING_INTERVAL = 0.15  # seconds per frame

    def __init__(self, **kwargs) -> None:
        super().__init__("", **kwargs)
        self._busy: bool = False
        self._saved_status: str = ""
        self._streaming: bool = False
        self._stream_frame: int = 0
        self._last_stream_update: float = 0.0
        self._sandbox_warning: str = ""

    def set_sandbox_warning(self, warning: str) -> None:
        """Pin a sandbox-downgrade warning (e.g. weak isolation backend)."""
        self._sandbox_warning = warning

    def update_status(
        self,
        state: ChatState,
        usage: TokenUsage,
        sandbox_warning: str | None = None,
    ) -> None:
        if sandbox_warning is not None:
            self._sandbox_warning = sandbox_warning
        if self._busy or self._streaming:
            self._saved_status = self._render_status_line(
                state, usage, self._sandbox_warning
            )
            return
        self._saved_status = ""
        self.update(self._render_status_line(state, usage, self._sandbox_warning))

    def set_busy(self, text: str) -> None:
        """Show a busy/loading indicator (e.g. tool execution, API call)."""
        self._busy = True
        self._streaming = False
        spinner = "⏳"
        if "Executing" in text or "Running" in text:
            spinner = "🔧"
        elif "Searching" in text or "Fetching" in text:
            spinner = "🔍"
        elif "Generating" in text or "Waiting" in text:
            spinner = "⏳"
        elif "Reading" in text or "Writing" in text:
            spinner = "📄"
        self.update(f"[bold #f9e2af]{spinner} {text}[/bold #f9e2af]")

    def set_streaming(self) -> None:
        """Show an animated streaming indicator (braille-style progress)."""
        self._busy = False
        self._streaming = True
        self._stream_frame = 0
        self._last_stream_update = 0.0
        self._update_stream_frame()

    def clear_busy(self) -> None:
        """Clear busy/streaming indicator and restore saved status."""
        self._busy = False
        self._streaming = False
        if self._saved_status:
            self.update(self._saved_status)
            self._saved_status = ""

    def tick(self) -> None:
        """Advance animation frame. Called from the TUI app's periodic timer."""
        if self._streaming:
            now = time.monotonic()
            if now - self._last_stream_update >= self._STREAMING_INTERVAL:
                self._update_stream_frame()

    def _update_stream_frame(self) -> None:
        """Render the next animation frame for the streaming indicator."""
        self._last_stream_update = time.monotonic()
        bar = self._STREAMING_FRAMES[self._stream_frame]
        self._stream_frame = (self._stream_frame + 1) % len(self._STREAMING_FRAMES)
        self.update(f"[bold #89dceb]{bar} Receiving…[/bold #89dceb]")

    @staticmethod
    def _render_status_line(
        state: ChatState, usage: TokenUsage, sandbox_warning: str = ""
    ) -> str:
        profile = state.profile or "auto"
        skills = f"+{len(state.active_skills)} skill" if state.active_skills else "plain"
        ctx_line = f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)"
        line = (
            f"r105 {profile}/{skills}  │  {ctx_line}  │  Type / for commands, /exit to quit"
        )
        if sandbox_warning:
            line += f"  │  ⚠ {sandbox_warning}"
        return line
