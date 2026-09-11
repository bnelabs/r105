"""Local tool executor — runs tools on the client side."""

from __future__ import annotations

__all__ = [
    # Re-exported from r105.registry (backwards-compat import location).
    "RegisteredTool",
    "ToolHandler",
    "ToolRegistry",
    "aexecute_tool_call",
    "execute_tool_call",
    "get_tool_definitions",
    "get_tool_registry",
]

import ast
import asyncio
import datetime
import json
import os
import platform
import re
import subprocess
import time

# -- Tool registry (decorator-based, unified via ComponentRegistry) ----------
from collections import OrderedDict
from pathlib import Path
from typing import Any

from r105 import tools_security as _security
from r105.constants import TOOL_MAX_OUTPUT_CHARS
from r105.logging import info as log_info
from r105.plugins import get_registry
from r105.registry import (
    RegisteredTool,
    ToolHandler,
    ToolRegistry,
    get_tool_registry,
)
from r105.sandbox import (
    SandboxProfile,
    current_posture,
    get_sandbox,
    posture_allows_tool,
    profile_for_tool,
)
from r105.tools_math import calculate_expression, convert_units

_MAX_FILE_READ = _security.MAX_FILE_READ
_is_plugin_override_allowed = _security.plugin_override_allowed
_resolve_path = _security.resolve_path
_validate_path = _security.validate_path
_validate_tool_args = _security.validate_tool_args
resolve_workspace = _security.resolve_workspace


def _mcp_manager() -> Any:
    """Load MCP support only when a tool call actually needs it."""
    from r105.mcp_client import get_mcp_manager

    return get_mcp_manager()

# -- Shared web helpers (SSRF checks, HTML stripping, DDG parsing) live in
# r105/tools_web.py so the logic exists in exactly one place. ----------------


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


def _builtin_tool_names() -> set[str]:
    return set(get_tool_registry().names())


# -- execute_python confirmation gate --------------------------------------
# Code execution needs a one-time approval per process: ``/approve
# execute_python`` in the TUI, ``--yes`` on the CLI, or the
# ``auto_approve_execute_python`` config key. The sandbox still applies;
# this gate is about intent, not isolation.

_EXECUTE_PYTHON_APPROVED = False
_EXECUTE_PYTHON_AUTO_APPROVE = False


def approve_execute_python() -> None:
    """Record one-time approval for execute_python (this process)."""
    global _EXECUTE_PYTHON_APPROVED
    _EXECUTE_PYTHON_APPROVED = True


def set_execute_python_auto_approve(enabled: bool) -> None:
    """Bypass the confirmation gate (``--yes`` / config)."""
    global _EXECUTE_PYTHON_AUTO_APPROVE
    _EXECUTE_PYTHON_AUTO_APPROVE = bool(enabled)


def reset_execute_python_approval() -> None:
    """Clear approval state (tests / session reset)."""
    global _EXECUTE_PYTHON_APPROVED, _EXECUTE_PYTHON_AUTO_APPROVE
    _EXECUTE_PYTHON_APPROVED = False
    _EXECUTE_PYTHON_AUTO_APPROVE = False


def execute_python_needs_approval() -> bool:
    """True when an execute_python call would be gated right now."""
    return not (_EXECUTE_PYTHON_APPROVED or _EXECUTE_PYTHON_AUTO_APPROVE)


_EXECUTE_PYTHON_GATE_MESSAGE = (
    "execute_python needs one-time approval before code can run. "
    "Reply with /approve execute_python (this session), restart with --yes, "
    "or set auto_approve_execute_python in config.json."
)


# -- Web tool rate limiting -------------------------------------------------
# Token-bucket-ish guard: at most N *executed* calls per rolling window per
# tool. Cache hits don't consume budget. Prevents runaway loops from
# hammering external services.

_WEB_RATE_LIMITS: dict[str, tuple[int, float]] = {
    "web_search": (30, 60.0),
    "web_fetch": (60, 60.0),
}

_web_call_times: dict[str, list[float]] = {}


def reset_rate_limits() -> None:
    """Clear recorded web-tool call timestamps (tests)."""
    _web_call_times.clear()


def _check_rate_limit(name: str) -> str | None:
    """Record a call and return an error when the budget is exhausted."""
    limit = _WEB_RATE_LIMITS.get(name)
    if limit is None:
        return None
    max_calls, window = limit
    now = time.monotonic()
    recent = [t for t in _web_call_times.get(name, []) if now - t < window]
    if len(recent) >= max_calls:
        return (
            f"error: {name} rate limited ({max_calls} calls per {window:.0f}s) — "
            "try a different approach or wait before retrying"
        )
    recent.append(now)
    _web_call_times[name] = recent
    return None


