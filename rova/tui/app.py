"""Textual TUI for Rova."""

from __future__ import annotations

from pathlib import Path

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import Header, Footer

from rova.client import RouterClient
from rova.state import ChatState
from rova.tui.screens.chat import ChatScreen


class RovaApp(App):
    """Main Rova Textual application."""

    CSS_PATH = "../themes/rova.tcss"

    BINDINGS = [
        Binding("ctrl+q", "quit", "Quit", show=True),
        Binding("ctrl+c", "quit", "Quit", show=False),
        Binding("f1", "show_help", "Help", show=True),
        Binding("ctrl+h", "show_help", "Help", show=False),
        Binding("ctrl+r", "show_history", "History", show=True),
    ]

    _THEME_DIR = Path(__file__).resolve().parent.parent / "themes"

    THEME_PATHS = {
        "rova": str(_THEME_DIR / "rova.tcss"),
        "dracula": str(_THEME_DIR / "dracula.tcss"),
        "solarized-dark": str(_THEME_DIR / "solarized-dark.tcss"),
        "high-contrast": str(_THEME_DIR / "high-contrast.tcss"),
    }

    def __init__(
        self,
        client: RouterClient,
        state: ChatState,
        workspace_dir: Path,
    ) -> None:
        super().__init__()
        self.rova_client = client
        self.rova_state = state
        self.rova_workspace = workspace_dir
        self.chat_screen = ChatScreen(client, state, workspace_dir)

    def on_mount(self) -> None:
        self.push_screen(self.chat_screen)
        # Apply theme from state if set
        theme = self.rova_state.theme
        if theme in self.THEME_PATHS:
            self.apply_theme(theme)

    def apply_theme(self, name: str) -> None:
        """Apply a theme at runtime, using Textual's native theme API."""
        path = self.THEME_PATHS.get(name, str(self._THEME_DIR / "rova.tcss"))
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
        from rova.commands import command_menu
        from rova.tui.screens.help_screen import HelpScreen

        self.push_screen(HelpScreen(command_menu()))

    def action_show_history(self) -> None:
        """Open the interactive history browser."""
        from rova.tui.screens.history_screen import HistoryScreen

        self.push_screen(HistoryScreen(self.rova_state, self.chat_screen))


def run_app(client: RouterClient, state: ChatState, workspace_dir: Path) -> None:
    """Entry point called from cli.py to start the Textual app."""
    app = RovaApp(client, state, workspace_dir)
    app.run()
