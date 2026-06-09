"""Main chat screen — the primary interactive screen with split layout."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path

import httpx
from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Static

from rova.client import RouterClient
from rova.state import ChatState, DEFAULT_MODEL, token_usage
from rova.commands import handle_slash_command
from rova.tools import execute_tool_call, TOOL_DEFINITIONS
from rova.tui.widgets.chat_view import ChatView
from rova.tui.widgets.input_area import ChatInput
from rova.tui.widgets.command_palette import CommandPalette, COMMAND_DEFS
from rova.tui.widgets.file_explorer import FileExplorer
from rova.tui.widgets.status_bar import StatusBarWidget


def _is_exact_command(text: str) -> bool:
    """Check if the input text has a recognized command as its first word.

    /help           → True  (exact match)
    /profile simple → True  (/profile is a known command)
    /prf            → False (fuzzy/partial, needs palette selection)
    """
    first_word = text.strip().split()[0] if text.strip() else ""
    for _cat, cmd, _usage, _desc in COMMAND_DEFS:
        if cmd == first_word:
            return True
    return False


class ChatScreen(Screen[None]):
    """The main chat screen with split layout: chat + file explorer/RAG pane."""

    def __init__(
        self,
        client: RouterClient,
        state: ChatState,
        workspace_dir: Path,
    ) -> None:
        super().__init__()
        self.client = client
        self.state = state
        self.workspace = workspace_dir
        self._http = httpx.AsyncClient()

    def on_unmount(self) -> None:
        asyncio.create_task(self._http.aclose())

    def compose(self) -> ComposeResult:
        yield Static(self._render_header(), id="rova-header")
        with Horizontal(id="main-content"):
            yield ChatView(id="chat-view")
            with Vertical(id="right-pane"):
                yield Static("[bold]RAG Sources[/bold]\n(none)", id="rag-sources")
                yield FileExplorer(self.workspace, id="file-explorer")
        yield CommandPalette(id="command-palette")
        yield ChatInput(id="chat-input")
        yield StatusBarWidget(id="status-bar")

    def on_mount(self) -> None:
        self._refresh_all()

    # -- Input handling ---------------------------------------------------

    def on_chat_input_chat_submitted(self, event: ChatInput.ChatSubmitted) -> None:
        """Handle a normal (non-slash) message submission."""
        text = event.value.strip()
        if not text:
            return

        chat_view = self.query_one("#chat-view", ChatView)

        if text.startswith("/"):
            _old_theme = self.state.theme
            result = await handle_slash_command(
                text, self.state, self.client, self.workspace
            )
            if text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
            if self.state.theme != _old_theme:
                try:
                    self.app.apply_theme(self.state.theme)
                except Exception:
                    pass
        else:
            chat_view.add_user(text)
            self._send_message(text)

        self._refresh_all()

    # -- Slash command palette --------------------------------------------

    def on_chat_input_slash_changed(self, event: ChatInput.SlashChanged) -> None:
        """Update the command palette filter as the user types."""
        palette = self.query_one("#command-palette", CommandPalette)
        if event.value.startswith("/"):
            palette.show_commands(event.value)
        else:
            palette.hide()

    def on_chat_input_slash_navigate(self, event: ChatInput.SlashNavigate) -> None:
        """Move the palette selection up or down."""
        palette = self.query_one("#command-palette", CommandPalette)
        if not palette.is_visible:
            return
        if event.direction == -1:
            palette.select_prev()
        else:
            palette.select_next()

    def on_chat_input_slash_select(self, event: ChatInput.SlashSelect) -> None:
        """Enter pressed in slash mode — select command or execute directly."""
        palette = self.query_one("#command-palette", CommandPalette)
        input_widget = self.query_one("#chat-input", ChatInput)
        chat_view = self.query_one("#chat-view", ChatView)

        current_text = input_widget.text.strip()
        palette.hide()

        if _is_exact_command(current_text):
            _old_theme = self.state.theme
            result = await handle_slash_command(
                current_text, self.state, self.client, self.workspace
            )
            if current_text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
            if self.state.theme != _old_theme:
                try:
                    self.app.apply_theme(self.state.theme)
                except Exception:
                    pass
            input_widget.clear()
            self._refresh_all()
            return

        selected_cmd = palette.get_selected_command()
        if selected_cmd:
            input_widget.text = selected_cmd + " "
            input_widget.cursor_location = (
                input_widget.document.line_count - 1,
                len(input_widget.text),
            )
            if selected_cmd.startswith("/"):
                input_widget.post_message(
                    ChatInput.SlashChanged(selected_cmd + " ")
                )
        self._refresh_all()

    def on_chat_input_slash_dismiss(self, event: ChatInput.SlashDismiss) -> None:
        """Escape pressed — hide the palette."""
        palette = self.query_one("#command-palette", CommandPalette)
        palette.hide()
        self._refresh_all()

    # -- File explorer ----------------------------------------------------

    def on_file_explorer_file_selected(self, event: FileExplorer.FileSelected) -> None:
        """Preview a file selected in the file explorer."""
        chat_view = self.query_one("#chat-view", ChatView)
        try:
            content = event.path.read_text(encoding="utf-8")
            preview = content[:1500] + ("…" if len(content) > 1500 else "")
            chat_view.add_system(
                f"[bold]Preview: {event.path.name}[/bold]\n{preview}"
            )
        except Exception as exc:
            chat_view.add_error(f"Cannot read {event.path.name}: {exc}")

    # -- Message sending & tool loop --------------------------------------

    @work(exclusive=True)
    async def _send_message(self, message: str) -> None:
        chat_view = self.query_one("#chat-view", ChatView)
        status_bar = self.query_one("#status-bar", StatusBarWidget)
        tools = TOOL_DEFINITIONS if self.state.profile == "tool_agent" else None

        status_bar.set_busy("Waiting for response...")
        try:
            result = await self.client.async_send(
                message, self.state, tools, self._http
            )
        except Exception as exc:
            status_bar.clear_busy()
            chat_view.add_error(f"Send failed: {exc}")
            self._refresh_all()
            return

        max_iterations = 10
        iteration = 0
        recent_calls: list[tuple[str, str]] = []  # track (name, args) for dedup
        while result.tool_calls and iteration < max_iterations:
            iteration += 1
            for tc in result.tool_calls:
                func = tc.get("function", {})
                name = func.get("name", "unknown")
                try:
                    args = func.get("arguments", {})
                    if isinstance(args, str):
                        args = json.loads(args)
                except (json.JSONDecodeError, TypeError):
                    args = {}
                args_str = json.dumps(args, indent=2, sort_keys=True)
                chat_view.add_tool_call(name, args_str)

                # Detect repeated tool calls (LLM stuck in a loop)
                call_key = (name, args_str)
                if call_key in recent_calls:
                    chat_view.add_system(
                        f"[dim]⚠️ Repeated call to {name} with same args — "
                        "injecting reminder to try a different approach[/dim]"
                    )
                recent_calls.append(call_key)
                if len(recent_calls) > 6:
                    recent_calls.pop(0)

                status_bar.set_busy(f"Executing {name}...")
                chat_view.add_tool_status(f"Running {name}...")

                tool_result_msg = execute_tool_call(tc, self.workspace)
                # If this is a duplicate, append a warning to the tool result
                if recent_calls.count(call_key) >= 2:
                    original = tool_result_msg.get("content", "")
                    tool_result_msg["content"] = (
                        f"{original}\n\n[SYSTEM NOTE: You just called {name} with "
                        "the same arguments. This already failed or returned no useful "
                        "result. Do NOT repeat this call. Try a different approach.]"
                    )
                result_content = tool_result_msg.get("content", "")
                chat_view.add_tool_result(result_content)

                self.state.history.append(
                    {
                        "role": "assistant",
                        "content": result.content or "",
                        "tool_calls": result.tool_calls,
                    }
                )
                self.state.history.append(tool_result_msg)

            try:
                status_bar.set_busy("Waiting for response...")
                result = await self.client.async_send(
                    "Tool results received. Continue or provide final answer.",
                    self.state,
                    tools,
                    self._http,
                )
            except Exception as exc:
                status_bar.clear_busy()
                chat_view.add_error(f"Tool loop error: {exc}")
                self._refresh_all()
                return

        status_bar.clear_busy()
        chat_view.add_assistant(result.content, result.wall_seconds)

        # Auto-compaction check
        if self.state.auto_compact:
            usage = token_usage(self.state)
            if usage.percent > 80:
                chat_view.add_system("[dim]⏳ Auto-compacting conversation (context > 80%)...[/dim]")
                try:
                    before = usage.used_tokens
                    self.client.compact(self.state)
                    after = token_usage(self.state).used_tokens
                    chat_view.add_system(f"[dim]Compacted {before} → {after} tokens[/dim]")
                except Exception:
                    chat_view.add_system("[dim]Auto-compaction skipped (client unavailable)[/dim]")

        self._refresh_all()
        self._refresh_file_explorer()

    # -- Refresh helpers --------------------------------------------------

    def _render_header(self) -> str:
        usage = token_usage(self.state)
        pct = f"({usage.percent:.0f}%)" if usage.percent > 0 else ""
        return (
            f"Rova  ·  {DEFAULT_MODEL}  ·  "
            f"{usage.used_tokens}/{usage.context_tokens} {pct}  ·  "
            f"{self.client.base_url}\n"
            f"profile={self.state.profile or 'auto'}  "
            f"rag={self.state.rag if self.state.rag is not None else 'auto'}  "
            f"quality={self.state.quality or 'auto'}  "
            f"auto-compact={'on' if self.state.auto_compact else 'off'}  "
            f"skills={','.join(self.state.active_skills) if self.state.active_skills else 'none'}"
        )

    def _refresh_all(self) -> None:
        self._refresh_header()
        self._refresh_status_bar()
        self._refresh_rag_sources()

    def _refresh_header(self) -> None:
        try:
            self.query_one("#rova-header", Static).update(self._render_header())
        except Exception:
            pass

    def _refresh_status_bar(self) -> None:
        try:
            usage = token_usage(self.state)
            self.query_one("#status-bar", StatusBarWidget).update_status(
                self.state, usage
            )
        except Exception:
            pass

    def _refresh_rag_sources(self) -> None:
        try:
            self.query_one("#rag-sources", Static).update(
                "[bold]RAG Sources[/bold]\n(none)"
            )
        except Exception:
            pass

    def _refresh_file_explorer(self) -> None:
        try:
            self.query_one("#file-explorer", FileExplorer).refresh_tree()
        except Exception:
            pass
