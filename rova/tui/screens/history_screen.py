"""Interactive scrollable history browser."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Static

from rova.state import ChatState


class HistoryScreen(ModalScreen[None]):
    """Modal screen showing scrollable, searchable conversation history.

    Up/Down: scroll   Enter: re-submit selected   Escape: dismiss
    """

    def __init__(self, state: ChatState, chat_screen) -> None:
        super().__init__()
        self.state = state
        self.chat_screen = chat_screen

    def compose(self) -> ComposeResult:
        with VerticalScroll(id="help-container"):
            yield Static(self._render_history(), id="help-text")
            yield Button("Close", variant="primary", id="help-close")

    def _render_history(self) -> str:
        if not self.state.history:
            return "[dim]No conversation history yet.[/dim]"

        lines = ["[bold]Conversation History[/bold]\n"]
        for i, msg in enumerate(self.state.history, 1):
            role = msg.get("role", "?")
            content = " ".join(str(msg.get("content", "")).split())
            icon = {"user": "👤", "assistant": "🤖", "system": "⚙️", "tool": "🔧"}.get(role, "❓")
            role_name = role.upper()
            # Show first 120 chars per message
            preview = content[:120] + ("…" if len(content) > 120 else "")
            lines.append(f"[bold]{i}.[/bold] {icon} [bold]{role_name}[/bold] {preview}")

        return "\n".join(lines)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "help-close":
            self.app.pop_screen()
