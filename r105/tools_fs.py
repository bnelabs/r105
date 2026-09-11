"""Filesystem helpers for tools — path resolution, validation, and diffs.

Extracted from ``r105/tools.py`` to keep the dispatcher module focused.
Pure helpers only (no registry imports) so there are no import cycles:
``r105/tools.py`` imports from here.
"""

from __future__ import annotations

import difflib
from pathlib import Path


def resolve_path(path: str, workspace_dir: Path) -> Path:
    """Resolve *path* safely within the workspace directory.

    Absolute paths are treated as relative to the workspace root to prevent
    path traversal attacks. Relative paths stay within the workspace.

    Raises PermissionError if the resolved path escapes the workspace.
    """
    p = Path(path)
    workspace_resolved = workspace_dir.resolve()

    if p.is_absolute():
        try:
            resolved = (workspace_resolved / p.relative_to(p.anchor)).resolve()
        except ValueError:
            resolved = (workspace_resolved / p.name).resolve()
    else:
        resolved = (workspace_resolved / p).resolve()

    try:
        resolved.relative_to(workspace_resolved)
    except ValueError as err:
        raise PermissionError(
            f"Access denied: '{path}' resolves outside the workspace ({workspace_resolved})"
        ) from err

    return resolved


def validate_path(path: str, workspace_dir: Path) -> Path:
    """Resolve *path* and verify it stays within the workspace (symlink-aware)."""
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
            except ValueError as err:
                raise PermissionError(
                    f"Access denied: '{path}' contains a symlink pointing "
                    f"outside the workspace ({real})"
                ) from err
    return resolved


def make_diff(file_path: Path, new_content: str, context_lines: int = 3) -> str:
    """Generate a unified diff between the current file and proposed content."""
    if not file_path.is_file():
        return ""
    try:
        old_content = file_path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    if old_content == new_content:
        return ""
    old_lines = old_content.splitlines(keepends=True)
    new_lines = new_content.splitlines(keepends=True)
    diff = difflib.unified_diff(
        old_lines,
        new_lines,
        fromfile=str(file_path),
        tofile=str(file_path),
        n=context_lines,
    )
    return "".join(diff)


def human_size(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024:
            return f"{size}{unit}"
        size //= 1024
    return f"{size}TB"
