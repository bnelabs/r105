"""Modal tool-call inspection screen (Ctrl+T).

Lists every tool call in the current session — name, arguments, and result
preview — so users can audit what the LLM executed without scrolling the
full transcript. Type to fuzzy-filter by tool name or argument text.
Escape (or Ctrl+T again): dismiss.
"""

from __future__ import annotations

import difflib
import json
from typing import Any

from textual import work
from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Static

from r105.state import ChatState


def _preview(text: str, limit: int = 300) -> str:
    flat = " ".join(str(text).split())
    return flat[:limit] + ("…" if len(flat) > limit else "")


def _collect_tool_calls(history: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Pair assistant tool_calls with their tool result messages by id."""
    results_by_id: dict[str, str] = {}
    for msg in history:
        if msg.get("role") == "tool":
            call_id = str(msg.get("tool_call_id", ""))
            if call_id:
                results_by_id[call_id] = str(msg.get("content", ""))
    calls: list[dict[str, Any]] = []
    for msg in history:
        for call in msg.get("tool_calls") or []:
            function = call.get("function") or {}
            call_id = str(call.get("id", ""))
            calls.append({
                "id": call_id,
                "name": str(function.get("name", "?")),
                "arguments": function.get("arguments", ""),
                "result": results_by_id.get(call_id, ""),
            })
    return calls


class ToolsScreen(ModalScreen[None]):
    """Modal screen showing the full tool-call history with results."""

    BINDINGS = [
        ("escape", "dismiss", "Close"),
        ("ctrl+t", "dismiss", "Close"),
    ]

    def __init__(self, state: ChatState) -> None:
        super().__init__()
        self.state = state
        self._filter: str = ""

    def compose(self) -> ComposeResult:
        yield Input(
            placeholder="Filter tools by name or arguments...",
            id="tools-search",
        )
        with VerticalScroll(id="tools-container"):
            yield Static(self._render_tools(), id="tools-text")
            yield Button("Close", variant="primary", id="tools-close")

    def on_mount(self) -> None:
        self.query_one("#tools-search", Input).focus()

    def _render_tools(self) -> str:
        calls = _collect_tool_calls(self.state.history)
        if not calls:
            return "[dim]No tool calls yet in this session.[/dim]"

        lines = ["[bold]Tool Calls[/bold]\n"]
        if self._filter:
            lines.append(f"[dim]Filter: {self._filter} — showing matching entries[/dim]\n")

        shown = 0
        for i, call in enumerate(calls, 1):
            haystack = f"{call['name']} {call['arguments']}"
            if self._filter and difflib.SequenceMatcher(
                None, haystack.lower(), self._filter.lower()
            ).ratio() < 0.3:
                continue
            args = call["arguments"]
            if isinstance(args, dict):
                args_text = json.dumps(args, sort_keys=True)
            else:
                args_text = str(args)
            lines.append(f"[bold]{i}.[/bold] 🔧 [bold]{call['name']}[/bold]")
            lines.append(f"   [dim]args:[/dim] {_preview(args_text)}")
            if call["result"]:
                lines.append(f"   [dim]→[/dim] {_preview(call['result'])}")
            else:
                lines.append("   [dim](no result yet)[/dim]")
            shown += 1

        if self._filter and shown == 0:
            lines.append("[dim]No matching tool calls found.[/dim]")
        lines.append(f"\n[dim]{shown} tool call(s) shown · {len(calls)} total[/dim]")
        return "\n".join(lines)

    def on_input_changed(self, event: Input.Changed) -> None:
        """Update the filter as the user types."""
        if event.input.id == "tools-search":
            self._filter = event.value.strip()
            self._refresh_display()

    @work
    async def _refresh_display(self) -> None:
        tools_text = self.query_one("#tools-text", Static)
        tools_text.update(self._render_tools())
        scroll = self.query_one("#tools-container", VerticalScroll)
        scroll.scroll_home(animate=False)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "tools-close":
            self.app.pop_screen()

    def action_dismiss(self, result: None = None) -> None:  # type: ignore[override]
        self.app.pop_screen()
