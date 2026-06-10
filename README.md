<p align="center">
  <img src="ROVA.png" alt="Rova" width="600">
</p>

# Rova — Rapid On-demand Virtual Assistant

Rova is a rich terminal frontend for [llama-router](https://github.com/komedi/llama-router), built on [Textual](https://textual.textualize.io/). It provides an interactive chat TUI with streaming SSE responses, a slash-command system, fuzzy command palette, local tool execution, secure sandboxing, MCP integration, plugin extensibility, session persistence, and multiple themes.

Rova can also connect to any OpenAI-compatible API (Ollama, vLLM, Groq, etc.) when you don't need llama-router's profile routing or RAG features.

<p align="center">
  <img src="https://img.shields.io/pypi/v/rova?color=cba6f7" alt="PyPI">
  <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13-blue" alt="Python">
  <img src="https://img.shields.io/badge/tests-165%20passed-brightgreen" alt="Tests">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
</p>

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Quick Start](#quick-start)
- [CLI Usage](#cli-usage)
- [Backends](#backends)
- [Interactive TUI](#interactive-tui)
  - [Layout](#layout)
  - [Command Palette](#command-palette)
  - [Slash Commands](#slash-commands)
  - [Keybindings](#keybindings)
- [Built-in Tools](#built-in-tools)
- [Sandbox & Security](#sandbox--security)
- [Skills](#skills)
- [Plugins](#plugins)
- [MCP — Model Context Protocol](#mcp--model-context-protocol)
- [Sessions & Export](#sessions--export)
- [RAG — Retrieval-Augmented Generation](#rag--retrieval-augmented-generation)
- [Themes](#themes)
- [Auto-Compaction](#auto-compaction)
- [Configuration](#configuration)
- [Docker](#docker)
- [Updating & Uninstalling](#updating--uninstalling)
- [Testing](#testing)
- [Architecture](#architecture)
- [Documentation](#documentation)
- [License](#license)

---

## Prerequisites

- **Python 3.12+**
- **llama-router** running on `http://127.0.0.1:8010` (or set `ROVA_URL` / `--url`)

---

## Installation

### pipx (recommended)

```sh
pipx install rova
```

### Homebrew (macOS / Linux)

```sh
brew install bnelabs/tap/rova
```

### Standalone binary

Pre-built single-file executables are attached to [GitHub Releases](https://github.com/bnelabs/rova/releases) for Linux and macOS. Download, `chmod +x`, and run — no Python install needed.

### From source

```sh
git clone https://github.com/bnelabs/rova.git
cd rova
python -m venv .venv
.venv/bin/pip install -e .
ln -sf "$(pwd)/bin/rova" ~/.local/bin/rova
```

### Docker

```sh
docker build -t rova .
docker run -it --rm \
  -v ~/.config/rova:/root/.config/rova \
  -v ~/rova-workspace:/root/rova-workspace \
  rova chat
```

For a quick-start stack with llama-router, use the included `docker-compose.yml`:

```sh
docker compose up -d llama-router
docker compose run rova chat
```

---

## Quick Start

```sh
# Launch the TUI (requires llama-router on http://127.0.0.1:8010)
rova chat

# One-shot: ask a question and get a response without the TUI
rova send "explain quicksort in 3 sentences"

# Connect to a different router or any OpenAI-compatible API
rova --url http://my-router:8010 chat
OPENAI_API_KEY=sk-... rova --url https://api.openai.com/v1 chat
```

---

## CLI Usage

Rova supports both one-shot prompts and management commands at the command line.

```sh
# Interactive chat (default, opens TUI)
rova chat

# One-shot prompt
rova send "explain quicksort in 3 sentences"

# Check router health
rova health

# List available task profiles
rova profiles

# Ingest documents for RAG
rova ingest /path/to/docs https://example.com/page

# Search the RAG index
rova search "query terms"

# Load a saved session on startup
rova --session my-session chat

# Version info
rova --version
```

### CLI Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--url` | `http://127.0.0.1:8010` | Router or API base URL |
| `--workspace` | `~/rova-workspace` | Workspace directory for file tools |
| `--skills-dir` | `./skills` | Directory containing skill `.md` files |
| `--plugins-dir` | `~/.config/rova/plugins` | Directory for custom tool plugins |
| `--profile` | auto | Force a router task profile |
| `--model` | auto | Override model selection |
| `--quality` | auto | Quality hint: `fast`, `balanced`, or `best` |
| `--rag` | off | Enable RAG for chat requests |
| `--max-tokens` | auto | Override max output tokens |
| `--json` | off | Request JSON-object responses |
| `--backend` | auto | `router` (profiles+RAG) or `direct` (any OpenAI-compatible) |
| `--session` | — | Load a saved session on startup |
| `--version` | — | Print version and exit |

---

## Backends

Rova auto-detects the best backend:

1. **`ROVA_URL` set → llama-router** — full profile routing, RAG, model selection
2. **`OPENAI_API_KEY` set → direct** — any OpenAI-compatible API (OpenAI, Ollama, vLLM, Groq)
3. **Otherwise** — checks local Ollama, falls back to direct

Override with `--backend router` or `--backend direct`.

### Backend Capabilities

| Feature | Router backend | Direct backend |
|---------|:---:|:---:|
| Profiles (task routing) | ✅ | — |
| RAG (document retrieval) | ✅ | — |
| Quality hints | ✅ | — |
| Model listing/switching | ✅ | ✅ |
| Tool calling | ✅ | ✅ |
| SSE streaming | ✅ | ✅ |
| Conversation compaction | ✅ | ✅ |

---

## Interactive TUI

### Layout

```
┌─────────────────────────────────────────────┐
│  Header: model, profile, context usage      │
├───────────────────────┬─────────────────────┤
│                       │                     │
│    Chat View          │   File Explorer     │
│    (messages,         │   (workspace tree)  │
│     streaming)        │                     │
│                       │                     │
├───────────────────────┴─────────────────────┤
│  Command Palette (hidden by default)        │
├─────────────────────────────────────────────┤
│  Chat Input (multi-line, history, autocomplete) │
├─────────────────────────────────────────────┤
│  Status Bar (profile, context, busy state)  │
└─────────────────────────────────────────────┘
```

### Command Palette

Press `/` to open the interactive command palette with:

- **Fuzzy filtering** — type any part of a command name to narrow
- **Arrow-key navigation** — ↑/↓ to browse, Enter to select
- **Tab autocomplete** — fills in the matching command
- **Category grouping** — Chat, RAG, Skills, Sessions, Plugins, MCP, Workspace, System
- **Escape to dismiss**

### Slash Commands

#### Chat

| Command | Description |
|---------|-------------|
| `/state` | Show active settings (profile, RAG, quality, tokens, model) |
| `/tokens` | Show estimated context token usage and capacity |
| `/model [name]` | Show current model, list available, or switch models (persistent) |
| `/history` | Show compact transcript preview of last messages |
| `/clear` | Clear all conversation history |
| `/compact` | Summarize conversation history and continue |
| `/profile <name>` | Force a router task profile: `simple`, `coding`, `complex_reasoning`, `long_context_qa`, `tool_agent`, `creative`, `strict_json` |
| `/quality fast\|balanced\|best` | Set quality hint metadata |
| `/json [on\|off]` | Toggle JSON object response mode |
| `/max <tokens>` | Override max output tokens |
| `/autocompact [on\|off]` | Toggle auto-compaction at 80% context threshold |
| `/copy` | Copy last assistant message to system clipboard |

#### RAG (Retrieval-Augmented Generation)

| Command | Description |
|---------|-------------|
| `/rag [on\|off]` | Toggle RAG metadata for chat requests |
| `/rag ingest <path-or-url>...` | Ingest local files/directories or URLs into the index |
| `/rag search <query>` | Search the active RAG index |
| `/rag list` | List all indexed documents |
| `/rag delete <id>` | Remove a document from the RAG index |
| `/rag update <path>` | Re-index specific paths |

#### Skills

| Command | Description |
|---------|-------------|
| `/skills` | List available skill files in the skills directory |
| `/skill use <name> [key=val...]` | Activate a skill with optional parameters |
| `/skill drop <name>` | Deactivate one skill |
| `/skill clear` | Deactivate all skills |
| `/skill show <name>` | Print the raw content of a skill file |

#### Sessions & Export

| Command | Description |
|---------|-------------|
| `/session save <name>` | Save current conversation to `~/.config/rova/sessions/` |
| `/session load <name>` | Load and restore a previously saved session |
| `/session list` | List all saved sessions with previews and timestamps |
| `/session delete <name>` | Delete a saved session |
| `/export markdown` | Export conversation as Markdown |
| `/export json` | Export conversation as JSON |
| `/export html` | Export conversation as a styled HTML page |

#### Plugins & MCP

| Command | Description |
|---------|-------------|
| `/plugin list` | List loaded custom tool plugins |
| `/plugin reload` | Reload plugins from disk |
| `/mcp list` | List connected MCP servers and their tool counts |
| `/mcp tools <server>` | List tools exposed by a specific MCP server |

#### Workspace & System

| Command | Description |
|---------|-------------|
| `/workspace` | Show workspace directory and list generated files |
| `/preview <filename>` | Preview a workspace file's contents |
| `/theme <name>` | Switch theme: `rova`, `dracula`, `solarized-dark`, `high-contrast` |
| `/health` | Check llama-router and upstream model health |
| `/profiles` | List available router task profiles |
| `/help` | Show the full command reference |
| `/exit` | Quit Rova |

### Keybindings

| Key | Context | Action |
|-----|---------|--------|
| `Enter` | Normal mode | Submit message |
| `Enter` | Slash mode | Execute or select command |
| `Shift+Enter` | Any | Insert newline |
| `↑` / `↓` | Normal mode | Navigate input history (up to 200 entries) |
| `↑` / `↓` | Slash mode | Navigate command palette |
| `Tab` | Slash mode | Fuzzy-autocomplete command |
| `Escape` | Slash mode | Dismiss command palette, clear input |
| `Ctrl+R` | Any | Open history browser with fuzzy search |
| `Ctrl+S` | History screen | Quick-save current session |
| `Ctrl+Y` | Any | Copy last assistant response to clipboard |
| `Ctrl+W` | Input | Delete word backward |
| `Ctrl+U` | Input | Clear line |
| `Ctrl+A` | Input | Jump to line start |
| `Ctrl+E` | Input | Jump to line end |
| `Ctrl+K` | Input | Kill to end of line |
| `F1` / `Ctrl+H` | Any | Show help screen |
| `Ctrl+Q` / `Ctrl+C` | Any | Quit |

---

## Built-in Tools

Rova provides 9 local tools the LLM can call. All tools are validated before execution with argument size caps, SSRF prevention, and path traversal hardening. `execute_python` runs in a sandboxed environment with no network or filesystem access.

| Tool | Description | Sandbox Profile |
|------|-------------|----------------|
| `execute_python` | Sandboxed Python execution (256MB RAM, 25s CPU, seccomp) | No network, no filesystem |
| `write_file` | Write/update files in workspace — generates unified diffs on edit | Filesystem write |
| `read_file` | Read file contents (max 50MB, with `<tool_output>` tags) | Filesystem read |
| `list_files` | List directory contents | Filesystem read |
| `web_search` | Search via DuckDuckGo HTML (no API key) | Network access |
| `web_fetch` | Fetch URL content (HTML stripped to text) | Network access |
| `get_time` | Current system time in ISO 8601 | Minimal isolation |
| `calculate` | Safe arithmetic expression evaluator | Minimal isolation |
| `system_info` | OS, Python version, CPU count | Minimal isolation |

### Tool Execution Model

- Tools run **synchronously** in a thread pool (`asyncio.to_thread`) to avoid blocking the TUI event loop
- Multiple independent tool calls run **in parallel** via `asyncio.gather`
- The tool loop follows the OpenAI protocol: after tool results are appended to history, `async_continue()` sends the model the full conversation state without injecting an artificial user message
- Output is truncated at 8,000 characters to keep context lean

### File Diffing

When `write_file` modifies an existing file, it generates a unified diff. The TUI displays this in a `DiffView` widget with:

- **Syntax-colored additions** (green) and **deletions** (red)
- **Approve / Reject** buttons — changes are not saved until approved
- **New file detection** — first writes show the file content directly

---

## Sandbox & Security

Rova implements layered security for code execution. The backend is auto-detected: `nsjail` > `bwrap` > `rlimit` > `none` (Windows fallback).

### Sandbox Backends

| Backend | Isolation Level | Requirements |
|---------|:---:|-------------|
| **Nsjail** (strongest) | Linux namespace isolation, seccomp-bpf syscall allowlist, no host filesystem | `nsjail` binary on PATH |
| **Bwrap** | User namespace isolation via bubblewrap, minimal `/dev` bind, conditional network/filesystem per profile | `bwrap` on PATH |
| **RLimit** | Resource limits via `setrlimit()` (256MB RAM, 25s CPU, no child procs) | Unix (not Windows) |
| **Noop** | No isolation — fallback for Windows or explicit config | None |

### Per-Tool Sandbox Profiles

Each tool gets a sandbox profile that specifies exactly what it needs:

| Profile | Network | Filesystem | Write | seccomp |
|---------|:---:|:---:|:---:|:---:|
| `PROFILE_EXECUTE_PYTHON` | No | No | No | Yes |
| `PROFILE_FILE_TOOLS` | No | Yes | Yes | Yes |
| `PROFILE_WEB_TOOLS` | Yes | No | No | Yes |
| `PROFILE_SYSTEM_TOOLS` | No | No | No | No |

### Security Features

- **SSRF prevention** — URL validation blocks private IPv4/IPv6 ranges, link-local addresses, and localhost aliases; DNS resolution is checked against reserved network blocks
- **Path traversal hardening** — symlink-aware path validation ensures all file operations stay within the workspace directory; skill loader blocks `../`, `\\`, and dot-prefixed paths
- **Environment sanitization** — secrets, tokens, API keys, SSH keys, cloud credentials (AWS, GCP, Azure, OpenAI, Anthropic, etc.) are stripped from subprocess environments; only explicitly safe variables are forwarded
- **Argument validation** — code capped at 100KB, file writes at 10MB, file reads at 50MB, search queries at 500 characters

---

## Skills

Skills are reusable Markdown prompt templates stored in `~/.config/rova/skills/`. They support `{param}` placeholder substitution for dynamic content injection.

Skills appear as system messages in the LLM context, persisting across `/clear` but not across sessions (re-activate on each launch).

### Built-in Skills

| Skill | Description |
|-------|-------------|
| `code-check` | Code review assistant — prefers executable snippets, checks syntax and imports |
| `concise` | Forces concise, direct responses with no preamble |
| `deep-review` | Comprehensive analysis covering deliverables, assumptions, edge cases, security |
| `rag-answer` | RAG-aware answering with exact source tag citations |

### Creating a Skill

```markdown
<!-- ~/.config/rova/skills/web-researcher.md -->
When answering questions, follow this process:
1. Break the question into search queries
2. Use web_search for each query
3. Use web_fetch for the top 3 results
4. Synthesize findings with citations (URL + snippet)
5. Note any gaps or uncertainties

Query: {query}
```

```sh
# Activate with parameter substitution
/skill use web-researcher query="how CPUs work"
```

The `{query}` placeholder is replaced with `how CPUs work` before injection.

### Security

Skills are prompt templates, not executable code. The loader blocks path traversal:

```python
if "/" in name or "\\" in name or name.startswith("."):
    return ""  # blocks ../../etc/passwd style attacks
```

Review skill content before activation if it comes from an untrusted source.

---

## Plugins

Custom tools can be loaded from Python files in `~/.config/rova/plugins/`. Each file exposes a `register(registry)` function that adds tools via `registry.add_tool()`.

**Example plugin** (`~/.config/rova/plugins/hello.py`):

```python
def register(registry):
    registry.add_tool(
        name="hello",
        description="Say hello to someone.",
        parameters={
            "name": {"type": "string", "description": "Name to greet."},
        },
        required=["name"],
        handler=lambda args, ws: f"Hello, {args.get('name', 'world')}!",
    )
```

Plugins are auto-discovered on startup. Use `/plugin reload` to reload without restarting.

See [docs/TOOLS.md](docs/TOOLS.md) for the full API reference.

---

## MCP — Model Context Protocol

Rova can connect to external MCP servers (stdio or SSE transport) to access community-built tools without changing Rova's code.

### Configuration

Configure servers in `~/.config/rova/config.json`:

```json
{
  "mcp_servers": [
    {
      "name": "github",
      "transport": "sse",
      "url": "http://127.0.0.1:3001/mcp"
    },
    {
      "name": "filesystem",
      "transport": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
    }
  ]
}
```

MCP tools are namespaced as `mcp_<server>_<tool>` in the LLM's tool definitions, keeping them distinct from built-in and plugin tools.

### Transports

| Transport | How it works |
|-----------|-------------|
| `stdio` | Spawns a subprocess, communicates via JSON-RPC over stdin/stdout |
| `sse` | Connects to an HTTP SSE endpoint (long-lived GET for events, POST for requests) |

---

## Sessions & Export

Conversations can be saved, loaded, and exported in multiple formats. Rova also **auto-saves on exit** as `__autosave__`.

### Session Storage

Sessions are stored as JSON files in `~/.config/rova/sessions/`. Each file contains:

- **Conversation history** (all user, assistant, and tool messages)
- **Session state** (profile, RAG, quality, skills, and parameters)
- **Message count and save timestamp**

### Export Formats

| Format | What you get |
|--------|-------------|
| `markdown` | Styled `.md` with role headers and message numbering |
| `json` | Raw conversation array, portable and machine-readable |
| `html` | Styled HTML page with role-colored messages |

---

## RAG — Retrieval-Augmented Generation

Rova integrates with llama-router's RAG pipeline. When RAG is enabled, router responses include source citations from your ingested documents.

### Workflow

1. **Ingest**: `/rag ingest /path/to/docs https://example.com` — sends documents to llama-router for indexing
2. **Enable**: `/rag on` — enables RAG metadata on subsequent chat requests
3. **Chat**: The router injects relevant context into the LLM prompt
4. **Citations**: Source attributions appear in the chat view with snippet previews

### Management

- `/rag list` — view all indexed documents with IDs
- `/rag search <query>` — search the index directly without a chat
- `/rag delete <id>` — remove a document
- `/rag update <path>` — re-index a path after changes

---

## Themes

Four themes are included. Switch at runtime with `/theme <name>`:

| Theme | Accent | Style |
|-------|--------|-------|
| `rova` (default) | `#cba6f7` mauve | Catppuccin Mocha |
| `dracula` | `#bd93f9` purple | Dracula |
| `solarized-dark` | `#268bd2` blue | Solarized Dark |
| `high-contrast` | `#ffff00` yellow | Accessibility-focused |

Themes are defined as Textual CSS files in `rova/themes/`. The selected theme persists in `~/.config/rova/state.json`.

---

## Auto-Compaction

When conversation context approaches 80% of the model's capacity, Rova can automatically summarize earlier messages to free space. This is controlled by:

- **`/autocompact on|off`** — toggle from within the TUI
- **`auto_compact` field** in `config.json` — persistent default

Compaction uses the `complex_reasoning` profile and keeps the most recent 30% of messages intact.

---

## Configuration

Rova stores configuration in `~/.config/rova/`:

```
~/.config/rova/
├── config.json          # MCP servers, model, sandbox backend, defaults
├── state.json           # Persistent TUI state (theme, profile, quality)
├── sessions/            # Saved conversation sessions (JSON)
│   ├── __autosave__.json
│   └── my-session.json
└── plugins/             # Custom tool Python files
    └── hello.py
```

### Example `config.json`

```json
{
  "model": "gemma-4-12b-it",
  "sandbox_backend": "bwrap",
  "auto_compact": true,
  "theme": "rova",
  "mcp_servers": [
    {
      "name": "github",
      "transport": "sse",
      "url": "http://127.0.0.1:3001/mcp"
    }
  ]
}
```

### Config Merging

CLI arguments override config file values, which override built-in defaults. The config file only stores overrides — matching defaults are omitted to keep the file lean.

### Structured Logging

Debug logs are written to `~/.local/state/rova/log.jsonl` in JSON Lines format. Set `ROVA_LOG_LEVEL=DEBUG` for verbose output.

---

## Docker

### Standalone

```sh
docker build -t rova .
docker run -it --rm \
  -v ~/.config/rova:/root/.config/rova \
  -v ~/rova-workspace:/root/rova-workspace \
  rova chat
```

### Quick-start stack with llama-router

The `docker-compose.yml` starts both rova and llama-router on a shared network:

```sh
docker compose up -d llama-router
docker compose run rova chat
```

The stack mounts `~/.config/rova` and `~/.config/llama-router` for persistent configuration. `ROVA_URL=http://llama-router:8010` is set automatically.

---

## Updating & Uninstalling

### pipx

```sh
pipx upgrade rova          # update
pipx uninstall rova        # remove
```

### Homebrew

```sh
brew upgrade rova          # update
brew uninstall rova        # remove
```

### Standalone binary

Download the latest binary from [GitHub Releases](https://github.com/bnelabs/rova/releases) and replace the old one.

### From source

```sh
cd /path/to/rova
git pull
.venv/bin/pip install -e .

# Uninstall
rm ~/.local/bin/rova
rm -rf /path/to/rova/.venv
```

### Docker

```sh
git pull && docker build -t rova .     # update
docker rmi rova                        # remove image
```

### Config cleanup

None of these methods remove your user-level data. To wipe everything:

```sh
rm -rf ~/.config/rova
rm -rf ~/rova-workspace
```

---

## Testing

```sh
# Install dev dependencies
pip install -e ".[dev]"

# Run full test suite (165+ tests)
python -m pytest tests/ -v

# With coverage
python -m pytest tests/ -v --cov=rova --cov-report=term-missing

# Lint and type-check
ruff check rova/ tests/
ruff format rova/ tests/
mypy rova/
```

### Test Structure

| File | Coverage |
|------|----------|
| `tests/test_client.py` | Payload building, response parsing, tool call extraction |
| `tests/test_integration.py` | Full async flows with mocked HTTP (`pytest-httpx`), SSE streaming |
| `tests/test_commands.py` | Slash command handlers, state mutations, error paths |
| `tests/test_tools.py` | Tool dispatch, sandbox, SSRF checks, safe expression evaluator |
| `tests/test_state.py` | ChatState defaults, TokenUsage math, token estimation |
| `tests/test_skills.py` | Skill listing, reading, parameter substitution, path traversal |
| `tests/test_tui.py` | Command detection, palette data integrity, widget structure |
| `tests/test_command_palette.py` | Fuzzy scoring, palette filtering, navigation |
| `tests/test_client_chaos.py` | Error handling, edge cases, malformed responses |

CI runs on GitHub Actions (`ci.yml`): ruff lint, mypy type-check, and pytest with coverage on Python 3.12 and 3.13. Coverage is uploaded to Codecov.

---

## Architecture

Rova is a layered Python application:

```
┌──────────────┐     HTTP/SSE      ┌──────────────┐     HTTP      ┌──────────────┐
│              │ ◄───────────────► │              │ ◄───────────► │              │
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

### Key Design Patterns

- **Backend-agnostic client** — `BaseClient` abstract class with `RouterClient` (profiles+RAG) and `DirectClient` (any OpenAI-compatible API) implementations, auto-detected based on URL and environment
- **`@work(exclusive=True)` cancellation** — new messages cancel in-flight requests; `finally` blocks clean up UI state regardless of cancellation
- **`asyncio.to_thread()` for tools** — synchronous tool handlers run in the default thread pool, keeping the TUI responsive
- **Parallel tool execution** — independent tool calls run concurrently via `asyncio.gather(return_exceptions=True)`, so one failure doesn't cancel the batch
- **Shared `httpx.AsyncClient`** — a single connection pool across the session lifetime for efficient HTTP reuse
- **Debounced streaming** — SSE tokens are buffered and rendered every ~50ms, preventing CPU thrashing from per-token Markdown re-renders

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full system design, data flow, component tree, and implementation details.

---

## Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | System design, data flow, component tree, key patterns |
| [CHANGELOG.md](CHANGELOG.md) | Version history and release notes |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup, testing, linting, PR and release process |
| [docs/TOOLS.md](docs/TOOLS.md) | Custom tool and plugin API with JSON Schema |
| [docs/SKILLS.md](docs/SKILLS.md) | Skill authoring guide with parameterized examples |
| [docs/homebrew.rb](docs/homebrew.rb) | Homebrew formula template |

---

## License

MIT — see the [LICENSE](LICENSE) file for details.
