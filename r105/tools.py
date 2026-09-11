"""Local tool executor — runs tools on the client side."""

from __future__ import annotations

import ast
import datetime
import json
import os
import platform
import re
import subprocess

# -- Tool registry (decorator-based, unified via ComponentRegistry) ----------
from collections import OrderedDict
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

import httpx

from r105 import __version__
from r105.constants import (
    TOOL_MAX_OUTPUT_CHARS,
    WEB_FETCH_MAX_CHARS,
    WEB_FETCH_TIMEOUT,
    WEB_SEARCH_MAX_RESULTS,
    WEB_SEARCH_TIMEOUT,
)
from r105.mcp_client import get_mcp_manager
from r105.plugins import get_registry
from r105.registry import ComponentRegistry, call_tool_handler
from r105.sandbox import (
    SandboxProfile,
    current_posture,
    get_sandbox,
    posture_allows_tool,
    profile_for_tool,
)
from r105.tools_math import calculate_expression, convert_units
from r105.tools_web import check_ssrf, parse_ddg_results, strip_html

_USER_AGENT = f"r105/{__version__}"

# Handler signature: (arguments: dict, workspace_dir: Path, **kwargs) -> str
ToolHandler = Callable[..., str]


@dataclass
class RegisteredTool:
    """Metadata + handler for a registered built-in tool."""

    name: str
    description: str
    parameters: dict[str, Any]
    required: list[str] = field(default_factory=list)
    needs_network: bool = False
    needs_filesystem: bool = False
    needs_output_truncation: bool = True
    needs_external_wrapping: bool = False  # XML <tool_output> tags
    handler: ToolHandler | None = None

    def to_definition(self) -> dict[str, Any]:
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": self.parameters,
                    "required": self.required,
                },
            },
        }


class ToolRegistry(ComponentRegistry["RegisteredTool"]):
    """Decorator-based registry for built-in tools (unified abstraction).

    Subclasses :class:`r105.registry.ComponentRegistry` so tools and plugins
    share shadowing protection, protected-name handling, and warning semantics.

    Usage::

        registry = ToolRegistry()

        @registry.register(
            name="my_tool",
            description="Does something useful.",
            parameters={
                "input": {"type": "string", "description": "Input value."},
            },
            required=["input"],
        )
        def my_tool(arguments: dict, workspace_dir: Path) -> str:
            return f"result: {arguments['input']}"
    """

    def __init__(self, *, allow_overwrite: bool = False) -> None:
        super().__init__(allow_overwrite=allow_overwrite)

    @property
    def _tools(self) -> dict[str, RegisteredTool]:
        # Backwards-compat: existing code touches ``registry._tools`` directly.
        return self._items

    @_tools.setter
    def _tools(self, value: dict[str, RegisteredTool]) -> None:
        self._items = value

    def register(
        self,
        name: str,
        description: str = "",
        parameters: dict[str, Any] | None = None,
        *,
        required: list[str] | None = None,
        needs_network: bool = False,
        needs_filesystem: bool = False,
        needs_output_truncation: bool = True,
        needs_external_wrapping: bool = False,
        allow_overwrite: bool | None = None,
    ) -> Callable[[ToolHandler], ToolHandler]:
        """Decorator that registers a function as a tool handler."""
        def decorator(handler: ToolHandler) -> ToolHandler:
            tool = RegisteredTool(
                name=name,
                description=description,
                parameters=parameters or {},
                required=required or [],
                needs_network=needs_network,
                needs_filesystem=needs_filesystem,
                needs_output_truncation=needs_output_truncation,
                needs_external_wrapping=needs_external_wrapping,
                handler=handler,
            )
            # Route through the unified registry so shadowing is checked.
            self.register_item(name, tool, allow_overwrite=allow_overwrite)
            return handler
        return decorator

    def get(self, name: str) -> RegisteredTool | None:
        return self._tools.get(name)

    def execute(self, name: str, arguments: dict[str, Any], workspace_dir: Path, **kwargs: Any) -> str | None:
        """Execute a registered tool. Returns None if not found."""
        tool = self._tools.get(name)
        if tool is None or tool.handler is None:
            return None
        result = call_tool_handler(tool.handler, arguments, workspace_dir, **kwargs)
        return cast("str | None", result)

    def get_definitions(self) -> list[dict[str, Any]]:
        return [t.to_definition() for t in self._tools.values()]

    def list_tools(self) -> list[RegisteredTool]:
        return list(self._tools.values())


