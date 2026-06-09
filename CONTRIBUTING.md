# Contributing to Rova

## Development Setup

```sh
# Clone and set up
git clone https://github.com/bnelabs/rova.git
cd rova
python3 -m venv .venv
source .venv/bin/activate
pip install -e ".[dev]"

# Verify
python -m pytest tests/ -v
```

You need Python 3.12 or later. A running [llama-router](https://github.com/komedi/llama-router) instance is required for end-to-end testing, but unit and integration tests run without one.

## Running Tests

```sh
# All tests
python -m pytest tests/ -v

# Specific test file
python -m pytest tests/test_client.py -v

# With coverage
pip install pytest-cov
python -m pytest tests/ --cov=rova --cov-report=term-missing
```

### Test Structure

| File | What it tests |
|------|---------------|
| `tests/test_client.py` | RouterClient helpers: `_build_payload`, `_parse_response`, `_extract_tool_calls` |
| `tests/test_integration.py` | Full async flows with mocked HTTP responses (`pytest-httpx`) |
| `tests/test_commands.py` | Slash command handlers and state mutations |
| `tests/test_tools.py` | Tool execution, sandbox, safe math evaluator |
| `tests/test_state.py` | ChatState, TokenUsage, token estimation |
| `tests/test_skills.py` | Skill file listing, reading, parameter substitution |
| `tests/test_command_palette.py` | Fuzzy matching and palette filtering |

## Linting & Type Checking

```sh
# Lint
ruff check rova/ tests/

# Auto-fix
ruff check --fix rova/ tests/

# Format
ruff format rova/ tests/

# Type check
mypy rova/
```

All three must pass before submitting a PR. CI enforces this automatically.

### Pre-commit Hooks (Optional)

```sh
pip install pre-commit
pre-commit install
```

This runs ruff and mypy on every commit.

## Code Style

- **Line length:** 100 characters
- **Quotes:** Double quotes (`"`)
- **Imports:** `from __future__ import annotations` at the top of every file
- **Type hints:** Use `from typing import Any` for `Any`. Use `dict[str, Any]` and `list[str]` (not `Dict`/`List` from typing). Use `| None` instead of `Optional`.
- **Docstrings:** Google-style. Every public function should have one.

### Async Patterns

Use `async def` / `await` for all I/O. Offload blocking calls with `asyncio.to_thread()`.

Use `return_exceptions=True` with `asyncio.gather()` when batching independent tasks — a single failure should not cancel the rest.

Textual workers use `@work(exclusive=True)`. The `finally` block is the right place for UI cleanup (it runs even on `CancelledError`).

### Error Handling

- Catch `httpx.HTTPError` for network failures — these are user-facing (router down, timeout)
- Let `asyncio.CancelledError` propagate — it's how Textual cancels workers
- Use `BaseException` checks when processing `asyncio.gather(return_exceptions=True)` results — `CancelledError` inherits from `BaseException`, not `Exception`

## Project Conventions

### Adding a New Slash Command

1. Add the command name to `SLASH_COMMANDS` in `rova/commands.py`
2. Add a new `if command == "/yourcmd":` branch in `handle_slash_command()`
3. Add the command to `command_menu()` output
4. Register it in `COMMAND_DEFS` in `rova/tui/widgets/command_palette.py` (category, command, usage, description)

### Adding a New Tool

1. Write the handler function in `rova/tools.py`
2. Add the JSON Schema definition to `TOOL_DEFINITIONS`
3. Add a dispatch branch in `execute_tool_call()`
4. Add tests in `tests/test_tools.py`

## PR Process

1. Fork the repo and create a feature branch
2. Make your changes
3. Run `ruff check rova/ tests/ && mypy rova/ && python -m pytest tests/ -v`
4. Push and open a PR against `main`
5. CI will run the same checks automatically

## Release Process (Maintainers)

```sh
# Update version in pyproject.toml
# Commit and tag
git tag v0.3.0
git push --tags

# Build and publish
python -m build
twine upload dist/*
```
