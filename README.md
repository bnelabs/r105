# Rova

Rich terminal frontend for [llama-router](https://github.com/komedi/llama-router).

## Prerequisites

- Python 3.12+
- [llama-router](https://github.com/komedi/llama-router) running on `http://127.0.0.1:8010`

## Install

```sh
pip install -e /home/komedi/rova
ln -sf /home/komedi/rova/bin/rova ~/.local/bin/rova
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

## Interactive Commands

Inside the TUI, type `/` for the command menu. Key commands:

| Command | Description |
|---------|-------------|
| `/profile <name>` | Force a task profile (or omit for auto) |
| `/rag on\|off` | Toggle RAG mode |
| `/quality fast\|balanced\|best` | Set quality hint |
| `/json on\|off` | Toggle JSON response mode |
| `/max <tokens>` | Override max_tokens |
| `/compact` | Summarize conversation history |
| `/skills` | List available skills |
| `/skill use <name>` | Activate a skill |
| `/skill drop <name>` | Deactivate a skill |
| `/clear` | Clear chat history |
| `/state` | Show current settings |
| `/exit` | Quit |

## Document Generation

Rova supports generating presentations (pptx), documents (docx), and PDFs
through the LLM. Describe what you want and the model will generate Python
scripts using python-pptx, python-docx, or fpdf2. Generated files land in
`~/rova-workspace/` by default.

## Configuration

| Env variable | Default | Description |
|-------------|---------|-------------|
| `ROVA_ROUTER_URL` | `http://127.0.0.1:8010` | llama-router base URL |
| `ROVA_WORKSPACE` | `~/rova-workspace` | Generated files directory |
| `ROVA_SKILLS_DIR` | `./skills` | Skills directory |
