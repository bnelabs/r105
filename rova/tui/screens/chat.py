"""Main chat screen — the primary interactive screen."""

from __future__ import annotations

import json
from pathlib import Path

from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.screen import Screen
from textual.widgets import Static

from rova.client import RouterClient
from rova.state import (
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
    DEFAULT_MODEL,
    token_usage,
)
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

    def compose(self) -> ComposeResult:
        yield Static(self._render_header(), id="rova-header")
        with Horizontal(id="main-content"):
            yield ChatView(id="chat-view")
            yield Sidebar(id="sidebar")
        yield ChatInput(id="chat-input")
        yield StatusBarWidget(id="status-bar")

    def on_mount(self) -> None:
        self._refresh_header()
        self._refresh_sidebar()
        self._refresh_status_bar()

    # --- Input handling -------------------------------------------------------

    def on_chat_input_submitted(self, event: ChatInput.Submitted) -> None:
        text = event.value.strip()
        if not text:
            return

        chat_view = self.query_one("#chat-view", ChatView)

        if text.startswith("/"):
            result = handle_slash_command(
                text, self.state, self.client, self.workspace
            )
            if text in {"/exit", "/quit"}:
                self.app.exit()
                return
            chat_view.add_system(result)
        else:
            chat_view.add_user(text)
            self._send_message(text)

        self._refresh_header()
        self._refresh_sidebar()
        self._refresh_status_bar()

    # --- Message sending & tool loop ------------------------------------------

    @work(exclusive=True)
    async def _send_message(self, message: str) -> None:
        chat_view = self.query_one("#chat-view", ChatView)

        # Build the tool-using send: if tool_agent profile or explicit tools,
        # pass tool definitions so the LLM can call them.
        tools = TOOL_DEFINITIONS if self.state.profile == "tool_agent" else None

        try:
            result = await self._call_send(message, tools)
        except Exception as exc:
            chat_view.add_error(f"error: {exc}")
            return

        # Tool loop: while the model returns tool calls, execute and continue
        max_iterations = 10
        iteration = 0
        while result.tool_calls and iteration < max_iterations:
            iteration += 1
            for tc in result.tool_calls:
                func = tc.get("function", {})
                name = func.get("name", "unknown")
                args_str = json.dumps(
                    func.get("arguments", {}), indent=2, sort_keys=True
                )
                chat_view.add_tool_call(name, args_str)

                tool_result_msg = execute_tool_call(tc, self.workspace)
                result_content = tool_result_msg.get("content", "")
                chat_view.add_tool_result(result_content)

                # Append the assistant message with tool_calls and the tool result
                self.state.history.append({
                    "role": "assistant",
                    "content": result.content or "",
                    "tool_calls": result.tool_calls,
                })
                self.state.history.append(tool_result_msg)

            # Send follow-up with tool results
            followup_msg = "Tool results received. Continue or provide final answer."
            try:
                result = await self._call_send(followup_msg, tools)
            except Exception as exc:
                chat_view.add_error(f"error during tool loop: {exc}")
                return

        chat_view.add_assistant(result.content, result.wall_seconds)
        self._refresh_sidebar()
        self._refresh_status_bar()

    async def _call_send(self, message: str, tools: list | None) -> "ChatResult":
        from rova.client import ChatResult
        import asyncio
        return await asyncio.to_thread(self.client.send, message, self.state, tools)

    # --- Refresh helpers ------------------------------------------------------

    def _render_header(self) -> str:
        usage = token_usage(self.state)
        return (
            f"Rova — local router console\n"
            f"model={DEFAULT_MODEL}  "
            f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)  "
            f"url={self.client.base_url}\n"
            f"profile={self.state.profile or 'auto'}  "
            f"rag={self.state.rag if self.state.rag is not None else 'auto'}  "
            f"quality={self.state.quality or 'auto'}  "
            f"skills={','.join(self.state.active_skills) if self.state.active_skills else 'none'}"
        )

    def _refresh_header(self) -> None:
        try:
            header = self.query_one("#rova-header", Static)
            header.update(self._render_header())
        except Exception:
            pass

    def _refresh_sidebar(self) -> None:
        try:
            sidebar = self.query_one("#sidebar", Sidebar)
            sidebar.refresh_state(self.state, self.workspace)
        except Exception:
            pass

    def _refresh_status_bar(self) -> None:
        try:
            bar = self.query_one("#status-bar", StatusBarWidget)
            usage = token_usage(self.state)
            bar.update_status(self.state, usage)
        except Exception:
            pass
