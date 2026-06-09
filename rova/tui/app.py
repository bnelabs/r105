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
    ]

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

    def action_show_help(self) -> None:
        from rova.commands import command_menu
        from rova.tui.screens.help_screen import HelpScreen

        self.push_screen(HelpScreen(command_menu()))


def run_app(client: RouterClient, state: ChatState, workspace_dir: Path) -> None:
    """Entry point called from cli.py to start the Textual app."""
    app = RovaApp(client, state, workspace_dir)
    app.run()
