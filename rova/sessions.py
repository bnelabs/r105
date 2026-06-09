"""Session persistence — save, load, list, and export conversations."""

from __future__ import annotations

import datetime
import json
from pathlib import Path
from typing import Any

from rova.config import CONFIG_DIR
from rova.state import ChatState

SESSION_DIR = CONFIG_DIR / "sessions"


def _ensure_dir() -> None:
    SESSION_DIR.mkdir(parents=True, exist_ok=True)


def _session_path(name: str) -> Path:
    # Sanitize: prevent path traversal
    safe = name.replace("/", "_").replace("\\", "_").replace("..", "_")
    if not safe:
        safe = "unnamed"
    return SESSION_DIR / f"{safe}.json"


def _serializable_state(state: ChatState) -> dict[str, Any]:
    """Extract session-relevant fields from ChatState."""
    return {
        "profile": state.profile,
        "rag": state.rag,
        "quality": state.quality,
        "max_tokens": state.max_tokens,
        "json_mode": state.json_mode,
        "active_skills": state.active_skills,
        "skill_params": state.skill_params,
    }


def _restore_state(state: ChatState, data: dict[str, Any]) -> None:
    """Restore session state into a ChatState object."""
    saved = data.get("state") or {}
    state.profile = saved.get("profile")
    state.rag = saved.get("rag")
    state.quality = saved.get("quality")
    state.max_tokens = saved.get("max_tokens")
    state.json_mode = saved.get("json_mode", False)
    state.active_skills = saved.get("active_skills") or []
    state.skill_params = saved.get("skill_params") or {}


def save_session(state: ChatState, name: str) -> Path:
    """Save the current conversation to a session file.

    Returns the path to the saved file.
    """
    _ensure_dir()
    path = _session_path(name)

    data: dict[str, Any] = {
        "history": state.history,
        "state": _serializable_state(state),
        "message_count": len(state.history),
        "saved_at": datetime.datetime.now().isoformat(),
    }

    path.write_text(json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    return path


def load_session(state: ChatState, name: str) -> int:
    """Load a saved session, replacing the current conversation history.

    Returns the number of messages loaded.

    Raises FileNotFoundError if the session doesn't exist.
    Raises json.JSONDecodeError if the file is corrupted.
    """
    path = _session_path(name)
    if not path.is_file():
        raise FileNotFoundError(f"session not found: {name}")

    data = json.loads(path.read_text(encoding="utf-8"))
    history = data.get("history") or []

    state.history.clear()
    state.history.extend(history)
    _restore_state(state, data)

    return len(history)


def list_sessions() -> list[dict[str, Any]]:
    """Return a list of saved sessions with metadata.

    Each entry is a dict with: name, saved_at, message_count, preview.
    Sorted by most recently saved first.
    """
    _ensure_dir()
    sessions: list[dict[str, Any]] = []

    for path in sorted(SESSION_DIR.glob("*.json"), key=lambda p: p.stat().st_mtime, reverse=True):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue

        history = data.get("history") or []
        preview = ""
        for msg in history:
            if msg.get("role") == "user":
                content = str(msg.get("content", ""))
                preview = content[:80] + ("…" if len(content) > 80 else "")
                if preview:
                    break

        sessions.append({
            "name": path.stem,
            "saved_at": data.get("saved_at", "unknown"),
            "message_count": len(history),
            "preview": preview,
        })

    return sessions


def delete_session(name: str) -> bool:
    """Delete a saved session file. Returns True if deleted, False if not found."""
    path = _session_path(name)
    if not path.is_file():
        return False
    path.unlink()
    return True


# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------


def export_conversation(state: ChatState, fmt: str = "markdown") -> str:
    """Render conversation history as a string in the requested format.

    Supported formats: markdown, json, html.
    """
    if fmt == "json":
        return json.dumps(state.history, indent=2, sort_keys=True, ensure_ascii=False)

    if fmt == "html":
        return _export_html(state)

    # Default: markdown
    return _export_markdown(state)


def _export_markdown(state: ChatState) -> str:
    """Render conversation as Markdown."""
    lines: list[str] = [
        "# Rova Conversation",
        f"Exported: {datetime.datetime.now().isoformat()}",
        f"Messages: {len(state.history)}",
        "",
    ]

    for i, msg in enumerate(state.history, 1):
        role = msg.get("role", "unknown").upper()
        content = str(msg.get("content", ""))

        if role == "TOOL":
            lines.append(f"### {i}. {role} — {msg.get('name', 'unknown')}")
            lines.append("")
            lines.append("```json")
            lines.append(content[:2000])
            lines.append("```")
        elif role == "ASSISTANT" and msg.get("tool_calls"):
            lines.append(f"### {i}. {role} (tool calls)")
            lines.append("")
            lines.append(content)
            lines.append("")
            lines.append("```json")
            lines.append(json.dumps(msg["tool_calls"], indent=2))
            lines.append("```")
        else:
            lines.append(f"### {i}. {role}")
            lines.append("")
            lines.append(content)

        lines.append("")
        lines.append("---")
        lines.append("")

    return "\n".join(lines)


def _export_html(state: ChatState) -> str:
    """Render conversation as a styled HTML page."""
    messages_html: list[str] = []

    for msg in state.history:
        role = msg.get("role", "unknown")
        content = str(msg.get("content", "")).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
        css_class = f"message-{role}"

        messages_html.append(f'<div class="message {css_class}">')
        messages_html.append(f'<div class="role">{role.upper()}</div>')
        messages_html.append(f'<div class="content"><pre>{content}</pre></div>')
        messages_html.append("</div>")

    return f"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Rova Conversation</title>
<style>
body {{ font-family: system-ui, sans-serif; max-width: 800px; margin: 2em auto; background: #1e1e2e; color: #cdd6f4; }}
.message {{ margin: 1em 0; padding: 1em; border-radius: 8px; }}
.message-user {{ background: #313244; border-left: 4px solid #89b4fa; }}
.message-assistant {{ background: #313244; border-left: 4px solid #a6e3a1; }}
.message-tool {{ background: #313244; border-left: 4px solid #f9e2af; }}
.message-system {{ background: #313244; border-left: 4px solid #cba6f7; }}
.role {{ font-weight: bold; margin-bottom: 0.5em; color: #89dceb; }}
.content pre {{ white-space: pre-wrap; font-family: monospace; margin: 0; }}
</style>
</head>
<body>
<h1>Rova Conversation</h1>
<p>{len(state.history)} messages · exported {datetime.datetime.now().isoformat()}</p>
{"".join(messages_html)}
</body>
</html>"""