# Module-level singleton
_TOOL_REGISTRY = ToolRegistry()


def get_tool_registry() -> ToolRegistry:
    """Return the global tool registry singleton."""
    return _TOOL_REGISTRY


# -- Limits for tool arguments ------------------------------------------

_MAX_CODE_SIZE = 100 * 1024          # 100 KB Python code
_MAX_FILE_CONTENT = 10 * 1024 * 1024 # 10 MB file write
_MAX_FILE_READ = 50 * 1024 * 1024    # 50 MB file read
_MAX_SEARCH_QUERY = 500              # chars

# -- Shared web helpers (SSRF checks, HTML stripping, DDG parsing) live in
# r105/tools_web.py so the logic exists in exactly one place. ----------------


def _validate_tool_args(name: str, arguments: dict[str, Any]) -> str | None:
    """Validate tool arguments before execution.

    Returns an error string on failure, or None on success.
    """
    if name == "execute_python":
        code = arguments.get("code", "")
        if len(code) > _MAX_CODE_SIZE:
            return f"code too large ({len(code)} bytes, max {_MAX_CODE_SIZE})"

    elif name == "write_file":
        path = arguments.get("path", "")
        if not path:
            return "path is required"
        content = arguments.get("content", "")
        if len(content) > _MAX_FILE_CONTENT:
            return f"content too large ({len(content)} bytes, max {_MAX_FILE_CONTENT})"

    elif name == "read_file":
        path = arguments.get("path", "")
        if not path:
            return "path is required"

    elif name == "web_search":
        query = arguments.get("query", "")
        if not query:
            return "query is required"
        if len(query) > _MAX_SEARCH_QUERY:
            return f"query too long ({len(query)} chars, max {_MAX_SEARCH_QUERY})"

    elif name == "web_fetch":
        url = arguments.get("url", "")
        if not url:
            return "url is required"
        err = check_ssrf(url)
        if err:
            return f"web_fetch rejected: {err}"

    elif name == "calculate":
        expression = arguments.get("expression", "")
        if not expression:
            return "expression is required"

    elif name == "convert":
        if arguments.get("value") is None or "value" not in arguments:
            return "value is required"
        if not arguments.get("from_unit"):
            return "from_unit is required"
        if not arguments.get("to_unit"):
            return "to_unit is required"

    return None


# -- Workspace resolution with session isolation --------------------------


def resolve_workspace(workspace_dir: Path, session_tag: str | None = None) -> Path:
    """Resolve workspace directory, optionally creating a session-specific subdirectory.

    When *session_tag* is provided (e.g., an ISO date or session name), tools
    operate within ``workspace_dir / session_tag /`` to prevent file collisions
    across sessions.
    """
    if session_tag:
        tagged = workspace_dir / session_tag
        tagged.mkdir(parents=True, exist_ok=True)
        return tagged
    workspace_dir.mkdir(parents=True, exist_ok=True)
    return workspace_dir


# -- Path validation (symlink-aware) ------------------------------------


def _validate_path(path: str, workspace_dir: Path) -> Path:
    """Resolve *path* and verify it stays within the workspace.

    Differs from ``_resolve_path`` by also checking every path component
    for symlinks, preventing symlink-based escapes.
    """
    resolved = _resolve_path(path, workspace_dir)

    # Walk parent components and check for symlinks
    workspace_resolved = workspace_dir.resolve()
    for parent in [resolved, *resolved.parents]:
        try:
            parent.relative_to(workspace_resolved)
        except ValueError:
            break  # reached workspace boundary

        if parent.is_symlink():
            real = parent.resolve()
            try:
                real.relative_to(workspace_resolved)
            except ValueError as err:
                raise PermissionError(
                    f"Access denied: '{path}' contains a symlink pointing "
                    f"outside the workspace ({real})"
                ) from err

    return resolved


# -- Output truncation -------------------------------------------------------


def _truncate_output(text: str, max_chars: int = TOOL_MAX_OUTPUT_CHARS) -> str:
    """Truncate tool output and append a summary note.

    Prevents context blow-up from large outputs (e.g. a 50,000-row DataFrame).
    """
    if len(text) <= max_chars:
        return text
    return f"{text[:max_chars]}\n\n[TRUNCATED: output exceeds {max_chars} chars ({len(text)} total)]"


