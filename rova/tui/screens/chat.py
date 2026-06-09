"""Main chat screen — the primary interactive screen with split layout."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import httpx
from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Static

from rova.client import RouterClient
from rova.commands import handle_slash_command
from rova.state import DEFAULT_MODEL, ChatState, token_usage
from rova.tools import execute_tool_call, get_tool_definitions
from rova.tui.widgets.chat_view import ChatView
from rova.tui.widgets.command_palette import COMMAND_DEFS, CommandPalette
from rova.tui.widgets.file_explorer import FileExplorer
from rova.tui.widgets.input_area import ChatInput
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

    async def on_unmount(self) -> None:
        await self._http.aclose()

    def compose(self) -> ComposeResult:
        yield Static(self._render_header(), id="rova-header")
        with Horizontal(id="main-content"):
            yield ChatView(id="chat-view")
            with Vertical(id="right-pane"):
                yield FileExplorer(self.workspace, id="file-explorer")
        yield CommandPalette(id="command-palette")
        yield ChatInput(id="chat-input")
        yield StatusBarWidget(id="status-bar")

    def on_mount(self) -> None:
        self._refresh_all()

    # -- Input handling ---------------------------------------------------

    async def on_chat_input_chat_submitted(self, event: ChatInput.ChatSubmitted) -> None:
        """Handle a normal (non-slash) message submission."""
        text = event.value.strip()
        if not text:
            return

        chat_view = self.query_one("#chat-view", ChatView)

        if text.startswith("/"):
            _old_theme = self.state.theme
            result = await handle_slash_command(
                text, self.state, self.client, self.workspace, http_client=self._http
            )
            if text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
            if self.state.theme != _old_theme:
                try:
                    self.app.apply_theme(self.state.theme)  # type: ignore[attr-defined]
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

    async def on_chat_input_slash_select(self, event: ChatInput.SlashSelect) -> None:
        """Enter pressed in slash mode — select command or execute directly."""
        palette = self.query_one("#command-palette", CommandPalette)
        input_widget = self.query_one("#chat-input", ChatInput)
        chat_view = self.query_one("#chat-view", ChatView)

        current_text = input_widget.text.strip()
        palette.hide()

        if _is_exact_command(current_text):
            _old_theme = self.state.theme
            result = await handle_slash_command(
                current_text, self.state, self.client, self.workspace, http_client=self._http
            )
            if current_text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
            if self.state.theme != _old_theme:
                try:
                    self.app.apply_theme(self.state.theme)  # type: ignore[attr-defined]
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
            content = event.path.read_text(encoding="utf-8", errors="replace")
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
        tools = get_tool_definitions() if self.state.profile == "tool_agent" else None

        status_bar.set_busy("Streaming...")
        chat_view.start_streaming()
        try:
            result = await self.client.async_send_streaming(
                message, self.state, tools, self._http,
                on_chunk=lambda token: chat_view.stream_chunk(token),
            )
        except Exception as exc:
            chat_view.add_error(f"Send failed: {exc}")
            self._refresh_all()
            return
        finally:
            chat_view.finish_streaming()
            status_bar.clear_busy()

        max_iterations = 10
        iteration = 0
        had_tools = bool(result.tool_calls)
        recent_calls: list[tuple[str, str]] = []  # track (name, args) for cross-iteration dedup
        while result.tool_calls and iteration < max_iterations:
            iteration += 1
            # Assistant message (with tool_calls) already recorded by async_send/async_continue.
            # Pre-parse tool call signatures once for dedup detection.
            signatures: list[tuple[dict[str, Any], str, str]] = []  # (tc, name, args_str)
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
                signatures.append((tc, name, args_str))

            # Phase 1: UI updates and dedup checks (main thread)
            call_keys: list[tuple[str, str]] = []
            for _tc, name, args_str in signatures:
                chat_view.add_tool_call(name, args_str)

                call_key = (name, args_str)
                call_keys.append(call_key)
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

            # Phase 2: Execute all tools in parallel via thread pool
            async def _exec_one(tc: dict[str, Any]) -> dict[str, Any]:
                return await asyncio.to_thread(execute_tool_call, tc, self.workspace)

            tool_results = await asyncio.gather(
                *[_exec_one(tc) for tc, _name, _args_str in signatures],
                return_exceptions=True,
            )

            # Phase 3: Process results and update history
            for (tc, name, args_str), tool_result_msg in zip(signatures, tool_results, strict=True):
                if isinstance(tool_result_msg, BaseException):
                    chat_view.add_error(f"Tool {name} failed: {tool_result_msg}")
                    tool_result_msg = {
                        "role": "tool",
                        "tool_call_id": tc.get("id", ""),
                        "name": name,
                        "content": f"error: {tool_result_msg}",
                    }
                call_key = (name, args_str)
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

                self.state.history.append(tool_result_msg)

            try:
                status_bar.set_busy("Streaming...")
                chat_view.start_streaming()
                result = await self.client.async_continue(
                    self.state,
                    tools,
                    self._http,
                    on_chunk=lambda token: chat_view.stream_chunk(token),
                )
            except Exception as exc:
                chat_view.add_error(f"Tool loop error: {exc}")
                self._refresh_all()
                return
            finally:
                chat_view.finish_streaming()
                status_bar.clear_busy()

        status_bar.clear_busy()
        # Show final response as markdown panel only when tools were involved
        # (the first streaming response was just an intermediate message).
        # When no tools were needed, the streamed content IS the final answer.
        if had_tools:
            chat_view.add_assistant(result.content, result.wall_seconds)

        # Auto-compaction check
        if self.state.auto_compact:
            usage = token_usage(self.state)
            if usage.percent > 80:
                chat_view.add_system("[dim]⏳ Auto-compacting conversation (context > 80%)...[/dim]")
                try:
                    before = usage.used_tokens
                    await self.client.async_compact(self.state, client=self._http)
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

    def _refresh_file_explorer(self) -> None:
        try:
            self.query_one("#file-explorer", FileExplorer).refresh_tree()
        except Exception:
            pass
