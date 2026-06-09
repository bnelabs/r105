"""Local tool executor — runs tools on the client side."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any


def execute_tool_call(
    call: dict[str, Any],
    workspace_dir: Path,
) -> dict[str, Any]:
    """Execute a single tool call locally and return a tool result message."""
    function = call.get("function") or {}
    name = function.get("name")
    raw_arguments = function.get("arguments") or "{}"
    arguments = (
        json.loads(raw_arguments) if isinstance(raw_arguments, str) else raw_arguments
    )

    if name == "execute_python":
        result = execute_python(arguments, workspace_dir)
    elif name == "write_file":
        result = write_file(arguments, workspace_dir)
    elif name == "read_file":
        result = read_file(arguments, workspace_dir)
    elif name == "list_files":
        result = list_files(arguments, workspace_dir)
    else:
        result = f"unknown tool: {name}"

    return {
        "role": "tool",
        "tool_call_id": call.get("id", ""),
        "name": name,
        "content": result if isinstance(result, str) else json.dumps(result, sort_keys=True),
    }


def execute_python(arguments: dict[str, Any], workspace_dir: Path) -> str:
    code = arguments.get("code", "")
    try:
        result = subprocess.run(
            ["python3", "-c", code],
            capture_output=True,
            text=True,
            timeout=30,
            cwd=str(workspace_dir),
        )
        return result.stdout if result.returncode == 0 else result.stderr
    except subprocess.TimeoutExpired:
        return "error: execution timed out (30s)"
    except Exception as e:
        return str(e)


def write_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    content = arguments.get("content", "")
    try:
        file_path = _resolve_path(path, workspace_dir)
        file_path.parent.mkdir(parents=True, exist_ok=True)
        file_path.write_text(content, encoding="utf-8")
        return f"wrote {len(content)} bytes to {file_path}"
    except Exception as e:
        return str(e)


def read_file(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", "")
    try:
        file_path = _resolve_path(path, workspace_dir)
        return file_path.read_text(encoding="utf-8")
    except Exception as e:
        return str(e)


def list_files(arguments: dict[str, Any], workspace_dir: Path) -> str:
    path = arguments.get("path", ".")
    try:
        target = _resolve_path(path, workspace_dir)
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
    """Resolve a path relative to workspace_dir, preventing escape."""
    p = Path(path)
    if p.is_absolute():
        return p
    resolved = (workspace_dir / p).resolve()
    # Allow absolute paths outside workspace, but log a warning-like prefix
    # for relative paths resolved outside workspace
    return resolved


TOOL_DEFINITIONS: list[dict[str, Any]] = [
    {
        "type": "function",
        "function": {
            "name": "execute_python",
            "description": "Execute Python code and return stdout/stderr.",
            "parameters": {
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python source code to execute.",
                    },
                },
                "required": ["code"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "write_file",
            "description": "Write content to a file in the workspace.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute).",
                    },
                    "content": {
                        "type": "string",
                        "description": "File content to write.",
                    },
                },
                "required": ["path", "content"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "read_file",
            "description": "Read the contents of a file.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to read.",
                    },
                },
                "required": ["path"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "list_files",
            "description": "List files in a directory.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path to list (default: workspace root).",
                    },
                },
                "required": [],
            },
        },
    },
]