def _head_tail_truncate(text: str, max_chars: int = TOOL_MAX_OUTPUT_CHARS, tail_ratio: float = 0.3) -> str:
    """Show head + tail of a large output, dropping the middle.

    Useful for long execution results where the head and tail are most informative.
    """
    if len(text) <= max_chars:
        return text
    tail_chars = int(max_chars * tail_ratio)
    head_chars = max_chars - tail_chars
    return (
        f"{text[:head_chars]}\n\n[... {len(text) - head_chars - tail_chars} chars omitted ...]\n\n"
        f"{text[-tail_chars:]}\n\n[TRUNCATED: output exceeds {max_chars} chars ({len(text)} total)]"
    )


# -- Prompt injection mitigation ---------------------------------------------

_TOOL_OUTPUT_TAG_BEGIN = "<tool_output>\n"
_TOOL_OUTPUT_TAG_END = "\n</tool_output>"


def _wrap_tool_output(content: str, source: str) -> str:
    """Wrap tool output in XML tags to mark it as untrusted data.

    The LLM receives tool outputs wrapped like this so it can distinguish
    between user-provided instructions and potentially malicious external
    data (e.g. from web pages or uploaded files).
    """
    return f"{_TOOL_OUTPUT_TAG_BEGIN}{content}{_TOOL_OUTPUT_TAG_END}"


# -- Tool result memoization (in-memory, per-session) -----------------------

# Bound the cache so long sessions cannot grow memory without limit.
# Oldest entries are evicted first (LRU); loading a session clears it.
_TOOL_CACHE_MAX_SIZE = 128

_tool_cache: OrderedDict[tuple[str, str], str] = OrderedDict()


def _memoize_tool(name: str, args_str: str, result: str) -> str:
    """Cache a tool result keyed by (tool_name, args). Returns the result."""
    _tool_cache[(name, args_str)] = result
    _tool_cache.move_to_end((name, args_str))
    while len(_tool_cache) > _TOOL_CACHE_MAX_SIZE:
        _tool_cache.popitem(last=False)
    return result


def _cached_result(name: str, args_str: str) -> str | None:
    """Return cached result if available, None otherwise."""
    result = _tool_cache.get((name, args_str))
    if result is not None:
        _tool_cache.move_to_end((name, args_str))
    return result


def _cache_clear() -> None:
    _tool_cache.clear()


# -- Structured output repair (malformed LLM tool arguments) ----------------


def repair_tool_arguments(raw_arguments: Any) -> dict[str, Any]:
    """Parse tool arguments with graceful repair for malformed LLM output.

    LLMs frequently emit slightly invalid JSON (trailing commas, single
    quotes, unquoted keys, markdown fences, truncated payloads). This helper
    tries ``json.loads`` first, then applies a series of low-risk repairs.
    Returns an empty dict only when nothing salvageable remains.
    """
    if isinstance(raw_arguments, dict):
        return raw_arguments
    if not isinstance(raw_arguments, str):
        return {}
    text = raw_arguments.strip()
    if not text:
        return {}
    # Strip markdown fences: ```json ... ```
    if text.startswith("```"):
        # Remove first fence line and trailing fence.
        lines = text.splitlines()
        if lines and lines[0].startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip().startswith("```"):
            lines = lines[:-1]
        text = "\n".join(lines).strip()
    try:
        parsed = json.loads(text)
        return parsed if isinstance(parsed, dict) else {}
    except json.JSONDecodeError:
        pass
    # Repair 1: trailing commas before } or ].
    repaired = re.sub(r",\s*([}\]])", r"\1", text)
    try:
        parsed = json.loads(repaired)
        if isinstance(parsed, dict):
            return parsed
    except json.JSONDecodeError:
        pass
    # Repair 2: single quotes -> double quotes (naive but effective for flat args).
    # Only attempt when the payload looks like a Python dict repr.
    if "'" in repaired and '"' not in repaired:
        try:
            parsed = ast.literal_eval(repaired)
            if isinstance(parsed, dict):
                return {str(k): v for k, v in parsed.items()}
        except (SyntaxError, ValueError):
            pass
    # Repair 3: extract the largest {...} substring (handles preamble chatter).
    start = text.find("{")
    end = text.rfind("}")
    if start != -1 and end != -1 and end > start:
        candidate = re.sub(r",\s*([}\]])", r"\1", text[start:end + 1])
        try:
            parsed = json.loads(candidate)
            if isinstance(parsed, dict):
                return parsed
        except json.JSONDecodeError:
            pass
    return {}


