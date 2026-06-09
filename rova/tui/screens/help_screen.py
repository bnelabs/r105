"""Help screen showing the command reference."""

from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Static


class HelpScreen(ModalScreen[None]):
    """Modal screen displaying the slash-command reference."""

    def __init__(self, menu_text: str) -> None:
        super().__init__()
        self.menu_text = menu_text

    def compose(self) -> ComposeResult:
        with VerticalScroll(id="help-container"):
            yield Static(self.menu_text, id="help-text")
            yield Button("Close", variant="primary", id="help-close")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "help-close":
            self.app.pop_screen()
