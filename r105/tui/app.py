"""Textual TUI for r105."""

from __future__ import annotations

from pathlib import Path

from textual.app import App
from textual.binding import Binding

from r105.client import BaseClient, Client
from r105.state import ChatState
from r105.tui.screens.chat import ChatScreen


class R105App(App):
    """Main r105 Textual application."""

    CSS_PATH = "../themes/r105.tcss"

    BINDINGS = [
        Binding("ctrl+q", "quit", "Quit", show=True, id="quit"),
        Binding("ctrl+c", "quit", "Quit", show=False),
        Binding("f1", "show_help", "Help", show=True, id="show_help"),
        Binding("ctrl+h", "show_help", "Help", show=False),
        Binding("ctrl+r", "show_history", "History", show=True, id="show_history"),
    ]

    _THEME_DIR = Path(__file__).resolve().parent.parent / "themes"

    THEME_PATHS = {
        "r105": str(_THEME_DIR / "r105.tcss"),
        "dracula": str(_THEME_DIR / "dracula.tcss"),
        "solarized-dark": str(_THEME_DIR / "solarized-dark.tcss"),
        "high-contrast": str(_THEME_DIR / "high-contrast.tcss"),
    }

    def __init__(
        self,
        client: BaseClient | Client,
        state: ChatState,
        workspace_dir: Path,
    ) -> None:
        super().__init__()
        if state.keybindings:
            self.set_keymap(state.keybindings)
        self.r105_client = client
        self.r105_state = state
        self.r105_workspace = workspace_dir
        self.chat_screen = ChatScreen(client, state, workspace_dir)

    def on_mount(self) -> None:
        self.push_screen(self.chat_screen)
        # Apply theme from state if set
        theme = self.r105_state.theme
        if theme in self.THEME_PATHS:
            self.apply_theme(theme)

    def apply_theme(self, name: str) -> None:
        """Apply a theme at runtime, using Textual's native theme API."""
        path = self.THEME_PATHS.get(name, str(self._THEME_DIR / "r105.tcss"))
        try:
            # Prefer Textual's native theme property (avoids bypassing CSS cache)
            if hasattr(self, "theme") and name in self.THEME_PATHS:
                self.theme = name
            else:
                self.stylesheet.read(path)
            self.refresh()
        except Exception:
            pass

    def action_show_help(self) -> None:
        from r105.commands import command_menu
        from r105.tui.screens.help_screen import HelpScreen

        self.push_screen(HelpScreen(command_menu()))

    def action_show_history(self) -> None:
        """Open the interactive history browser."""
        from r105.tui.screens.history_screen import HistoryScreen

        self.push_screen(HistoryScreen(self.r105_state, self.chat_screen))


def run_app(client: BaseClient | Client, state: ChatState, workspace_dir: Path) -> None:
    """Entry point called from cli.py to start the Textual app."""
    app = R105App(client, state, workspace_dir)
    app.run()
