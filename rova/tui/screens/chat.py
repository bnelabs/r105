"""Main chat screen — the primary interactive screen."""

from __future__ import annotations

import json
from pathlib import Path

import httpx
from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.screen import Screen
from textual.widgets import Static

from rova.client import RouterClient
from rova.state import ChatState, DEFAULT_MODEL, token_usage
from rova.commands import handle_slash_command
from rova.tools import execute_tool_call, TOOL_DEFINITIONS
from rova.tui.widgets.chat_view import ChatView
from rova.tui.widgets.input_area import ChatInput
from rova.tui.widgets.sidebar import Sidebar
from rova.tui.widgets.status_bar import StatusBarWidget


class ChatScreen(Screen[None]):
    """The main chat screen with chat history, input, and sidebar."""

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
        """Clean up the shared HTTP client when the screen is removed."""
        # Schedule async close — Textual will handle this
        import asyncio
        asyncio.create_task(self._http.aclose())

    def compose(self) -> ComposeResult:
        yield Static(self._render_header(), id="rova-header")
        with Horizontal(id="main-content"):
            yield ChatView(id="chat-view")
            yield Sidebar(id="sidebar")
        yield ChatInput(id="chat-input")
        yield StatusBarWidget(id="status-bar")

    def on_mount(self) -> None:
        self._refresh_all()

    # --- Input handling ---------------------------------------------------

    def on_chat_input_submitted(self, event: ChatInput.Submitted) -> None:
        text = event.value.strip()
        if not text:
            return

        chat_view = self.query_one("#chat-view", ChatView)

        if text.startswith("/"):
            result = handle_slash_command(text, self.state, self.client, self.workspace)
            if text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
        else:
            chat_view.add_user(text)
            self._send_message(text)

        self._refresh_all()

    # --- Message sending & tool loop --------------------------------------

    @work(exclusive=True)
    async def _send_message(self, message: str) -> None:
        chat_view = self.query_one("#chat-view", ChatView)
        tools = TOOL_DEFINITIONS if self.state.profile == "tool_agent" else None

        try:
            result = await self.client.async_send(message, self.state, tools, self._http)
        except Exception as exc:
            chat_view.add_error(f"Send failed: {exc}")
            self._refresh_all()
            return

        # Tool loop: execute tool calls locally and continue
        max_iterations = 10
        iteration = 0
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

                tool_result_msg = execute_tool_call(tc, self.workspace)
                result_content = tool_result_msg.get("content", "")
                chat_view.add_tool_result(result_content)

                self.state.history.append({
                    "role": "assistant",
                    "content": result.content or "",
                    "tool_calls": result.tool_calls,
                })
                self.state.history.append(tool_result_msg)

            try:
                result = await self.client.async_send(
                    "Tool results received. Continue or provide final answer.",
                    self.state,
                    tools,
                    self._http,
                )
            except Exception as exc:
                chat_view.add_error(f"Tool loop error: {exc}")
                self._refresh_all()
                return

        chat_view.add_assistant(result.content, result.wall_seconds)
        self._refresh_all()

    # --- Refresh helpers --------------------------------------------------

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
            f"skills={','.join(self.state.active_skills) if self.state.active_skills else 'none'}"
        )

    def _refresh_all(self) -> None:
        self._refresh_header()
        self._refresh_sidebar()
        self._refresh_status_bar()

    def _refresh_header(self) -> None:
        try:
            self.query_one("#rova-header", Static).update(self._render_header())
        except Exception:
            pass

    def _refresh_sidebar(self) -> None:
        try:
            self.query_one("#sidebar", Sidebar).refresh_state(self.state, self.workspace)
        except Exception:
            pass

    def _refresh_status_bar(self) -> None:
        try:
            usage = token_usage(self.state)
            self.query_one("#status-bar", StatusBarWidget).update_status(self.state, usage)
        except Exception:
            pass
