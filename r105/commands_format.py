"""Shared formatting helpers for slash commands.

Extracted from ``r105/commands.py`` so presentation logic can be tested and
reused without importing the full async dispatch table. ``commands.py``
re-exports these for backwards compatibility.
"""

from __future__ import annotations

from pathlib import Path

from r105.state import ChatState, token_usage


def format_state(state: ChatState) -> str:
    usage = token_usage(state)
    return (
        f"profile={state.profile or 'auto'} "
        f"quality={state.quality or 'auto'} "
        f"max_tokens={state.max_tokens or 'auto'} "
        f"json={state.json_mode} "
        f"skills={','.join(state.active_skills) if state.active_skills else 'none'} "
        f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%) "
        f"turns={len(state.history) // 2}"
    )


def status_line(state: ChatState) -> str:
    usage = token_usage(state)
    return f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)"


def format_history(state: ChatState, limit: int = 12) -> str:
    if not state.history:
        return "history empty"
    lines: list[str] = []
    for index, message in enumerate(state.history[-limit:], start=max(1, len(state.history) - limit + 1)):
        role = message.get("role", "unknown")
        content = " ".join(str(message.get("content", "")).split())
        lines.append(f"{index}. {role}: {content[:220]}")
    return "\n".join(lines)


def format_skills(names: list[str]) -> str:
    return "\n".join(names) if names else "no skills"


def format_workspace(workspace_dir: Path) -> str:
    if not workspace_dir.exists():
        return f"workspace dir does not exist: {workspace_dir}"
    files = sorted(workspace_dir.iterdir())
    if not files:
        return f"workspace empty: {workspace_dir}"
    lines = [f"workspace: {workspace_dir}"]
    for f in files:
        if f.name == ".gitkeep":
            continue
        size = f.stat().st_size
        lines.append(f"  {f.name}  ({human_size(size)})")
    return "\n".join(lines)


def human_size(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024:
            return f"{size}{unit}"
        size //= 1024
    return f"{size}TB"
