# Rova Architecture

## System Overview

Rova is a rich terminal frontend for [llama-router](https://github.com/komedi/llama-router), built with the [Textual](https://textual.textualize.io/) TUI framework. It provides an interactive chat interface with streaming responses, local tool execution, slash commands, and RAG management.

```
┌──────────────┐     HTTP/SSE      ┌──────────────┐     HTTP      ┌──────────────┐
│              │ ◄──────────────► │              │ ◄───────────► │              │
│   Rova TUI   │    (REST + SSE)   │ llama-router │   (OpenAI API) │ llama-server │
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
| **Rova TUI** | Textual app: chat screen, command palette, file explorer, streaming display |
| **RouterClient** | HTTP client: sends chat requests, receives SSE streams, manages history |
| **Tool Runner** | Executes LLM-requested tools (Python, file I/O, web search, etc.) in sandboxed subprocesses |
| **llama-router** | FastAPI middleware: request classification, profile selection, RAG, response critique |
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
│ ChatView         │  RichLog: add_user(), stream_chunk(), add_assistant()
│ (RichLog)       │  Panels for tool calls, results, errors
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
  ├─ async for line in aiter_lines():
  │   ├─ "data: {...}" → parse JSON
  │   ├─ delta.content → on_chunk(token)
  │   └─ delta.tool_calls → accumulate by index
  └─ Return ChatResult(accumulated content, tool_calls)
```

## Component Tree (TUI)

```
RovaApp (App)
└── ChatScreen (Screen)
    ├── Static (#rova-header)          — model, profile, context usage
    ├── Horizontal (#main-content)
    │   ├── ChatView (#chat-view)      — RichLog: chat messages, streaming
    │   └── Vertical (#right-pane)
    │       └── FileExplorer           — workspace directory tree
    ├── CommandPalette                 — fuzzy-slash-command picker (hidden by default)
    ├── ChatInput (#chat-input)        — TextArea with slash-mode, history nav
    └── StatusBarWidget (#status-bar)  — busy indicator, token usage
```

### Key TUI Widgets

| Widget | Type | Purpose |
|--------|------|---------|
| `ChatView` | `RichLog` subclass | Append-only chat log. Line-buffered streaming via `stream_chunk()`. |
| `ChatInput` | `TextArea` subclass | Multi-line input. Slash mode (`/`) triggers fuzzy palette. History nav with ↑/↓. |
| `CommandPalette` | `Static` subclass | Filtered list of slash commands with fuzzy matching. |
| `FileExplorer` | `DirectoryTree` subclass | Async-lazy-loaded directory tree of the workspace. |
| `StatusBarWidget` | `Static` subclass | Shows model, profile, token usage, and busy state. |

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
~/.config/rova/config.json          CLI arguments (--profile, --theme, etc.)
         │                                    │
         ▼                                    │
  ensure_config()                             │
  load_state_overrides()                      │
         │                                    │
         ▼                                    ▼
  ChatState ──────────────────────────────────┘
         │
         ├── profile, quality, max_tokens → request payload metadata
         ├── theme → TUI theme (applied instantly via app.apply_theme)
         ├── auto_compact → triggers compaction at >80% context
         ├── active_skills → loaded as system messages
         └── history → conversation state (mutated by RouterClient methods)

  save_config() writes persistent keys (theme, auto_compact) back to config.json
```

### Config File Format

```json
{
  "theme": "dracula",
  "workspace": "/home/user/rova-workspace",
  "skills_dir": "/home/user/.config/rova/skills",
  "quality": "best",
  "auto_compact": false,
  "url": "http://127.0.0.1:8010"
}
```

## Skills System

Skills are markdown files in `~/.config/rova/skills/` that are injected as system messages into the conversation. They support `{param}` placeholder substitution.

```
/skill use code-reviewer language=python style=strict
```

```
~/.config/rova/skills/code-reviewer.md:
  You are a {language} code reviewer.
  Apply {style} standards.
```

When loaded, the system message becomes:
```
Active skill: code-reviewer
You are a python code reviewer.
Apply strict standards.
```

Skills are read by `rova/skills.py::read_skill()` and converted to system messages by `rova/client.py::_skill_messages()`.

## Tools

Tools are defined in `rova/tools.py` as a `TOOL_DEFINITIONS` list (JSON Schema for the LLM) paired with handler functions dispatched by `execute_tool_call()`. Tool execution runs in a thread pool via `asyncio.to_thread()`. The Python sandbox uses `resource.setrlimit()` for basic resource limits (256MB memory, 25s CPU) with a stripped environment.

## Directory Layout

```
rova/
├── cli.py              # CLI entry point (argparse)
├── client.py           # RouterClient — sync + async HTTP
├── commands.py         # Slash command handlers
├── config.py           # Config file read/write (~/.config/rova/config.json)
├── skills.py           # Skill file loading
├── state.py            # ChatState, ChatResult, TokenUsage
├── tools.py            # Tool definitions + execution
├── themes/             # TCSS theme files (rova, dracula, solarized-dark, high-contrast)
└── tui/
    ├── app.py          # RovaApp (Textual App)
    ├── screens/
    │   └── chat.py     # ChatScreen — main interactive screen
    └── widgets/
        ├── chat_view.py        # RichLog chat display + streaming
        ├── input_area.py       # ChatInput — multi-line with slash mode
        ├── command_palette.py  # Fuzzy slash-command picker
        ├── file_explorer.py    # Workspace directory tree
        └── status_bar.py       # Context usage + busy indicator
```
