# Custom Tools in r105

r105 ships with 10 built-in tools (`execute_python`, `write_file`, `read_file`, `list_files`, `web_search`, `web_fetch`, `get_time`, `calculate`, `convert`, `system_info`). This guide explains how tools work and how to add your own.

## How Tools Work

Each tool has two parts:

1. **A JSON Schema definition** — tells the LLM what the tool does and what arguments it accepts
2. **A handler function** — executes the tool and returns a result

When the LLM decides to call a tool, it emits a `tool_calls` array in its response. r105's tool loop picks up these calls, dispatches to the handler via `execute_tool_call()`, and feeds the results back into the conversation.

## Anatomy of a Tool

### The Schema Definition

Registered with the `@get_tool_registry().register(...)` decorator in `r105/tools.py` (the schema doubles as the LLM's `TOOL_DEFINITIONS` entry):

```python
@get_tool_registry().register(
    name="web_search",
    description="Search the web and return results with titles, URLs, and snippets.",
    parameters={
        "query": {
            "type": "string",
            "description": "Search query string.",
        },
    },
    required=["query"],
    needs_network=True,
    needs_filesystem=False,
    needs_output_truncation=True,
)
def web_search(arguments: dict[str, Any]) -> str:
    ...

### The Handler Function

A synchronous function that receives parsed arguments. Handlers may declare
`(arguments)`, `(arguments, workspace_dir)`, or no parameters at all —
dispatch adapts to the declared arity (`call_tool_handler()` in
`r105/registry.py`), so pure tools omit what they don't need:

```python
def web_search(arguments: dict[str, Any]) -> str:
    """Search the web using DuckDuckGo HTML (no API key required)."""
    query = arguments.get("query", "")
    if not query:
        return "error: query is required"

    try:
        # Production web tools use _safe_http_client() and
        # _request_with_validated_redirects() from r105.tools_web so every
        # redirect and every post-DNS TCP destination is checked.
        with _safe_http_client() as client:
            response = _request_with_validated_redirects(
                client,
                "https://html.duckduckgo.com/html/",
                params={"q": query},
                timeout=15.0,
            )
        response.raise_for_status()
        results = parse_ddg_results(response.text, WEB_SEARCH_MAX_RESULTS)
        return json.dumps(results, indent=2, ensure_ascii=False)
    except Exception as e:
        return f"search error: {e}"
```

Shared web plumbing (SSRF allow-listing, HTML stripping, DDG parsing) lives
in `r105/tools_web.py` — import from there instead of duplicating it.

### The Dispatch Entry

Dispatch is automatic: the decorator registers the handler on the shared `ToolRegistry`, and `execute_tool_call()` routes by name (built-ins → plugins → MCP). No manual dispatch table to edit. Add argument checks to `_validate_tool_args()` in `r105/tools.py` when the tool needs them:

```python
elif name == "web_search":
    query = arguments.get("query", "")
    if not query:
        return "query is required"
```

## Adding a New Tool: Step by Step

Let's walk through adding a `send_email` tool.

### Step 1: Write the Handler

In `r105/tools.py`, add a new function:

```python
def send_email(arguments: dict[str, Any]) -> str:
    """Send an email via SMTP (requires smtplib credentials in config)."""
    to_addr = arguments.get("to", "")
    subject = arguments.get("subject", "")
    body = arguments.get("body", "")

    if not all([to_addr, subject, body]):
        return "error: 'to', 'subject', and 'body' are required"

    # In production, load SMTP config from config.json
    # For now, return a placeholder
    return f"Would send email to {to_addr}:\nSubject: {subject}\n{len(body)} chars"
```

### Step 2: Register the Schema

Decorate the handler with `@get_tool_registry().register(...)` (name, description, parameters, required, plus `needs_network` / `needs_filesystem` / `needs_output_truncation` flags). This both defines the LLM-facing JSON Schema and wires up dispatch — there is no separate `TOOL_DEFINITIONS` list or `elif` chain to edit:

```python
@get_tool_registry().register(
    name="send_email",
    description="Send an email to a recipient.",
    parameters={
        "to": {
            "type": "string",
            "description": "Recipient email address.",
        },
        "subject": {
            "type": "string",
            "description": "Email subject line.",
        },
        "body": {
            "type": "string",
            "description": "Email body text.",
        },
    },
    required=["to", "subject", "body"],
    needs_network=True,
    needs_filesystem=False,
)
def send_email(arguments: dict[str, Any]) -> str:
    ...
```

### Step 3: Validate Arguments

If the tool needs argument checks, add a branch to `_validate_tool_args()` in `r105/tools.py`:

```python
elif name == "send_email":
    if not arguments.get("to"):
        return "to is required"
```

### Step 4: Add Tests

In `tests/test_tools.py`:

```python
def test_send_email_missing_args(workspace_dir: Path) -> None:
    call = {
        "id": "call_test",
        "function": {
            "name": "send_email",
            "arguments": '{"to": "a@b.com"}',
        },
    }
    result = execute_tool_call(call, workspace_dir)
    assert "required" in result["content"]


def test_send_email_success(workspace_dir: Path) -> None:
    call = {
        "id": "call_test",
        "function": {
            "name": "send_email",
            "arguments": '{"to": "a@b.com", "subject": "Hi", "body": "Hello"}',
        },
    }
    result = execute_tool_call(call, workspace_dir)
    assert "Would send email" in result["content"]
```

## Tool Execution Model

Tools run **synchronously** in a thread pool via `asyncio.to_thread()`:

```python
async def _exec_one(tc: dict[str, Any]) -> dict[str, Any]:
    return await asyncio.to_thread(execute_tool_call, tc, self.workspace)
```

This prevents I/O or CPU-heavy tools from blocking the TUI event loop. Multiple independent tool calls run in **parallel** via `asyncio.gather()`.

## Return Value Format

Tool handlers must return a `str`. The result is wrapped in a tool message:

```python
{
    "role": "tool",
    "tool_call_id": call.get("id", ""),
    "name": name,
    "content": result,  # your handler's return value
}
```

For structured data (search results, API responses), return JSON:

```python
return json.dumps(results, indent=2, ensure_ascii=False)
```

For errors, prefix with `error:`:

```python
return f"error: {description}"
```

## Security Considerations

### Sandboxing

Python execution (`execute_python`) runs in the auto-detected sandbox backend (`nsjail` > `bwrap` > `docker` > `rlimit` > `none`):
- Strong backends (nsjail/bwrap/docker): namespace/container isolation, 256 MB memory, 25s CPU, stripped environment
- Weak backends (rlimit/none): resource limits only or nothing — auto-selection is surfaced via a startup warning and the TUI status bar

See [Sandbox & Security](../README.md#sandbox--security) in the README for details.

For other tools, apply input validation:

```python
# File tools: path containment check
def _resolve_path(path: str, workspace_dir: Path) -> Path:
    # Prevents path traversal (../../etc/passwd → workspaces-only)

# Web tools: URL validation
if not url.startswith(("http://", "https://")):
    return "error: invalid URL scheme"
```

### Input Validation Checklist

- **File paths:** Route through `_resolve_path()` to enforce workspace containment
- **URLs:** Validate scheme (http/https only), consider an allowlist
- **Search queries:** Sanitize length, avoid injection
- **Code execution:** Always use the sandbox, never `eval()` or `exec()` in-process
- **Rate limiting:** Consider adding per-session call counters for web_search/web_fetch

## Tool Arguments Convention

| Pattern | Convention |
|---------|------------|
| Required args | Listed in `required` array in schema |
| Optional args | Have sensible defaults in handler |
| Boolean flags | Use `True`/`False`, not `"true"`/`"false"` |
| File paths | Always resolved relative to workspace |
| Large output | Truncate with `... (truncated, original: N chars)` |