# -- Tool dispatch -----------------------------------------------------------


def _is_plugin_override_allowed() -> bool:
    """True when the operator explicitly allows plugins to shadow built-ins."""
    import os as _os
    if _os.environ.get("R105_ALLOW_PLUGIN_OVERRIDE", "").lower() in {"1", "true", "yes", "on"}:
        return True
    try:
        from r105.config import ensure_config as _ensure_config
        return bool(_ensure_config().get("allow_plugin_overrides", False))
    except Exception:
        return False


def _builtin_tool_names() -> set[str]:
    return set(get_tool_registry().names())


def execute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
    *,
    use_cache: bool = True,
) -> dict[str, Any]:
    """Execute a single tool call locally and return a tool result message.

    When *use_cache* is True, memoization prevents duplicate calls from
    re-running expensive operations (web_search, web_fetch, execute_python).
    """
    function = call.get("function") or {}
    name = str(function.get("name", ""))
    raw_arguments = function.get("arguments") or "{}"
    # Structured-output repair: tolerate malformed JSON from the LLM.
    try:
        arguments = (
            json.loads(raw_arguments) if isinstance(raw_arguments, str) else raw_arguments
        )
        if not isinstance(arguments, dict):
            arguments = repair_tool_arguments(raw_arguments)
    except json.JSONDecodeError:
        arguments = repair_tool_arguments(raw_arguments)
    if not isinstance(arguments, dict):
        arguments = {}
    args_str = json.dumps(arguments, sort_keys=True)

    # Enforce the active permission posture before any work happens
    allowed, reason = posture_allows_tool(current_posture(), name)
    if not allowed:
        return {
            "role": "tool",
            "tool_call_id": call.get("id", ""),
            "name": name,
            "content": f"error: {reason}",
        }

    # Validate arguments before dispatching
    error = _validate_tool_args(name, arguments)
    if error:
        return {
            "role": "tool",
            "tool_call_id": call.get("id", ""),
            "name": name,
            "content": f"validation error: {error}",
        }

    # Check cache for pure tools (not write_file)
    if use_cache and name != "write_file":
        cached = _cached_result(name, args_str)
        if cached is not None:
            return {
                "role": "tool",
                "tool_call_id": call.get("id", ""),
                "name": name,
                "content": f"{cached}\n\n[SYSTEM NOTE: This result was cached from a previous identical call.]",
            }

    # Dispatch: try built-in registry → plugins → MCP.
    # SECURITY: built-ins always win over plugins/MCP when names collide,
    # unless the operator explicitly allows overrides. This prevents a
    # malicious plugin/MCP server from hijacking e.g. ``execute_python``.
    result: str | None = None
    builtin_names = _builtin_tool_names()
    if name == "execute_python":
        result = execute_python(arguments, workspace_dir, profile=profile_for_tool(name))
    else:
        result = get_tool_registry().execute(name, arguments, workspace_dir)
    if result is None:
        # Sync the protected-name set so PluginRegistry can reject shadowing
        # at registration time as well (defense in depth).
        try:
            get_registry().set_protected_names(builtin_names)
        except Exception:
            pass
        if name in builtin_names and not _is_plugin_override_allowed():
            # A plugin/MCP tool shadows a built-in: ignore the shadow and
            # report it instead of executing untrusted code.
            plugin_shadow = get_registry().get_tool(name)
            if plugin_shadow is not None:
                result = (
                    f"error: plugin tool '{name}' shadows a built-in tool and was blocked. "
                    "Rename the plugin tool or set allow_plugin_overrides=true / "
                    "R105_ALLOW_PLUGIN_OVERRIDE=1 to allow shadowing explicitly."
                )
            else:
                plugin_result = get_registry().execute(name, arguments, workspace_dir)
                if plugin_result is not None:
                    result = plugin_result
                else:
                    mcp_result = get_mcp_manager().execute_tool(name, arguments)
                    result = mcp_result if mcp_result is not None else f"unknown tool: {name}"
        else:
            plugin_result = get_registry().execute(name, arguments, workspace_dir)
            if plugin_result is not None:
                result = plugin_result
            else:
                mcp_result = get_mcp_manager().execute_tool(name, arguments)
                if mcp_result is not None:
                    result = mcp_result
                else:
                    result = f"unknown tool: {name}"

    # Apply truncation to large outputs (execute_python, read_file, web_search, web_fetch)
    content = result if isinstance(result, str) else json.dumps(result, sort_keys=True)
    if name in ("execute_python", "read_file", "web_search", "web_fetch"):
        content = _head_tail_truncate(content)
        content = _memoize_tool(name, args_str, content)
    # Wrap outputs from external sources in untrusted XML tags (prompt injection protection)
    if name in ("web_fetch", "read_file"):
        content = _wrap_tool_output(content, name)
    return {
        "role": "tool",
        "tool_call_id": call.get("id", ""),
        "name": name,
        "content": content,
    }


