# r105 — Beyond the prompt.

r105 is a rich terminal AI assistant built on [Textual](https://textual.textualize.io/). It connects to any OpenAI-compatible API (OpenAI, Ollama, vLLM, Groq, and others) and provides an interactive chat TUI with streaming SSE responses, a slash-command system, fuzzy command palette, local tool execution, secure sandboxing, MCP integration, plugin extensibility, session persistence, and multiple themes.

<p align="center">
  <img src="https://img.shields.io/pypi/v/r105?color=cba6f7" alt="PyPI">
  <img src="https://img.shields.io/badge/python-3.12%20%7C%203.13-blue" alt="Python">
  <img src="https://img.shields.io/badge/tests-440%20passed-brightgreen" alt="Tests">
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
- **llama-router** running on `http://127.0.0.1:8010` (or set `R105_URL` / `--url`)

---

## Installation

### pipx (recommended)

```sh
pipx install r105

# With optional export support (docx/pptx/pdf):
pipx install "r105[export]"
```

### Homebrew (macOS / Linux)

```sh
brew install bnelabs/tap/r105
```

### Linux and FreeBSD packages

The release page includes native x86_64 packages with the standard extension
for each package manager. Set `VERSION` to the release you downloaded:

```sh
VERSION=0.7.0

# Ubuntu / Debian
sudo apt install "./r105_${VERSION}_amd64.deb"

# Arch Linux
sudo pacman -U "r105-${VERSION}-1-x86_64.pkg.tar.zst"

# Fedora / RHEL-like distributions
sudo dnf install "./r105-${VERSION}-1.x86_64.rpm"

# FreeBSD
sudo pkg add "./r105-${VERSION}-freebsd-amd64.pkg"

# Alpine Linux (verify SHA256SUMS before installing an unsigned local APK)
sudo apk add --allow-untrusted "./r105-${VERSION}-r0.apk"
```

The Ubuntu, Arch, and Fedora packages use the Linux x86_64 binary built on
Ubuntu 22.04. The FreeBSD and Alpine packages are built natively on their
respective systems. The Alpine job is best effort because it targets a
separate musl ABI; if that native build is unavailable, the other release
assets are still published.

### Standalone binary

Pre-built single-file executables are attached to [GitHub Releases](https://github.com/bnelabs/r105/releases) as archives for Linux, macOS (x64 and ARM64), and Windows. Verify `SHA256SUMS`, extract the archive, make the Unix executable runnable, and place it on your `PATH` — no Python install needed. The archives are named `r105-linux-x64.tar.gz`, `r105-macos-x64.tar.gz`, `r105-macos-arm64.tar.gz`, and `r105-windows-x64.zip`.

### From source

```sh
git clone https://github.com/bnelabs/r105.git
cd r105
python -m venv .venv
.venv/bin/pip install -e .
ln -sf "$(pwd)/bin/r105" ~/.local/bin/r105
```

### Docker

```sh
docker build -t r105 .
docker run -it --rm \
  -v ~/.config/r105:/root/.config/r105 \
  -v ~/r105-workspace:/root/r105-workspace \
  r105 chat
```

For a quick-start stack with llama-router, use the included `docker-compose.yml`:

```sh
docker compose up -d llama-router
docker compose run r105 chat
```

---

## Quick Start

```sh
# Launch the TUI (requires llama-router on http://127.0.0.1:8010)
r105 chat

# One-shot: ask a question and get a response without the TUI
r105 send "explain quicksort in 3 sentences"

# Connect to a different router or any OpenAI-compatible API
r105 --url http://my-router:8010 chat
OPENAI_API_KEY=sk-... r105 --url https://api.openai.com/v1 chat
```

---

## CLI Usage

r105 supports both one-shot prompts and management commands at the command line.

```sh
# Interactive chat (default, opens TUI)
r105 chat

# One-shot prompt
r105 send "explain quicksort in 3 sentences"

# Check router health
r105 health

# List available task profiles
r105 profiles

# Diagnose environment: config, sandbox, backend, workspace
r105 doctor

# Print the supported config.json JSON Schema
r105 config-schema

# Load a saved session on startup
r105 --session my-session chat

# Auto-approve code execution (skip the confirmation gate)
r105 --yes chat

# Version info
r105 --version
```

### CLI Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--url` | `http://127.0.0.1:8010` | Router or API base URL |
| `--workspace` | `~/r105-workspace` | Workspace directory for file tools |
| `--skills-dir` | `./skills` | Directory containing skill `.md` files |
| `--plugins-dir` | `~/.config/r105/plugins` | Directory for custom tool plugins |
| `--profile` | auto | Force a router task profile |
| `--model` | auto | Override model selection |
| `--quality` | auto | Quality hint: `fast`, `balanced`, or `best` |
| `--max-tokens` | auto | Override max output tokens |
| `--json` | off | Request JSON-object responses |
| `--backend` | auto | `router` (profiles) or `direct` (any OpenAI-compatible) |
| `--session` | — | Load a saved session on startup |
| `--yes` | off | Auto-approve `execute_python` (skip confirmation gate) |
| `--version` | — | Print version and exit |

---

## Backends

r105 auto-detects the best backend:

1. **`R105_URL` set → llama-router** — full profile routing, model selection
2. **`OPENAI_API_KEY` set → direct** — any OpenAI-compatible API (OpenAI, Ollama, vLLM, Groq)
3. **Otherwise** — checks local Ollama, falls back to direct

Override with `--backend router` or `--backend direct`.

### Backend Capabilities

| Feature | Router backend | Direct backend |
|---------|:---:|:---:|
| Profiles (task routing) | ✅ | — |
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

### Thinking Panels & Virtualized Transcript

- **Collapsible thinking panels** — reasoning models that emit Gemma-4-style
  thinking blocks (`<|channel|>thought … <channel|>`) get an interactive
  `💭 THINKING` panel per block, folded by default so answers stay readable.
  Click the panel — or press `t` / `Enter` / `Space` while it is focused — to
  expand the full reasoning and fold it again. `show_thinking` controls
  whether panels render at all; `thinking_default_expanded` starts them
  expanded (see [Configuration](#configuration)).
- **Virtualized transcript** — the chat view keeps the full conversation as
  lightweight records and only materializes the messages on screen (plus an
  overscan window) as widgets. Very long sessions stay responsive and
  memory-bounded, and the transcript is never truncated: scrolling back
  re-renders history on demand.
- **Auto-follow** — the view pins to the newest message while you're at the
  bottom and stops following the moment you scroll up; returning to the
  bottom re-pins it.

### Tool Results & Inspector

- **Collapsible long outputs** — tool results over ~2,000 characters start
  folded to a preview. Click the panel — or press `t` / `Enter` / `Space`
  while it is focused — to expand the full output in place.
- **Diff coloring & code highlighting** — unified diffs render with
  green/red/hunk styling; outputs with fenced code blocks get syntax
  highlighting.
- **Tool inspector (`Ctrl+T`)** — a modal screen listing every tool call in
  the session with its arguments and result preview, plus fuzzy filtering.

### Command Palette

Press `/` to open the interactive command palette with:

- **Fuzzy filtering** — type any part of a command name to narrow
- **Arrow-key navigation** — ↑/↓ to browse, Enter to select
- **Tab autocomplete** — fills in the matching command
- **Category grouping** — Chat, Skills, Sessions, Plugins, MCP, Workspace, System
- **Escape to dismiss**

### Slash Commands

#### Chat

| Command | Description |
|---------|-------------|
| `/state` | Show active settings (profile, quality, tokens, model) |
| `/tokens` | Show context usage, estimate source, and confidence |
| `/model [name]` | Show current model, list available, or switch models (persistent) |
| `/history` | Show compact transcript preview of last messages |
| `/clear` | Clear all conversation history |
| `/compact` | Summarize conversation history and continue |
| `/profile <name>` | Force a router task profile: `simple`, `coding`, `complex_reasoning`, `long_context_qa`, `tool_agent`, `creative`, `strict_json` |
| `/quality fast\|balanced\|best` | Set quality hint metadata |
| `/json [on\|off]` | Toggle JSON object response mode |
| `/max <tokens>` | Override max output tokens |
| `/cache-prompt [on\|off]` | Enable llama.cpp prompt-prefix caching |
| `/config reload` | Reload supported `config.json` settings into the current session |
| `/autocompact [on\|off]` | Toggle auto-compaction at 80% context threshold |
| `/reasoning auto\|off\|low\|medium\|high` | Set reasoning effort (sent to capable backends) |
| `/permissions <posture>` | Set tool-execution posture (`full-access\|restricted\|sandboxed\|off`) |
| `/approve execute_python` | One-time approval for code execution (this session) |
| `/copy` | Copy last assistant message to system clipboard |

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
| `/session save <name>` | Save current conversation to `~/.config/r105/sessions/` |
| `/session load <name>` | Load and restore a previously saved session (shows a diff of unsaved changes first) |
| `/session list` | List all saved sessions with previews and timestamps |
| `/session search <query>` | Search message text across saved sessions |
| `/session delete <name>` | Delete a saved session |
| `/export text` | Export conversation as plain text |
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
| `/mcp reconnect <server>` | Reconnect a server and rediscover its tools |

#### Workspace & System

| Command | Description |
|---------|-------------|
| `/workspace` | Show workspace directory and list generated files |
| `/preview <filename>` | Preview a workspace file's contents |
| `/theme <name>` | Switch theme: `r105`, `dracula`, `solarized-dark`, `high-contrast` |
| `/health` | Check llama-router and upstream model health |
| `/profiles` | List available router task profiles |
| `/help` | Show the full command reference |
| `/exit` | Quit r105 |

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
| `Ctrl+T` | Any | Open tool-call inspector (names, args, result previews) |
| `Ctrl+X` | Any | Cancel the current local-tool batch |
| `Ctrl+S` | History screen | Quick-save current session |
| `Ctrl+Y` | Any | Copy last assistant response to clipboard |
| `t` / `Enter` / `Space` | Thinking panel focused | Expand / fold the thinking panel |
| `t` / `Enter` / `Space` | Long tool result focused | Expand / fold the full output |
| `Ctrl+W` | Input | Delete word backward |
| `Ctrl+U` | Input | Clear line |
| `Ctrl+A` | Input | Jump to line start |
| `Ctrl+E` | Input | Jump to line end |
| `Ctrl+K` | Input | Kill to end of line |
| `F1` / `Ctrl+H` | Any | Show help screen |
| `Ctrl+Q` / `Ctrl+C` | Any | Quit |

Core actions can be remapped in `config.json` by binding ID. The supported
IDs are `quit`, `show_help`, `show_history`, `copy_last_message`,
`show_tools`, `cancel_tools`, and `cancel_request`:

```json
{
  "keybindings": {
    "show_tools": "ctrl+o",
    "copy_last_message": "ctrl+y"
  }
}
```

---

## Built-in Tools

r105 provides 10 local tools the LLM can call. All tools are validated before execution with argument size caps, SSRF prevention, and path traversal hardening. Web fetches validate every redirect and revalidate DNS answers at the TCP connection boundary. `execute_python` runs in a sandboxed environment with no network or filesystem access — and needs a one-time approval per session (`/approve execute_python`, `--yes`, or the `auto_approve_execute_python` config key) before it runs at all. Web tools are rate-limited per session (30 searches / 60 fetches per minute).

| Tool | Description | Sandbox Profile |
|------|-------------|----------------|
| `execute_python` | Sandboxed Python execution (256MB RAM, 25s CPU, seccomp) | No network, no filesystem |
| `write_file` | Write/update files in workspace — generates unified diffs on edit | Filesystem write |
| `read_file` | Read file contents (max 50MB, with `<tool_output>` tags) | Filesystem read |
| `list_files` | List directory contents | Filesystem read |
| `web_search` | Search via DuckDuckGo HTML (no API key) | Network access |
| `web_fetch` | Fetch URL content (HTML stripped to text) | Network access |
| `get_time` | Current system time in ISO 8601 | Minimal isolation |
| `calculate` | Safe math evaluator (arithmetic, math functions, pi/e/tau) | Minimal isolation |
| `convert` | Unit conversion (length, mass, time, data, speed, volume, temperature) | Minimal isolation |
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

r105 implements layered security for code execution. The backend is auto-detected: `nsjail` > `bwrap` > `docker` > `rlimit` > `none` (Windows fallback).

> **Fallback transparency:** if a stronger backend is unavailable, r105 tells you why — a `warning:` line on startup, a `⚠ sandbox=<backend> (no isolation)` segment in the TUI status bar, and a structured log entry. Falling back to `rlimit`/`none` means tool code runs **without filesystem/network isolation**.

### Sandbox Backends

| Backend | Isolation Level | Requirements |
|---------|:---:|-------------|
| **Nsjail** (strongest) | Linux namespace isolation, seccomp-bpf syscall allowlist, no host filesystem | `nsjail` binary on PATH |
| **Bwrap** | User namespace isolation via bubblewrap, minimal `/dev` bind, conditional network/filesystem per profile | `bwrap` on PATH |
| **Docker** | Fresh container per execution, read-only root FS, dropped capabilities | `docker` + running daemon |
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

Skills are reusable Markdown prompt templates stored in `~/.config/r105/skills/`. They support `{param}` placeholder substitution for dynamic content injection.

Skills appear as system messages in the LLM context, persisting across `/clear` but not across sessions (re-activate on each launch).

### Built-in Skills

| Skill | Description |
|-------|-------------|
| `code-check` | Code review assistant — prefers executable snippets, checks syntax and imports |
| `concise` | Forces concise, direct responses with no preamble |
| `deep-review` | Comprehensive analysis covering deliverables, assumptions, edge cases, security |

### Creating a Skill

```markdown
<!-- ~/.config/r105/skills/web-researcher.md -->
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

Custom tools can be loaded from Python files in `~/.config/r105/plugins/`. Each file exposes a `register(registry)` function that adds tools via `registry.add_tool()`.

**Example plugin** (`~/.config/r105/plugins/hello.py`):

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

Plugin loading is validated: the file must expose `register(registry)` taking exactly one argument, and every tool needs a non-empty name/description, a parameters schema object, and a callable handler — violations are reported as specific warnings instead of silently skipping. Plugins can declare `__r105_min_version__ = "0.7.0"` and `PLUGIN_REQUIREMENTS = ["package_name"]`; incompatible plugins are skipped with a warning. `/plugin reload` drains active plugin calls before replacing handlers. Plugins cannot shadow built-in tools unless you opt in via the `allow_plugin_overrides` config key or `R105_ALLOW_PLUGIN_OVERRIDE=1`.

See [docs/TOOLS.md](docs/TOOLS.md) for the full API reference.

---

## MCP — Model Context Protocol

r105 can connect to external MCP servers (stdio or SSE transport) to access community-built tools without changing r105's code.

### Configuration

Configure servers in `~/.config/r105/config.json`:

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

Conversations can be saved, loaded, and exported in multiple formats. r105 also **auto-saves on exit** as `__autosave__`.

### Session Storage

Sessions are stored as JSON files in `~/.config/r105/sessions/`. Each file contains:

- **Conversation history** (all user, assistant, and tool messages)
- **Session state** (profile, quality, skills, and parameters)
- **Model settings** (selected model, context capacity, and prompt-cache toggle)
- **Message count and save timestamp**

### Export Formats

| Format | What you get |
|--------|-------------|
| `markdown` | Styled `.md` with role headers and message numbering |
| `json` | Raw conversation array, portable and machine-readable |
| `html` | Styled HTML page with role-colored messages |

### Export Dependencies

`text`, `markdown`, `json`, and `html` exports work out of the box. The `pdf`, `docx`, and `pptx` formats require the optional export extra:

```sh
pip install "r105[export]"
```

If the extra is missing, `/export` shows a helpful install hint. See [docs/EXPORT.md](docs/EXPORT.md) for details.

---

## Themes

Four themes are included. Switch at runtime with `/theme <name>`:

| Theme | Accent | Style |
|-------|--------|-------|
| `r105` (default) | `#cba6f7` mauve | Catppuccin Mocha |
| `dracula` | `#bd93f9` purple | Dracula |
| `solarized-dark` | `#268bd2` blue | Solarized Dark |
| `high-contrast` | `#ffff00` yellow | Accessibility-focused |

Themes are defined as Textual CSS files in `r105/themes/`. The selected theme persists in `~/.config/r105/config.json`.

---

## Auto-Compaction

When conversation context approaches 80% of the model's capacity, r105 can automatically summarize earlier messages to free space. This is controlled by:

- **`/autocompact on|off`** — toggle from within the TUI
- **`auto_compact` field** in `config.json` — persistent default

The context indicator reports its measurement source and confidence. After a
backend returns standard `usage` metadata, r105 uses the provider's exact
total. Before that, it uses tiktoken when available and labels generic or
heuristic estimates with lower confidence.

Use `/config reload` after editing `config.json` to apply live session settings
such as the theme, model, context overrides, prompt caching, keybindings, and
permission posture. Workspace paths and MCP server lists remain startup-level
settings; use `/mcp reconnect <server>` for an individual MCP connection.

### Prompt Caching

`cache_prompt` is disabled by default. Enable it with `/cache-prompt on` or in
`config.json` when the selected backend is llama.cpp-compatible. The flag is
omitted when disabled because other OpenAI-compatible providers may reject it.

Compaction uses the `complex_reasoning` profile and keeps the most recent 30% of messages intact.

---

## Configuration

r105 stores configuration in `~/.config/r105/`:

```
~/.config/r105/
├── config.json          # theme, model, sandbox backend, defaults
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
  "cache_prompt": false,
  "keybindings": {
    "show_tools": "ctrl+o"
  },
  "theme": "r105",
  "show_thinking": true,
  "thinking_default_expanded": false,
  "model_families": {
    "my-gemma4-finetune": "gemma-4",
    "gemma-4-12b": null
  },
  "mcp_servers": [
    {
      "name": "github",
      "transport": "sse",
      "url": "http://127.0.0.1:3001/mcp"
    }
  ]
}
```

Invalid configuration falls back to defaults during normal startup. Set
`R105_STRICT_CONFIG=1` when validating a deployment so unknown keys, invalid
values, malformed JSON, and unreadable config files stop startup with an error.
Run `r105 config-schema` to print the supported JSON Schema, or
`r105 config-schema --output /path/to/config.schema.json` to write it.

### Model Families (`model_families`)

r105 gates model-specific behavior (Gemma-4 channel-syntax handling: native
`<|tool_call|>` parsing, `<|channel|>thought` capture, stray-token stripping)
behind a built-in model-family catalog, so every other model's output is
treated as opaque text. `model_families` lets you override that classification
per model-name fragment — longest matching fragment wins:

- `"<fragment>": "gemma-4"` — force a model (e.g. a Gemma-4 fine-tune with a
  custom name) into the Gemma-4 family so channel-syntax handling applies.
- `"<fragment>": null` — force a model out of every family (opaque
  passthrough), e.g. to disable channel-syntax handling for a model the
  catalog would otherwise classify as Gemma-4.

Values are family names (currently `gemma-4`, `gemma`, `qwen`, `llama`,
`mistral`, `deepseek`, `glm`, `gpt`, `claude`, `phi`) or `null`. Only the
`gemma-4` family currently gates behavior; other families are labels.

### Config Merging

CLI arguments override config file values, which override built-in defaults. The config file only stores overrides — matching defaults are omitted to keep the file lean.

### Config Validation

The config file is validated on load: unknown keys and invalid values fall back to defaults, and a clear error is raised on save.

### Structured Logging

Debug logs are written to `~/.local/state/r105/log.jsonl` in JSON Lines format. Set `R105_LOG_LEVEL=DEBUG` for verbose output.

---

## Docker

### Standalone

```sh
docker build -t r105 .
docker run -it --rm \
  -v ~/.config/r105:/root/.config/r105 \
  -v ~/r105-workspace:/root/r105-workspace \
  r105 chat
```

### Quick-start stack with llama-router

The `docker-compose.yml` starts both r105 and llama-router on a shared network:

```sh
docker compose up -d llama-router
docker compose run r105 chat
```

The stack mounts `~/.config/r105` and `~/.config/llama-router` for persistent configuration. `R105_URL=http://llama-router:8010` is set automatically.

---

## Updating & Uninstalling

### pipx

```sh
pipx upgrade r105          # update (extras are preserved)
pipx uninstall r105        # remove
```

### Homebrew

```sh
brew upgrade r105          # update
brew uninstall r105        # remove
```

### Standalone binary

Download the latest binary from [GitHub Releases](https://github.com/bnelabs/r105/releases) and replace the old one.

For archive downloads, verify `SHA256SUMS`, extract the archive, and replace
the executable inside it. Package-manager installations can be upgraded with
the same command used for the initial install.

### From source

```sh
cd /path/to/r105
git pull
.venv/bin/pip install -e .

# Uninstall
rm ~/.local/bin/r105
rm -rf /path/to/r105/.venv
```

### Docker

```sh
git pull && docker build -t r105 .     # update
docker rmi r105                        # remove image
```

### Config cleanup

None of these methods remove your user-level data. To wipe everything:

```sh
rm -rf ~/.config/r105
rm -rf ~/r105-workspace
```

---

## Testing

```sh
# Install dev dependencies
pip install -e ".[dev]"

# Run full test suite (440 tests)
python -m pytest tests/ -v

# With coverage
python -m pytest tests/ -v --cov=r105 --cov-report=term-missing

# Compare local token estimates with a tiktoken baseline
python benchmarks/token_estimation.py

# Lint and type-check
ruff check r105/ tests/
ruff format r105/ tests/
mypy r105/
```

### Test Structure

| File | Coverage |
|------|----------|
| `tests/test_client.py` | Payload building, response parsing, tool call extraction |
| `tests/test_client_chaos.py` | Error handling, edge cases, malformed SSE, retry behavior |
| `tests/test_integration.py` | Full async flows with mocked HTTP (`pytest-httpx`), SSE streaming |
| `tests/test_commands.py` | Slash command handlers, state mutations, error paths |
| `tests/test_tools.py` | Tool dispatch, sandbox, SSRF checks, safe math evaluator, unit conversion |
| `tests/test_state.py` | ChatState defaults, TokenUsage math, token estimation |
| `tests/test_skills.py` | Skill listing, reading, parameter substitution, path traversal |
| `tests/test_config.py` | Config validation (manual + pydantic schema) |
| `tests/test_architecture_layers.py` | Client facade, command registry, tool protocol, trace IDs, atomic sessions, schema checks |
| `tests/test_model_catalog.py` | Model family/context resolution and overrides |
| `tests/test_reasoning_permissions.py` | Reasoning effort and permission posture handling |
| `tests/test_sessions_export_guard.py` | Optional export-dependency guard |
| `tests/test_sandbox_fallback.py` | Sandbox fallback reasons and weak-backend warnings |
| `tests/test_plugin_validation.py` | Plugin signature and tool-schema validation |
| `tests/test_session_diff.py` | Session diff summaries |
| `tests/test_tools_screen.py` | Tool-inspector collection and rendering |
| `tests/test_tui.py` | Command detection, palette data integrity, widget structure |
| `tests/test_command_palette.py` | Fuzzy scoring, palette filtering, navigation |
| `tests/test_tui_features.py` | TUI feature coverage (streaming, panels, markers) |
| `tests/test_tui_virtualization.py` | Virtualized transcript, collapsible panels and results |

CI runs on GitHub Actions (`ci.yml`): ruff lint, mypy type-check, and pytest with coverage on Python 3.12 and 3.13. Coverage is uploaded to Codecov.

---

## Architecture

r105 is a layered Python application:

```
┌──────────────┐     HTTP/SSE      ┌──────────────┐     HTTP      ┌──────────────┐
│              │ ◄───────────────► │              │ ◄───────────► │              │
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

### Key Design Patterns

- **Backend-agnostic client** — `BaseClient` abstract class with `RouterClient` (profiles) and `DirectClient` (any OpenAI-compatible API) implementations, auto-detected based on URL and environment
- **Stable client facade** — `r105.client.Client` exposes `chat()`, `stream_chat()`, `list_models()`, health, and compaction without requiring callers to know which backend was selected
- **`@work(exclusive=True)` cancellation** — new messages cancel in-flight requests; `finally` blocks clean up UI state regardless of cancellation
- **`asyncio.to_thread()` for tools** — synchronous tool handlers run in the default thread pool, keeping the TUI responsive
- **Parallel tool execution** — independent tool calls run concurrently via `asyncio.gather(return_exceptions=True)`, so one failure doesn't cancel the batch
- **Shared `httpx.AsyncClient`** — a single connection pool across the session lifetime for efficient HTTP reuse
- **Debounced streaming** — SSE tokens are buffered and rendered every ~50ms, preventing CPU thrashing from per-token Markdown re-renders
- **Correlation and diagnostics** — each `ChatState` carries a trace ID through request headers, structured logs, and tool execution; the TUI status bar periodically checks backend health and local runtime readiness

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full system design, data flow, component tree, and implementation details.

---

## Documentation

| Document | Description |
|----------|-------------|
| [ARCHITECTURE.md](ARCHITECTURE.md) | System design, data flow, component tree, key patterns |
| [CHANGELOG.md](CHANGELOG.md) | Version history and release notes |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup, testing, linting, PR and release process |
| [docs/EXPORT.md](docs/EXPORT.md) | Optional export dependencies and formats |
| [docs/TOOLS.md](docs/TOOLS.md) | Custom tool and plugin API with JSON Schema |
| [docs/SKILLS.md](docs/SKILLS.md) | Skill authoring guide with parameterized examples |
| [docs/homebrew.rb](docs/homebrew.rb) | Homebrew formula template |

---

## License

MIT — see the [LICENSE](LICENSE) file for details.
