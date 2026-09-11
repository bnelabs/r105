"""Security policy and validation helpers shared by local tools.

Keeping argument validation, workspace containment, and plugin override policy
outside the tool dispatcher makes those decisions auditable independently of
execution and registration code.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from r105.tools_web import check_ssrf

MAX_CODE_SIZE = 100 * 1024
MAX_FILE_CONTENT = 10 * 1024 * 1024
MAX_FILE_READ = 50 * 1024 * 1024
MAX_SEARCH_QUERY = 500


def validate_tool_args(name: str, arguments: dict[str, Any]) -> str | None:
    """Validate arguments before a tool is allowed to execute."""
    if name == "execute_python":
        code = arguments.get("code", "")
        if len(code) > MAX_CODE_SIZE:
            return f"code too large ({len(code)} bytes, max {MAX_CODE_SIZE})"
    elif name == "write_file":
        path = arguments.get("path", "")
        if not path:
            return "path is required"
        content = arguments.get("content", "")
        if len(content) > MAX_FILE_CONTENT:
            return f"content too large ({len(content)} bytes, max {MAX_FILE_CONTENT})"
    elif name == "read_file":
        if not arguments.get("path", ""):
            return "path is required"
    elif name == "web_search":
        query = arguments.get("query", "")
        if not query:
            return "query is required"
        if len(query) > MAX_SEARCH_QUERY:
            return f"query too long ({len(query)} chars, max {MAX_SEARCH_QUERY})"
    elif name == "web_fetch":
        url = arguments.get("url", "")
        if not url:
            return "url is required"
        error = check_ssrf(url)
        if error:
            return f"web_fetch rejected: {error}"
    elif name == "calculate":
        if not arguments.get("expression", ""):
            return "expression is required"
    elif name == "convert":
        if arguments.get("value") is None or "value" not in arguments:
            return "value is required"
        if not arguments.get("from_unit"):
            return "from_unit is required"
        if not arguments.get("to_unit"):
            return "to_unit is required"
    return None


def resolve_workspace(workspace_dir: Path, session_tag: str | None = None) -> Path:
    """Resolve a workspace, optionally isolating one session subdirectory."""
    if session_tag:
        tagged = workspace_dir / session_tag
        tagged.mkdir(parents=True, exist_ok=True)
        return tagged
    workspace_dir.mkdir(parents=True, exist_ok=True)
    return workspace_dir


def resolve_path(path: str, workspace_dir: Path) -> Path:
    """Resolve a path while forcing it to remain under the workspace."""
    candidate = Path(path)
    workspace_resolved = workspace_dir.resolve()

    if candidate.is_absolute():
        try:
            resolved = (workspace_resolved / candidate.relative_to(candidate.anchor)).resolve()
        except ValueError:
            resolved = (workspace_resolved / candidate.name).resolve()
    else:
        resolved = (workspace_resolved / candidate).resolve()

    try:
        resolved.relative_to(workspace_resolved)
    except ValueError as error:
        raise PermissionError(
            f"Access denied: '{path}' resolves outside the workspace ({workspace_resolved})"
        ) from error
    return resolved


def validate_path(path: str, workspace_dir: Path) -> Path:
    """Resolve a path and reject symlink components escaping the workspace."""
    resolved = resolve_path(path, workspace_dir)
    workspace_resolved = workspace_dir.resolve()
    for parent in [resolved, *resolved.parents]:
        try:
            parent.relative_to(workspace_resolved)
        except ValueError:
            break
        if parent.is_symlink():
            real = parent.resolve()
            try:
                real.relative_to(workspace_resolved)
            except ValueError as error:
                raise PermissionError(
                    f"Access denied: '{path}' contains a symlink pointing "
                    f"outside the workspace ({real})"
                ) from error
    return resolved


def plugin_override_allowed() -> bool:
    """Return whether the operator explicitly allows plugin shadowing."""
    if os.environ.get("R105_ALLOW_PLUGIN_OVERRIDE", "").lower() in {
        "1", "true", "yes", "on"
    }:
        return True
    try:
        from r105.config import ensure_config

        return bool(ensure_config().get("allow_plugin_overrides", False))
    except Exception:
        return False


# Private aliases retain the old import names for integrations that used the
# module's internal helpers while the public implementation lives here.
_validate_tool_args = validate_tool_args
_resolve_path = resolve_path
_validate_path = validate_path
_is_plugin_override_allowed = plugin_override_allowed


__all__ = [
    "MAX_CODE_SIZE",
    "MAX_FILE_CONTENT",
    "MAX_FILE_READ",
    "MAX_SEARCH_QUERY",
    "plugin_override_allowed",
    "resolve_path",
    "resolve_workspace",
    "validate_path",
    "validate_tool_args",
]