def get_tool_definitions() -> list[dict[str, Any]]:
    """Return merged tool definitions: built-in registry + plugins + MCP.

    When a plugin/MCP tool shadows a built-in name it is excluded unless
    overrides are explicitly allowed — the LLM then only sees the trusted
    built-in definition.
    """
    builtin_defs = get_tool_registry().get_definitions()
    builtin_names = {d.get("function", {}).get("name") for d in builtin_defs}
    allow_override = _is_plugin_override_allowed()
    # Keep the plugin registry's protected set in sync for early rejection.
    try:
        get_registry().set_protected_names({n for n in builtin_names if isinstance(n, str)})
    except Exception:
        pass
    plugin_defs = get_registry().get_definitions()
    if not allow_override:
        plugin_defs = [d for d in plugin_defs if d.get("function", {}).get("name") not in builtin_names]
    mcp_defs = get_mcp_manager().get_all_definitions()
    if not allow_override:
        mcp_defs = [d for d in mcp_defs if d.get("function", {}).get("name") not in builtin_names]
    return [
        *builtin_defs,
        *plugin_defs,
        *mcp_defs,
    ]


# -- Python execution (sandboxed) ---------------------------------------


@get_tool_registry().register(
    name="execute_python",
    description="Execute Python code and return stdout or stderr.",
    parameters={"code": {"type": "string", "description": "Python source code to execute."}},
    required=["code"],
    needs_network=False,
    needs_filesystem=False,
    needs_output_truncation=True,
)
def execute_python(
    arguments: dict[str, Any],
    workspace_dir: Path,
    profile: SandboxProfile | None = None,
) -> str:
    """Execute Python code in a sandboxed subprocess.

    Uses the configured sandbox backend (nsjail > bwrap > rlimit > none).
    The optional *profile* controls isolation level. See r105/sandbox.py.
    """
    code = arguments.get("code", "")
    if not code:
        return "error: no code provided"

    try:
        proc = get_sandbox().execute(code, profile=profile)
        if proc.returncode == 0:
            return proc.stdout
        if proc.returncode < 0:
            return f"error: process killed by signal {-proc.returncode}"
        return proc.stderr or f"error: exit code {proc.returncode}"
    except subprocess.TimeoutExpired:
        return "error: execution timed out (30s)"
    except FileNotFoundError:
        return "error: sandbox backend not available (missing executable)"
    except Exception as e:
        return str(e)


# -- File operations ----------------------------------------------------


def _make_diff(file_path: Path, new_content: str, context_lines: int = 3) -> str:
    """Generate a unified diff between the current file and proposed content.

    Returns an empty string if the file doesn't exist yet (new file).
    """
    if not file_path.is_file():
        return ""
    try:
        old_content = file_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""

    if old_content == new_content:
        return ""

    import difflib
    old_lines = old_content.splitlines(keepends=True)
    new_lines = new_content.splitlines(keepends=True)
    diff = difflib.unified_diff(
        old_lines, new_lines,
        fromfile=str(file_path),
        tofile=str(file_path),
        n=context_lines,
    )
    return "".join(diff)


