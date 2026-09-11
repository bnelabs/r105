"""Main chat screen — the primary interactive screen with split layout."""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import Any, Literal

import httpx
from textual import work
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Static

from r105.client import BaseClient, Client, create_client
from r105.commands import copy_to_clipboard, handle_slash_command
from r105.config import ensure_config
from r105.constants import (
    AUTO_COMPACT_THRESHOLD_PCT,
    MAX_TOOL_LOOP_ITERATIONS,
)
from r105.errors import format_request_error
from r105.model_catalog import uses_gemma4_channel_syntax
from r105.sandbox import current_backend_name, weak_backend_warning
from r105.sessions import auto_save
from r105.state import ChatState, token_usage
from r105.tool_loop import (
    LoopDedupTracker,
    parse_tool_signatures,
    run_tools_parallel,
)
from r105.tools import execute_tool_call, get_tool_definitions
from r105.tui.widgets.chat_view import ChatView
from r105.tui.widgets.command_palette import COMMAND_DEFS, CommandPalette
from r105.tui.widgets.file_explorer import FileExplorer
from r105.tui.widgets.input_area import ChatInput
from r105.tui.widgets.status_bar import StatusBarWidget


def _is_exact_command(text: str) -> bool:
    """Check if the input text has a recognized command as its first word.

    /help           → True  (exact match)
    /profile simple → True  (/profile is a known command)
    /prf            → False (fuzzy/partial, needs palette selection)
    """
    first_word = text.strip().split()[0] if text.strip() else ""
    for _cat, cmd, _usage, _desc in COMMAND_DEFS:
        if cmd == first_word or cmd.split()[0] == first_word:
            return True
    return False


