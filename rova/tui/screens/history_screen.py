"""Interactive scrollable history browser with fuzzy search."""

from __future__ import annotations

import difflib

from textual import work
from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Static

from rova.state import ChatState


class HistoryScreen(ModalScreen[None]):
    """Modal screen showing scrollable, searchable conversation history.

    Type in the search box to fuzzy-filter messages by content.
    Up/Down: scroll   Enter: re-submit selected   Escape: dismiss   Ctrl+S: save session
    """

    BINDINGS = [
        ("escape", "dismiss", "Close"),
        ("ctrl+s", "save_session", "Save"),
    ]

    def __init__(self, state: ChatState, chat_screen) -> None:
        super().__init__()
        self.state = state
        self.chat_screen = chat_screen
        self._filter: str = ""

    def compose(self) -> ComposeResult:
        yield Input(
            placeholder="Search history (fuzzy)...",
            id="history-search",
        )
        with VerticalScroll(id="help-container"):
            yield Static(self._render_history(), id="history-text")
            yield Button("Close", variant="primary", id="history-close")

    def on_mount(self) -> None:
        self.query_one("#history-search", Input).focus()

    def _render_history(self) -> str:
        if not self.state.history:
            return "[dim]No conversation history yet.[/dim]"

        lines = ["[bold]Conversation History[/bold]\n"]
        if self._filter:
            lines.append(f"[dim]Filter: {self._filter} — showing matching entries[/dim]\n")

        count = 0
        for i, msg in enumerate(self.state.history, 1):
            role = msg.get("role", "?")
            content = " ".join(str(msg.get("content", "")).split())
            icon = {"user": "👤", "assistant": "🤖", "system": "⚙️", "tool": "🔧"}.get(role, "❓")
            role_name = role.upper()

            # Apply fuzzy filter (if any)
            if self._filter:
                score = max(
                    _fuzzy_score(content, self._filter),
                    _fuzzy_score(role, self._filter),
                )
                if score < 0.4:
                    continue

            # Show first 120 chars per message
            preview = content[:120] + ("…" if len(content) > 120 else "")
            lines.append(f"[bold]{i}.[/bold] {icon} [bold]{role_name}[/bold] {preview}")
            count += 1

        if self._filter and count == 0:
            lines.append("[dim]No matching messages found.[/dim]")

        lines.append(f"\n[dim]{count} message(s) shown · {len(self.state.history)} total[/dim]")
        return "\n".join(lines)

    def on_input_changed(self, event: Input.Changed) -> None:
        """Update the filter as the user types."""
        if event.input.id == "history-search":
            self._filter = event.value.strip()
            self._refresh_display()

    @work
    async def _refresh_display(self) -> None:
        history_text = self.query_one("#history-text", Static)
        history_text.update(self._render_history())
        # Scroll to top when filter changes
        scroll = self.query_one("#help-container", VerticalScroll)
        scroll.scroll_home(animate=False)

    def action_save_session(self) -> None:
        """Save the current conversation with auto-generated name."""
        from rova.sessions import save_session

        name = f"history-{len(self.state.history)}msgs"
        try:
            path = save_session(self.state, name)
            self._show_status(f"Session saved: {name}")
        except OSError as exc:
            self._show_status(f"Save failed: {exc}")

    def _show_status(self, message: str) -> None:
        status = self.query_one("#history-text", Static)
        status.update(f"[dim]{message}[/dim]\n\n{self._render_history()}")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "history-close":
            self.app.pop_screen()

    def action_dismiss(self) -> None:
        self.app.pop_screen()


def _fuzzy_score(text: str, query: str) -> float:
    """Return a fuzzy match score (0.0–1.0) between *text* and *query*.

    Uses difflib SequenceMatcher for the score. Case-insensitive.
    """
    if not query or not text:
        return 0.0
    return difflib.SequenceMatcher(
        None, text.lower(), query.lower()
    ).ratio()