@get_tool_registry().register(
    name="write_file",
    description="Write content to a file in the workspace.",
    parameters={
        "path": {"type": "string", "description": "File path (relative to workspace or absolute)."},
        "content": {"type": "string", "description": "File content to write."},
    },
    required=["path", "content"],
    needs_network=False,
    needs_filesystem=True,
    needs_output_truncation=False,
)
def write_file(
    arguments: dict[str, Any],
    workspace_dir: Path,
    *,
    dry_run: bool = False,
) -> str:
    """Write content to a file in the workspace.

    If *dry_run* is True, returns the diff without writing.
    If the file already exists, returns a unified diff of the changes.
    For new files, returns a creation notice.
    """
    path = arguments.get("path", "")
    content = arguments.get("content", "")
    try:
        file_path = _validate_path(path, workspace_dir)
        file_path.parent.mkdir(parents=True, exist_ok=True)

        diff = _make_diff(file_path, content)

        if dry_run:
            if diff:
                return f"diff for {file_path}:\n{diff}"
            if not file_path.is_file():
                return f"would create new file: {file_path} ({len(content)} bytes)"
            return f"no changes for {file_path}"

        # Actually write
        if diff:
            is_new = False
        else:
            is_new = not file_path.is_file()

        file_path.write_text(content, encoding="utf-8")

        if is_new:
            return f"created {file_path} ({len(content)} bytes)"
        if diff:
            return f"wrote {len(content)} bytes to {file_path} (diff above)"
        return f"wrote {len(content)} bytes to {file_path}"
    except Exception as e:
        return str(e)


@get_tool_registry().register(
    name="read_file",
    description="Read the contents of a file.",
    parameters={"path": {"type": "string", "description": "File path to read."}},
    required=["path"],
    needs_network=False,
    needs_filesystem=True,
    needs_output_truncation=True,
    needs_external_wrapping=True,
)
def read_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    try:
        file_path = _validate_path(path, workspace_dir)
        # Guard against reading huge files
        if file_path.stat().st_size > _MAX_FILE_READ:
            return f"error: file too large ({file_path.stat().st_size} bytes, max {_MAX_FILE_READ})"
        return file_path.read_text(encoding="utf-8", errors="replace")
    except Exception as e:
        return str(e)