class ChatScreen(Screen[None]):
    """The main chat screen with split layout: chat + file explorer pane."""

    BINDINGS = [
        Binding("ctrl+y", "copy_last_message", "Copy last response", id="copy_last_message"),
        Binding("ctrl+t", "show_tools", "Inspect tool calls", id="show_tools"),
        Binding("ctrl+x", "cancel_tools", "Cancel local tools", id="cancel_tools"),
        Binding("escape", "cancel_request", "Cancel current request", id="cancel_request"),
    ]

    def __init__(
        self,
        client: BaseClient | Client,
        state: ChatState,
        workspace_dir: Path,
    ) -> None:
        super().__init__()
        self.client = client
        self.state = state
        self.workspace = workspace_dir
        self._http = httpx.AsyncClient()
        self._active_worker: Any | None = None
        self._tool_batch_task: asyncio.Task[Any] | None = None
        self._backend_health = "checking"
        self._sandbox_backend = current_backend_name() or "auto"
        self._workspace_status = "ok" if os.access(self.workspace, os.W_OK) else "unwritable"
        # Cheap read (no detection): warns only if a weak backend is active.
        self._sandbox_warning = weak_backend_warning() or ""

    async def on_unmount(self) -> None:
        # Cancel any in-flight LLM/tool worker so background tasks do not
        # outlive the screen (leaked workers keep the httpx pool and the
        # asyncio loop busy after the UI is gone).
        worker, self._active_worker = self._active_worker, None
        if worker is not None:
            try:
                worker.cancel()
            except Exception:
                pass
        tool_task, self._tool_batch_task = self._tool_batch_task, None
        if tool_task is not None:
            tool_task.cancel()
        saved = auto_save(self.state)
        if saved:
            self._notify(f"Session autosaved: {saved}", severity="information")
        await self._http.aclose()

    def compose(self) -> ComposeResult:
        yield Static(self._render_header(), id="r105-header")
        with Horizontal(id="main-content"):
            yield ChatView(
                show_thinking=self.state.show_thinking,
                thinking_default_expanded=self.state.thinking_default_expanded,
                gemma4_channel_syntax=uses_gemma4_channel_syntax(
                    self.state.model, self.state.model_families
                ),
                id="chat-view",
            )
            with Vertical(id="right-pane"):
                yield FileExplorer(self.workspace, id="file-explorer")
        yield CommandPalette(id="command-palette")
        yield ChatInput(id="chat-input")
        yield StatusBarWidget(id="status-bar")

    def on_mount(self) -> None:
        # Periodic timer for status bar animation (sprite frames)
        self.set_interval(0.05, self._tick_status_bar)
        self.set_interval(30.0, self._start_health_check)
        self._start_health_check()
        self._refresh_all()
        # Textual 8 made RichLog focusable, so ChatView steals startup focus
        # from the chat input and the TUI ignores keyboard input until the
        # user clicks/tabs. Explicitly focus the input on mount.
        self.query_one("#chat-input", ChatInput).focus()

    # -- Toast notifications for background events -------------------------------

    def _notify(self, message: str, severity: Literal["information", "warning", "error"] = "information") -> None:
        """Show a non-blocking toast notification.

        Textual's built-in notify() is used — it auto-dismisses after a timeout.
        """
        try:
            self.app.notify(message, severity=severity, timeout=4)
        except Exception:
            pass

    def _tick_status_bar(self) -> None:
        """Advance status bar animation frames (streaming indicator)."""
        status_bar = self.query_one("#status-bar", StatusBarWidget)
        status_bar.tick()

    def _start_health_check(self) -> None:
        """Start a periodic non-blocking backend health probe."""
        self._check_backend_health()

    @work(exclusive=True)
    async def _check_backend_health(self) -> None:
        """Refresh backend connectivity without blocking Textual's event loop."""
        self._backend_health = "checking"
        self._refresh_status_bar()
        try:
            result = await self.client.async_health(client=self._http)
            if result.get("ok"):
                count = result.get("models_available")
                self._backend_health = f"ok/{count}" if count is not None else "ok"
            else:
                self._backend_health = "down"
        except Exception:
            self._backend_health = "down"
        self._refresh_status_bar()

    def action_copy_last_message(self) -> None:
        """Copy the last assistant message to the system clipboard."""
        last_content = ""
        for msg in reversed(self.state.history):
            if msg.get("role") == "assistant":
                last_content = msg.get("content", "")
                break

        if not last_content:
            return

        if copy_to_clipboard(last_content):
            chat_view = self.query_one("#chat-view", ChatView)
            chat_view.add_system(f"[dim]Copied {len(last_content)} chars to clipboard[/dim]")
        else:
            chat_view = self.query_one("#chat-view", ChatView)
            chat_view.add_system("[dim]Clipboard unavailable (install xclip or wl-copy)[/dim]")

    def action_show_tools(self) -> None:
        """Open the tool-call inspection screen (Ctrl+T)."""
        from r105.tui.screens.tools_screen import ToolsScreen

        self.app.push_screen(ToolsScreen(self.state))

    def action_cancel_request(self) -> None:
        """Cancel the in-flight LLM request (Esc). Shows a [CANCELED] marker."""
        worker = self._active_worker
        if worker is not None:
            try:
                worker.cancel()
            except Exception:
                pass
            try:
                chat_view = self.query_one("#chat-view", ChatView)
                chat_view.finish_streaming()
                chat_view.add_canceled("Request canceled by user")
            except Exception:
                pass
            try:
                status_bar = self.query_one("#status-bar", StatusBarWidget)
                status_bar.clear_busy()
            except Exception:
                pass
            self._active_worker = None
            self._refresh_all()

    def action_cancel_tools(self) -> None:
        """Cancel the current local-tool batch without leaving the TUI."""
        task = self._tool_batch_task
        if task is None or task.done():
            self._notify("No local tool execution is active", severity="information")
            return
        task.cancel()
        self._notify("Canceling local tool execution…", severity="warning")

    # -- Input handling ---------------------------------------------------

    async def _execute_slash_command(self, text: str, chat_view: ChatView) -> None:
        """Execute a slash command, handle theme changes, and exit requests."""
        old_theme = self.state.theme
        result = await handle_slash_command(
            text, self.state, self.client, self.workspace, http_client=self._http
        )
        command = text.strip().split(maxsplit=1)[0].lower() if text.strip() else ""
        if command in {"/connect", "/provider"} and result.startswith("provider="):
            self._reconfigure_client()
        if text in {"/exit", "/quit"}:
            self.app.exit()
            return
        # /model <name> switches the model: re-resolve the channel-syntax
        # capability (respecting model_families config overrides) so the
        # transcript handles the new model's output correctly.
        if text.startswith("/model"):
            chat_view.set_gemma4_channel_syntax(
                uses_gemma4_channel_syntax(self.state.model, self.state.model_families)
            )
        if text.startswith("/config"):
            # Config reload updates settings that the live app owns. Textual
            # keymap changes and transcript rendering flags need an explicit
            # refresh because the command handler only has the shared state.
            self.app.set_keymap(self.state.keybindings)
            chat_view.show_thinking = self.state.show_thinking
            chat_view.thinking_default_expanded = self.state.thinking_default_expanded
            chat_view.set_gemma4_channel_syntax(
                uses_gemma4_channel_syntax(self.state.model, self.state.model_families)
            )
        chat_view.add_system(result)
        if self.state.theme != old_theme:
            try:
                self.app.apply_theme(self.state.theme)  # type: ignore[attr-defined]
            except Exception:
                pass

    def _reconfigure_client(self) -> None:
        """Apply the provider selected by /connect to the live TUI."""
        try:
            config = ensure_config(strict=True)
            self.client = create_client(
                base_url=config.get("url"),
                backend=config.get("backend"),
            )
            if hasattr(self.app, "r105_client"):
                self.app.r105_client = self.client
            self._backend_health = "checking"
            self._start_health_check()
            self._refresh_all()
        except (OSError, ValueError) as exc:
            self._notify(f"Provider switch failed: {exc}", severity="error")

    async def on_chat_input_chat_submitted(self, event: ChatInput.ChatSubmitted) -> None:
        """Handle a normal (non-slash) message submission."""
        text = event.value.strip()
        if not text:
            return

        chat_view = self.query_one("#chat-view", ChatView)

        if text.startswith("/"):
            await self._execute_slash_command(text, chat_view)
        else:
            chat_view.add_user(text)
            try:
                self._active_worker = self._send_message(text)
            except Exception:
                self._active_worker = None

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
            await self._execute_slash_command(current_text, chat_view)
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
                on_status=lambda status: status_bar.set_busy(status),
            )
        except asyncio.CancelledError:
            # User pressed Esc: leave a distinct marker so the transcript
            # explains why output stopped mid-stream.
            try:
                chat_view.finish_streaming()
            except Exception:
                pass
            chat_view.add_canceled("Request canceled")
            self._refresh_all()
            return
        except Exception as exc:
            chat_view.add_error(format_request_error(exc, action="Send"))
            self._refresh_all()
            return
        finally:
            try:
                chat_view.finish_streaming()
            except Exception:
                pass
            try:
                status_bar.clear_busy()
            except Exception:
                pass

        max_iterations = MAX_TOOL_LOOP_ITERATIONS
        iteration = 0
        had_tools = bool(result.tool_calls)
        tracker = LoopDedupTracker()  # (name, args) dedup across iterations
        while result.tool_calls and iteration < max_iterations:
            iteration += 1
            # Assistant message (with tool_calls) already recorded by async_send/async_continue.
            # Pre-parse tool call signatures once for dedup detection.
            signatures = parse_tool_signatures(result.tool_calls)

            # Phase 1: UI updates and dedup checks (main thread)
            call_keys: list[tuple[str, str]] = []
            stuck = False
            for _tc, name, args_str in signatures:
                chat_view.add_tool_call(name, args_str)

                call_keys.append((name, args_str))
                is_stuck, is_repeat = tracker.check(name, args_str)
                if is_stuck:
                    stuck = True
                    count = tracker.count_for(name, args_str)
                    chat_view.add_system(
                        f"[bold red]🛑 Repeated call to {name} ({count}x) — "
                        "forcing tool loop break[/bold red]"
                    )
                    self.state.history.append({
                        "role": "system",
                        "content": LoopDedupTracker.stop_loop_message(name, count),
                    })
                elif is_repeat:
                    count = tracker.count_for(name, args_str)
                    chat_view.add_system(
                        f"[dim]⚠️ Repeated call to {name} with same args ({count}x) — "
                        "try a different approach[/dim]"
                    )

            if stuck:
                break

            # Phase 2: Execute all tools in parallel via thread pool with timeouts
            tool_task = asyncio.create_task(
                run_tools_parallel(
                    signatures,
                    lambda tc: execute_tool_call(
                        tc, self.workspace, trace_id=self.state.trace_id
                    ),
                )
            )
            self._tool_batch_task = tool_task
            try:
                outcomes = await tool_task
            except asyncio.CancelledError:
                # Ctrl+X cancels this task directly. Escape/unmount cancels the
                # parent worker as well, so only add a marker when the worker
                # is still active and the action has not already rendered one.
                if self._active_worker is not None:
                    chat_view.finish_streaming()
                    chat_view.add_canceled("Tool execution canceled by user")
                status_bar.clear_busy()
                self._active_worker = None
                self._refresh_all()
                return
            finally:
                if self._tool_batch_task is tool_task:
                    self._tool_batch_task = None

            # Phase 3: Process results and update history
            for (_tc, name, args_str), (tool_result_msg, tool_error) in zip(signatures, outcomes, strict=True):
                if tool_error is not None:
                    chat_view.add_error(f"Tool {name} failed: {tool_error}")
                # If this is a duplicate, append a warning to the tool result
                if tracker.is_duplicate_result(name, args_str):
                    original = tool_result_msg.get("content", "")
                    tool_result_msg["content"] = (
                        f"{original}{LoopDedupTracker.duplicate_note(name)}"
                    )
                result_content = tool_result_msg.get("content", "")
                chat_view.add_tool_result(result_content)

                self.state.history.append(tool_result_msg)

            try:
                status_bar.set_streaming()
                chat_view.start_streaming()
                result = await self.client.async_continue(
                    self.state,
                    tools,
                    self._http,
                    on_chunk=lambda token: chat_view.stream_chunk(token),
                    on_status=lambda status: status_bar.set_busy(status),
                )
            except asyncio.CancelledError:
                try:
                    chat_view.finish_streaming()
                except Exception:
                    pass
                chat_view.add_canceled("Request canceled during tool loop")
                self._refresh_all()
                return
            except Exception as exc:
                chat_view.add_error(format_request_error(exc, action="Tool loop"))
                self._refresh_all()
                return
            finally:
                try:
                    chat_view.finish_streaming()
                except Exception:
                    pass
                try:
                    status_bar.clear_busy()
                except Exception:
                    pass

        status_bar.clear_busy()
        # Show final response as markdown panel only when tools were involved
        # (the first streaming response was just an intermediate message).
        # When no tools were needed, the streamed content IS the final answer.
        if had_tools:
            chat_view.add_assistant(result.content, result.wall_seconds)
        elif result.content and not chat_view.received_content:
            # Nothing streamed (thinking models can exhaust their budget during
            # reasoning, leaving content empty in every delta). The client
            # falls back to reasoning_content; render it here since no live
            # text was shown.
            chat_view.add_assistant(result.content, result.wall_seconds)

        # Auto-compaction check
        if self.state.auto_compact:
            usage = token_usage(self.state)
            if usage.percent > AUTO_COMPACT_THRESHOLD_PCT:
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
        self._active_worker = None

    # -- Refresh helpers --------------------------------------------------

    def _render_header(self) -> str:
        usage = token_usage(self.state)
        pct = f"({usage.percent:.0f}%)" if usage.percent > 0 else ""
        return (
            f"r105  ·  {self.state.model}  ·  "
            f"{usage.used_tokens}/{usage.context_tokens} {pct} [{usage.estimate_label}]  ·  "
            f"{self.client.base_url}\n"
            f"profile={self.state.profile or 'auto'}  "
            f"quality={self.state.quality or 'auto'}  "
            f"auto-compact={'on' if self.state.auto_compact else 'off'}  "
            f"skills={','.join(self.state.active_skills) if self.state.active_skills else 'none'}"
        )

    def _refresh_all(self) -> None:
        self._refresh_header()
        self._refresh_status_bar()

    def _refresh_header(self) -> None:
        try:
            self.query_one("#r105-header", Static).update(self._render_header())
        except Exception:
            pass

    def _refresh_status_bar(self) -> None:
        try:
            usage = token_usage(self.state)
            status_bar = self.query_one("#status-bar", StatusBarWidget)
            status_bar.set_environment_status(
                health=self._backend_health,
                sandbox_backend=self._sandbox_backend,
                workspace=self._workspace_status,
            )
            status_bar.update_status(
                self.state,
                usage,
                sandbox_warning=self._sandbox_warning or None,
            )
        except Exception:
            pass

    def _refresh_file_explorer(self) -> None:
        try:
            self.query_one("#file-explorer", FileExplorer).refresh_tree()
        except Exception:
            pass