def execute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
    *,
    use_cache: bool = True,
    trace_id: str | None = None,
) -> dict[str, Any]:
    """Execute a single tool call locally and return a tool result message.

    When *use_cache* is True, memoization prevents duplicate calls from
    re-running expensive operations (web_search, web_fetch, execute_python).
    """
    function = call.get("function") or {}
    name = str(function.get("name", ""))
    log_info("tool_started", tool=name, trace_id=trace_id)
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

    # Confirmation gate for code execution (intent, not isolation —
    # the sandbox still applies once approved).
    if name == "execute_python" and execute_python_needs_approval():
        return {
            "role": "tool",
            "tool_call_id": call.get("id", ""),
            "name": name,
            "content": _EXECUTE_PYTHON_GATE_MESSAGE,
        }

    # Rate-limit web tools (cache hits above already bypassed this).
    rate_error = _check_rate_limit(name)
    if rate_error is not None:
        return {
            "role": "tool",
            "tool_call_id": call.get("id", ""),
            "name": name,
            "content": rate_error,
        }

    # Dispatch through the registry. Built-ins win over plugins/MCP by
    # default; an explicit override opt-in lets a plugin replace a built-in,
    # including execute_python. Keep the sandbox profile on the built-in
    # registry call so a plugin never receives an unexpected keyword argument.
    result: str | None = None
    builtin_names = _builtin_tool_names()
    builtin_kwargs: dict[str, Any] = {}
    if name == "execute_python":
        builtin_kwargs["profile"] = profile_for_tool(name)
    allow_override = _is_plugin_override_allowed()
    plugin_attempted = False
    if allow_override:
        plugin_attempted = True
        result = get_registry().execute(name, arguments, workspace_dir)
        if result is None:
            result = get_tool_registry().execute(
                name, arguments, workspace_dir, **builtin_kwargs
            )
    else:
        result = get_tool_registry().execute(name, arguments, workspace_dir, **builtin_kwargs)
    if result is None:
        # Sync the protected-name set so PluginRegistry can reject shadowing
        # at registration time as well (defense in depth).
        try:
            get_registry().set_protected_names(builtin_names)
        except Exception:
            pass
        if name in builtin_names and not allow_override:
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
                    mcp_result = _mcp_manager().execute_tool(name, arguments)
                    result = mcp_result if mcp_result is not None else f"unknown tool: {name}"
        else:
            plugin_result = (
                None
                if plugin_attempted
                else get_registry().execute(name, arguments, workspace_dir)
            )
            if plugin_result is not None:
                result = plugin_result
            else:
                mcp_result = _mcp_manager().execute_tool(name, arguments)
                result = mcp_result if mcp_result is not None else f"unknown tool: {name}"

    # Apply truncation to large outputs (execute_python, read_file, web_search, web_fetch)
    content = result if isinstance(result, str) else json.dumps(result, sort_keys=True)
    if name in ("execute_python", "read_file", "web_search", "web_fetch"):
        content = _head_tail_truncate(content)
        content = _memoize_tool(name, args_str, content)
    # Wrap outputs from external sources in untrusted XML tags (prompt injection protection)
    if name in ("web_fetch", "read_file"):
        content = _wrap_tool_output(content, name)
    response = {
        "role": "tool",
        "tool_call_id": call.get("id", ""),
        "name": name,
        "content": content,
    }
    log_info("tool_completed", tool=name, trace_id=trace_id)
    return response


async def aexecute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
    *,
    use_cache: bool = True,
    trace_id: str | None = None,
) -> dict[str, Any]:
    """Execute a synchronous local tool without blocking the event loop."""
    return await asyncio.to_thread(
        execute_tool_call,
        call,
        workspace_dir,
        use_cache=use_cache,
        trace_id=trace_id,
    )


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
    plugin_names = {
        d.get("function", {}).get("name")
        for d in plugin_defs
        if isinstance(d.get("function", {}).get("name"), str)
    }
    if allow_override:
        # An explicit plugin override must also replace the built-in schema;
        # exposing two definitions with the same name leaves model behavior
        # dependent on provider-specific duplicate handling.
        builtin_defs = [
            d for d in builtin_defs
            if d.get("function", {}).get("name") not in plugin_names
        ]
    else:
        plugin_defs = [d for d in plugin_defs if d.get("function", {}).get("name") not in builtin_names]
    mcp_defs = _mcp_manager().get_all_definitions()
    if allow_override:
        mcp_defs = [
            d for d in mcp_defs
            if d.get("function", {}).get("name") not in plugin_names
        ]
    else:
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
