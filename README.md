<p align="center">
  <img src="ROVA.png" alt="Rova" width="600">
</p>

# Rova — Rapid On-demand Virtual Assistant

Rich terminal frontend for [llama-router](https://github.com/komedi/llama-router) — a Textual-based TUI with interactive slash commands, multi-line input, fuzzy matching, tool execution, RAG management, and multiple themes.

## Prerequisites

- Python 3.12+
- [llama-router](https://github.com/komedi/llama-router) running on `http://127.0.0.1:8010`

## Install

### pipx (recommended)

```sh
pipx install rova
```

### From source

```sh
git clone https://github.com/bnelabs/rova.git
cd rova
python -m venv .venv
.venv/bin/pip install -e .
ln -sf "$(pwd)/bin/rova" ~/.local/bin/rova
```

Or install from a local checkout:

```sh
pip install -e /path/to/rova
ln -sf /path/to/rova/bin/rova ~/.local/bin/rova
```

## Usage

```sh
# Interactive chat (default)
rova chat

# One-shot prompt
rova send "explain quicksort in 3 sentences"

# Check router health
rova health

# List available profiles
rova profiles

# Ingest documents for RAG
rova ingest /path/to/docs https://example.com/page

# Search the RAG index
rova search "query terms"

# Full options
rova --help
```

### CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--url` | `http://127.0.0.1:8010` | Router base URL |
| `--workspace` | `~/rova-workspace` | Generated files directory |
| `--skills-dir` | `./skills` | Skills directory |
| `--profile` | auto | Force a task profile |
| `--rag` | off | Enable RAG mode |
| `--quality` | auto | Quality hint (fast/balanced/best) |
| `--max-tokens` | auto | Override max_tokens |
| `--json` | off | JSON response mode |

## Interactive Commands

Type `/` in the TUI to open the interactive command palette with fuzzy filtering, arrow-key navigation, and Tab autocomplete.

### Chat

| Command | Description |
|---------|-------------|
| `/state` | Show active settings (profile, rag, quality, tokens) |
| `/tokens` | Show estimated context usage |
| `/model` | Show active model and context window capacity |
| `/history` | Open scrollable history browser |
| `/clear` | Clear all conversation history |
| `/compact` | Summarize conversation history and continue |
| `/profile <name>` | Force a router task profile, or omit for auto |
| `/quality fast\|balanced\|best` | Set quality hint metadata |
| `/json [on\|off]` | Toggle JSON object response mode |
| `/max <tokens>` | Override max_tokens, or omit for auto |
| `/autocompact [on\|off]` | Toggle auto-compaction at 80% context |

### RAG

| Command | Description |
|---------|-------------|
| `/rag on\|off` | Toggle RAG metadata for chat requests |
| `/rag ingest <path-or-url>...` | Ingest local files/directories or URLs |
| `/rag search <query>` | Search the active RAG index |
| `/rag list` | List all indexed documents |
| `/rag delete <id>` | Remove a document from the RAG index |
| `/rag update <path>` | Re-index specific paths |

### Skills

| Command | Description |
|---------|-------------|
| `/skills` | List available skill files |
| `/skill use <name> [key=val...]` | Add a skill with optional parameters |
| `/skill drop <name>` | Remove one active skill |
| `/skill clear` | Remove all active skills |
| `/skill show <name>` | Print a skill file |

### Workspace & System

| Command | Description |
|---------|-------------|
| `/workspace` | Show workspace directory and generated files |
| `/preview <filename>` | Preview a workspace file |
| `/theme <name>` | Switch theme (rova, dracula, solarized-dark, high-contrast) |
| `/health` | Check llama-router health |
| `/profiles` | List available router profiles |
| `/help` | Show full command reference |
| `/exit` | Quit Rova |

## Keybindings

| Key | Action |
|-----|--------|
| `Enter` | Submit message (normal mode) or select command (slash mode) |
| `Shift+Enter` | Insert newline |
| `↑` / `↓` | Navigate history (normal) or palette (slash mode) |
| `Tab` | Fuzzy-autocomplete slash command |
| `Escape` | Dismiss command palette |
| `Ctrl+R` | Open interactive history browser |
| `F1` / `Ctrl+H` | Show help screen |
| `Ctrl+Q` / `Ctrl+C` | Quit |

## Themes

Four themes included — switch at runtime with `/theme <name>`:

| Theme | Accent | Style |
|-------|--------|-------|
| `rova` (default) | `#cba6f7` mauve | Catppuccin Mocha |
| `dracula` | `#bd93f9` purple | Dracula |
| `solarized-dark` | `#268bd2` blue | Solarized Dark |
| `high-contrast` | `#ffff00` yellow | Accessibility-focused |

## Tools

The local tool executor provides 9 tools the LLM can call:

| Tool | Description |
|------|-------------|
| `execute_python` | Sandboxed Python execution (256MB mem, 25s CPU, no network) |
| `write_file` | Write content to workspace |
| `read_file` | Read file contents |
| `list_files` | List directory contents |
| `web_search` | Search via DuckDuckGo (no API key) |
| `web_fetch` | Fetch URL content (HTML stripped) |
| `get_time` | Current system time in ISO 8601 |
| `calculate` | Safe arithmetic evaluation |
| `system_info` | OS, Python version, CPU count |

## Skill Parameters

Skills support `{param}` placeholders for dynamic substitution:

```markdown
<!-- skills/search.md -->
When answering, use web search with query="{query}" and cite sources.
```

```sh
/skill use search query="Python 3.13 release notes"
```

Parameters are substituted when the skill is loaded into the conversation.

## Document Generation

Generate presentations (pptx), documents (docx), and PDFs through the LLM. The model writes Python scripts using python-pptx, python-docx, or fpdf2. Generated files land in `~/rova-workspace/` by default.

## Tests

```sh
PYTHONPATH=. .venv/bin/python -m pytest tests/ -v
```

146+ tests covering commands, tools, state, skills, client, integration, TUI components, and the command palette.

## Docker

### Build and run standalone

```sh
docker build -t rova .
docker run -it --rm rova --version
docker run -it --rm \
  -v ~/.config/rova:/root/.config/rova \
  -v ~/rova-workspace:/root/rova-workspace \
  rova chat
```

### Quick-start stack with llama-router

The included `docker-compose.yml` starts both rova and llama-router:

```sh
# Start the router in the background
docker compose up -d llama-router

# Launch the TUI
docker compose run rova chat
```

The compose stack mounts `~/.config/rova` for persistent config and
`~/.config/llama-router` for router configuration. Both services are on
the same Docker network — rova connects to `http://llama-router:8010`
automatically.

## Documentation

- [Architecture](ARCHITECTURE.md) — system design, data flow, component tree, key patterns
- [Contributing](CONTRIBUTING.md) — dev setup, testing, linting, PR process
- [Custom Tools](docs/TOOLS.md) — how to add new tools with JSON Schema + handler functions
- [Skills](docs/SKILLS.md) — creating prompt templates with `{param}` substitution
- [Changelog](CHANGELOG.md) — version history and release notes

## License

MIT
