# r105 Architecture

## System Overview

r105 is a rich terminal AI assistant built with the [Textual](https://textual.textualize.io/) TUI framework. It connects to any OpenAI-compatible API (OpenAI, Ollama, vLLM, Groq, llama-router, etc.) and provides an interactive chat interface with streaming responses, local tool execution, and slash commands.

```
┌──────────────┐     HTTP/SSE      ┌──────────────┐     HTTP      ┌──────────────┐
│              │ ◄──────────────► │              │ ◄───────────► │              │
│   r105 TUI   │    (REST + SSE)   │ llama-router │   (OpenAI API) │ llama-server │
│  (Textual)   │                   │  (FastAPI)   │                │  (llama.cpp) │
│              │                   │              │                │              │
└──────┬───────┘                   └──────────────┘                └──────────────┘
       │
       │ Local execution
       ▼
┌──────────────┐
│  Tool Runner │
│  (subprocess │
│   sandbox)   │
└──────────────┘
```

### Component Roles

| Component | Role |
|-----------|------|
| **r105 TUI** | Textual app: chat screen, command palette, file explorer, streaming display |
| **RouterClient** | HTTP client: sends chat requests, receives SSE streams, manages history |
| **Tool Runner** | Executes LLM-requested tools (Python, file I/O, web search, etc.) in sandboxed subprocesses |
| **llama-router** | FastAPI middleware: request classification, profile selection, response critique |
| **llama-server** | llama.cpp inference server: model execution, token generation |

## Data Flow

### Chat Message Flow

```
User types message
       │
       ▼
┌─────────────────┐
│ ChatInput        │  Textual widget, emits ChatSubmitted message
│ (TextArea)      │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ ChatScreen       │  on_chat_input_chat_submitted()
│ _send_message() │  ┌─ Slash command? → handle_slash_command()
│                 │  └─ Normal message? ↓
└────────┬────────┘
         │
         ▼
┌─────────────────┐     ┌──────────────────┐
│ RouterClient     │────►│ async_send_       │  Builds payload, sends POST
│                 │     │ streaming()       │  SSE: stream → on_chunk(token)
└────────┬────────┘     └──────────────────┘
         │
         │ ChatResult (content + tool_calls)
         ▼
┌─────────────────┐
│ Tool Loop        │  while result.tool_calls:
│                 │    1. execute_tool_call() per tool (parallel via asyncio.gather)
│                 │    2. Append tool results to history
│                 │    3. async_continue() → next ChatResult
│                 │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ ChatView         │  VirtualScroll: add_user(), stream_chunk(), add_assistant()
│ (VirtualScroll)  │  Panels for tool calls, results, errors
└─────────────────┘
```

### Streaming Path

```
POST /v1/chat/completions (stream: true)
       │
       ▼
llama-router → StreamingResponse(text/event-stream)
       │
       ▼
RouterClient._stream_sse()
  ┌─ httpx.AsyncClient.stream("POST", ...)
  ├─ non-2xx before streaming → RouterAPIError (transient 5xx retried
  │   with exponential backoff until the first byte arrives)
  ├─ async for line in aiter_lines():
  │   ├─ "event: ..." → tracked; "event: error" raises RouterAPIError
  │   ├─ "data: {...}" → parse JSON (malformed frames skipped + counted)
  │   ├─ delta.content → on_chunk(token)
  │   └─ delta.tool_calls → accumulate by index
  └─ Return ChatResult(accumulated content, tool_calls)
```

## Component Tree (TUI)

```
R105App (App)
└── ChatScreen (Screen)
    ├── Static (#r105-header)          — model, profile, context usage
    ├── Horizontal (#main-content)
    │   ├── ChatView (#chat-view)      — virtualized chat messages, streaming
    │   └── Vertical (#right-pane)
    │       └── FileExplorer           — workspace directory tree
    ├── CommandPalette                 — fuzzy-slash-command picker (hidden by default)
    ├── ChatInput (#chat-input)        — TextArea with slash-mode, history nav
    └── StatusBarWidget (#status-bar)  — busy indicator, token usage, sandbox warnings
```

### Key TUI Widgets

| Widget | Type | Purpose |
|--------|------|---------|
| `ChatView` | `VerticalScroll` subclass | Virtualized transcript: full history as records, viewport window materialized as widgets. Debounced streaming via `stream_chunk()`; collapsible thinking panels and long tool results. |
| `ChatInput` | `TextArea` subclass | Multi-line input. Slash mode (`/`) triggers fuzzy palette. History nav with ↑/↓. |
| `CommandPalette` | `Static` subclass | Filtered list of slash commands with fuzzy matching. |
| `FileExplorer` | `DirectoryTree` subclass | Async-lazy-loaded directory tree of the workspace. |
| `StatusBarWidget` | `Static` subclass | Shows profile, token usage, busy/streaming state, and sandbox-downgrade warnings. |

## Key Patterns

### `@work(exclusive=True)` — Cancellation Safety

`ChatScreen._send_message()` is decorated with `@work(exclusive=True)`. When a new message is submitted while a previous send is still running, Textual cancels the old worker by raising `asyncio.CancelledError` inside the coroutine.

Since `CancelledError` inherits from `BaseException` (not `Exception`), bare `except Exception` clauses do NOT catch it. The `finally` blocks in `_send_message()` clean up UI state (clear busy indicator, stop streaming) regardless of cancellation.

```python
@work(exclusive=True)
async def _send_message(self, message: str) -> None:
    try:
        result = await self.client.async_send_streaming(...)
    except Exception as exc:
        ...  # does NOT catch CancelledError — it propagates normally
    finally:
        chat_view.finish_streaming()  # always runs
        status_bar.clear_busy()
```

### Shared `httpx.AsyncClient` — Connection Pooling

`ChatScreen` creates a single `httpx.AsyncClient` instance (`self._http`) and passes it to every `RouterClient` method via the `client` parameter. This enables HTTP connection reuse across the session lifetime.

```python
class ChatScreen(Screen):
    def __init__(self, ...):
        self._http = httpx.AsyncClient()

    async def on_unmount(self) -> None:
        await self._http.aclose()  # clean shutdown
```

### `asyncio.to_thread()` — Non-Blocking Tool Execution

Tool functions (`execute_python`, `write_file`, `web_search`, etc.) are synchronous. They are run in the default thread pool executor to avoid blocking the TUI event loop:

```python
async def _exec_one(tc: dict[str, Any]) -> dict[str, Any]:
    return await asyncio.to_thread(execute_tool_call, tc, self.workspace)
```

### Parallel Tool Execution with `return_exceptions=True`

When the LLM requests multiple independent tool calls (e.g., `web_search` + `web_fetch`), they run concurrently via `asyncio.gather`. `return_exceptions=True` prevents one tool failure from cancelling the entire batch:

```python
tool_results = await asyncio.gather(
    *[_exec_one(tc) for tc, _name, _args_str in signatures],
    return_exceptions=True,
)
```

### `async_continue()` — Protocol-Compliant Tool Loop

After tool results are appended to history, the model should continue directly — no intervening user message. `async_continue()` sends `state.history` as-is:

```python
# Correct: history contains user → assistant(tool_calls) → tool → tool → ...
result = await self.client.async_continue(self.state, tools, self._http)

# Wrong (old pattern): injects artificial user message that confuses routing
result = await self.client.async_send("Tool results received. Continue.", ...)
```

## Configuration Flow

```
~/.config/r105/config.json          CLI arguments (--profile, --theme, etc.)
         │                                    │
         ▼                                    │
  ensure_config()                             │
  load_state_overrides()                      │
         │                                    │
         ▼                                    ▼
  ChatState ──────────────────────────────────┘
         │
          ├── profile, quality → router request payload metadata
          ├── model, max_tokens → top-level request fields
         ├── theme → TUI theme (applied instantly via app.apply_theme)
         ├── auto_compact → triggers compaction at >80% context
         ├── cache_prompt → optional llama.cpp prompt-prefix reuse
         ├── usage metadata → exact backend totals when returned; local fallback carries confidence
         ├── active_skills → loaded as system messages
         └── history → conversation state (mutated by RouterClient methods)

  save_config() writes persistent keys (theme, auto_compact, cache_prompt) back to config.json
```

### Config File Format

```json
{
  "theme": "dracula",
  "workspace": "/home/user/r105-workspace",
  "skills_dir": "/home/user/.config/r105/skills",
  "quality": "best",
  "auto_compact": false,
  "cache_prompt": false,
  "url": "http://127.0.0.1:8010"
}
```

## Skills System

Skills are markdown files in `~/.config/r105/skills/` that are injected as system messages into the conversation. They support `{param}` placeholder substitution.

```
/skill use code-reviewer language=python style=strict
```

```
~/.config/r105/skills/code-reviewer.md:
  You are a {language} code reviewer.
  Apply {style} standards.
```

When loaded, the system message becomes:
```
Active skill: code-reviewer
You are a python code reviewer.
Apply strict standards.
```

Skills are read by `r105/skills.py::read_skill()` and converted to system messages by `r105/skills.py::skill_messages()`.

## Tools

Tools are registered on a decorator-based `ToolRegistry` (unified with plugins via `ComponentRegistry`) in `r105/registry.py`, with implementations split into `r105/tools.py` (executor/dispatch), `r105/tools_fs.py`, `r105/tools_web.py` (`web_search`/`web_fetch`), and `r105/tools_math.py`. Dispatch adapts to handler arity (`call_tool_handler()`), so pure tools declare `(arguments)` or `()` instead of the full `(arguments, workspace_dir)` contract. Slash commands take a single `CommandContext` dataclass. The chat tool loop's pure mechanics (signature parsing, repeat dedup, per-tool timeouts) live in `r105/tool_loop.py`; the TUI screen keeps only orchestration. Backends live in `r105/backends/` (`base`/`direct`/`router`) with wire format in `r105/payload.py` and streaming in `r105/sse.py` (`r105/client.py` re-exports for compatibility). `TOOL_DEFINITIONS` exposes the JSON Schema list for the LLM; `execute_tool_call()` validates arguments, enforces the permission posture, and dispatches built-ins → plugins → MCP. Tool execution runs in a thread pool via `asyncio.to_thread()`. Python code runs in the auto-detected sandbox backend (`nsjail` > `bwrap` > `docker` > `rlimit` > `none`); `detect_backend_with_reason()` / `get_fallback_reason()` report downgrades to weaker backends.

## Directory Layout

```
r105/
├── backends/           # Backend clients: base.py (BaseClient), direct.py, router.py
├── cli.py              # CLI entry point (argparse)
├── client.py           # Backwards-compat facade re-exporting backends/payload/sse
├── commands.py         # Slash command handlers (COMMAND_DISPATCH, CommandContext)
├── commands_format.py  # Presentation helpers for slash commands
├── config.py           # Config file read/write + validation (~/.config/r105/config.json)
├── constants.py        # Shared constants + ToolProfile per-tool budgets
├── errors.py           # Typed exception hierarchy (R105Error, ...)
├── logging.py          # Structured JSON-lines logging
├── mcp_client.py       # MCP stdio/SSE clients + manager
├── model_catalog.py    # Model family/context resolution + overrides
├── payload.py          # Chat payload building + response parsing (wire format)
├── plugins.py          # Plugin loading, validation, registry
├── providers.py        # Native Anthropic/Gemini adapters
├── registry.py         # ComponentRegistry + ToolRegistry + arity-adapting dispatch
├── sandbox.py          # Sandbox backends (nsjail/bwrap/docker/rlimit/none)
├── sessions.py         # Session save/load/diff + conversation export
├── skills.py           # Skill file loading + system-message injection
├── sse.py              # SSE streaming core (stream_sse)
├── state.py            # ChatState, ChatResult, TokenUsage
├── tool_loop.py        # Tool-loop mechanics: signatures, dedup, timeouts
├── tools.py            # Tool execution/dispatch (registry lives in registry.py)
├── tools_fs.py         # File/workspace tool helpers
├── tools_math.py       # Safe math evaluator + unit conversion
├── tools_web.py        # Web search/fetch tool implementations + web helpers
├── themes/             # TCSS theme files (r105, dracula, solarized-dark, high-contrast)
└── tui/
    ├── app.py          # r105App (Textual App)
    ├── screens/
    │   ├── chat.py           # ChatScreen — main interactive screen
    │   ├── help_screen.py    # HelpScreen — command reference modal
    │   ├── history_screen.py # HistoryScreen — searchable transcript modal
    │   └── tools_screen.py   # ToolsScreen — tool-call inspector modal (Ctrl+T)
    └── widgets/
        ├── chat_view.py        # Virtualized chat display + streaming + collapsibles
        ├── input_area.py       # ChatInput — multi-line with slash mode
        ├── command_palette.py  # Fuzzy slash-command picker
        ├── diff_view.py        # File-diff approve/reject widget
        ├── file_explorer.py    # Workspace directory tree
        └── status_bar.py       # Context usage + busy indicator + sandbox warnings
```