@get_tool_registry().register(
    name="list_files",
    description="List files in a directory.",
    parameters={"path": {"type": "string", "description": "Directory path to list (default: workspace root)."}},
    needs_network=False,
    needs_filesystem=True,
    needs_output_truncation=True,
)
def list_files(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", ".")
    try:
        target = _validate_path(path, workspace_dir)
        if not target.exists():
            return f"path not found: {target}"
        entries = []
        for f in sorted(target.iterdir()):
            kind = "dir" if f.is_dir() else "file"
            size = f.stat().st_size
            entries.append(f"{f.name} ({kind}, {size} bytes)")
        return "\n".join(entries) if entries else "empty directory"
    except Exception as e:
        return str(e)


def _resolve_path(path: str, workspace_dir: Path) -> Path:
    """Resolve a path safely within the workspace directory.

    Absolute paths are treated as relative to the workspace root to prevent
    path traversal attacks. Relative paths stay within the workspace.

    Raises PermissionError if the resolved path escapes the workspace.
    """
    p = Path(path)
    workspace_resolved = workspace_dir.resolve()

    if p.is_absolute():
        # Strip the root anchor and force it relative to the workspace
        try:
            resolved = (workspace_resolved / p.relative_to(p.anchor)).resolve()
        except ValueError:
            resolved = (workspace_resolved / p.name).resolve()
    else:
        resolved = (workspace_resolved / p).resolve()

    # Strict containment check — no path may escape the workspace
    try:
        resolved.relative_to(workspace_resolved)
    except ValueError as err:
        raise PermissionError(
            f"Access denied: '{path}' resolves outside the workspace ({workspace_resolved})"
        ) from err

    return resolved


# -- Web tools ----------------------------------------------------------

@get_tool_registry().register(
    name="web_search",
    description="Search the web and return results with titles, URLs, and snippets.",
    parameters={"query": {"type": "string", "description": "Search query string."}},
    required=["query"],
    needs_network=True,
    needs_filesystem=False,
    needs_output_truncation=True,
)
def web_search(arguments: dict[str, Any]) -> str:
    """Search the web using DuckDuckGo HTML (no API key required)."""
    query = arguments.get("query", "")
    if not query:
        return "error: query is required"

    try:
        response = httpx.get(
            "https://html.duckduckgo.com/html/",
            params={"q": query},
            timeout=WEB_SEARCH_TIMEOUT,
            headers={"User-Agent": _USER_AGENT},
            follow_redirects=True,
        )
        response.raise_for_status()
        results = parse_ddg_results(response.text, WEB_SEARCH_MAX_RESULTS)
        if not results:
            return f"no results found for: {query}"
        return json.dumps(results, indent=2, ensure_ascii=False)
    except httpx.HTTPError as e:
        return f"search error: {e}"
    except Exception as e:
        return f"search error: {e}"


@get_tool_registry().register(
    name="web_fetch",
    description="Fetch a URL and return its text content (HTML tags removed).",
    parameters={
        "url": {"type": "string", "description": "URL to fetch."},
        "max_length": {"type": "integer", "description": "Maximum characters to return (default: 8000)."},
    },
    required=["url"],
    needs_network=True,
    needs_filesystem=False,
    needs_output_truncation=True,
    needs_external_wrapping=True,
)
def web_fetch(arguments: dict[str, Any]) -> str:
    """Fetch a URL and return its text content (HTML tags stripped)."""
    url = arguments.get("url", "")
    max_length = arguments.get("max_length", WEB_FETCH_MAX_CHARS)
    if not url:
        return "error: url is required"

    try:
        response = httpx.get(
            url,
            timeout=WEB_FETCH_TIMEOUT,
            headers={"User-Agent": _USER_AGENT},
            follow_redirects=True,
        )
        response.raise_for_status()
        text = strip_html(response.text)
        if len(text) > max_length:
            text = text[:max_length] + f"\n... (truncated, original: {len(text)} chars)"
        return text
    except httpx.HTTPError as e:
        return f"fetch error: {e}"
    except Exception as e:
        return f"fetch error: {e}"


# -- Utility tools ------------------------------------------------------


@get_tool_registry().register(
    name="get_time",
    description="Return the current system time in ISO 8601 format.",
    parameters={},
    needs_network=False,
    needs_filesystem=False,
    needs_output_truncation=False,
)
def get_time() -> str:
    """Return current system time in ISO format."""
    return datetime.datetime.now().isoformat()


@get_tool_registry().register(
    name="calculate",
    description=(
        "Safely evaluate a mathematical expression (+, -, *, /, **, %, "
        "parentheses, math functions like sqrt/sin/log, constants pi/e/tau)."
    ),
    parameters={"expression": {"type": "string", "description": "Arithmetic expression to evaluate."}},
    required=["expression"],
    needs_network=False,
    needs_filesystem=False,
    needs_output_truncation=False,
)
def calculate(arguments: dict[str, Any]) -> str:
    """Safely evaluate a mathematical expression (delegates to tools_math)."""
    expression = arguments.get("expression", "")
    if not expression:
        return "error: expression is required"
    return calculate_expression(expression)


@get_tool_registry().register(
    name="convert",
    description=(
        "Convert a value between units (length, mass, time, data, speed, "
        "volume, temperature). E.g. value=5, from_unit='km', to_unit='mi'."
    ),
    parameters={
        "value": {"type": "number", "description": "Numeric value to convert."},
        "from_unit": {"type": "string", "description": "Source unit (e.g. 'km', 'lb', 'C')."},
        "to_unit": {"type": "string", "description": "Target unit (e.g. 'mi', 'kg', 'F')."},
    },
    required=["value", "from_unit", "to_unit"],
    needs_network=False,
    needs_filesystem=False,
    needs_output_truncation=False,
)
def convert(arguments: dict[str, Any]) -> str:
    """Convert between units (delegates to tools_math)."""
    try:
        value = float(arguments.get("value", ""))
    except (TypeError, ValueError):
        return f"convert error: value must be a number, got {arguments.get('value')!r}"
    from_unit = str(arguments.get("from_unit", ""))
    to_unit = str(arguments.get("to_unit", ""))
    if not from_unit or not to_unit:
        return "convert error: from_unit and to_unit are required"
    return convert_units(value, from_unit, to_unit)


@get_tool_registry().register(
    name="system_info",
    description="Return basic OS and hardware information as JSON.",
    parameters={},
    needs_network=False,
    needs_filesystem=False,
    needs_output_truncation=False,
)
def system_info() -> str:
    """Return basic OS and hardware information as JSON."""
    import socket
    info = {
        "platform": platform.platform(),
        "python_version": platform.python_version(),
        "cpu_count": os.cpu_count(),
        "hostname": socket.gethostname(),
        "machine": platform.machine(),
    }
    return json.dumps(info, indent=2, sort_keys=True)


# Backward-compatible alias — TOOL_DEFINITIONS from the registry
TOOL_DEFINITIONS: list[dict[str, Any]] = []


def _populate_tool_definitions() -> None:
    global TOOL_DEFINITIONS
    TOOL_DEFINITIONS[:] = get_tool_registry().get_definitions()


# Populate TOOL_DEFINITIONS at module load time
_populate_tool_definitions()

