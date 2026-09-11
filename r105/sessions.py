"""Session persistence — save, load, list, and export conversations."""

from __future__ import annotations

import datetime
import json
import threading
from pathlib import Path
from typing import Any

from r105.config import CONFIG_DIR
from r105.state import ChatState, invalidate_backend_usage

SESSION_DIR = CONFIG_DIR / "sessions"

#: Current on-disk session format version. Bumped whenever the shape
#: written by :func:`save_session` changes; :func:`load_session` migrates
#: older files forward via :func:`migrate_session_data` and rejects files
#: stamped with a *newer* version than this code understands.
SESSION_FORMAT_VERSION = 1


class SessionManager:
    """Thread-safe session manager (replaces the global ``_AUTO_SAVE_ENABLED`` flag).

    The old module-level boolean was not thread-safe: concurrent TUI workers
    could race on enable/disable. This class encapsulates the flag behind a
    lock and groups all session operations so callers can share one instance.
    Module-level ``set_auto_save``/``get_auto_save``/``auto_save`` functions
    below delegate to a default shared instance for backwards compatibility.
    """

    def __init__(self, session_dir: Path | None = None, *, auto_save_enabled: bool = True) -> None:
        self._dir = Path(session_dir) if session_dir else SESSION_DIR
        self._lock = threading.RLock()
        self._auto_save_enabled = auto_save_enabled

    @property
    def session_dir(self) -> Path:
        return self._dir

    def set_auto_save(self, enabled: bool) -> None:
        with self._lock:
            self._auto_save_enabled = enabled
        # Keep deprecated module-global mirror in sync.
        try:
            import sys as _sys
            _sys.modules[__name__].__dict__["_AUTO_SAVE_ENABLED"] = enabled
        except Exception:
            pass

    def get_auto_save(self) -> bool:
        with self._lock:
            return self._auto_save_enabled

    def auto_save(self, state: ChatState) -> str | None:
        with self._lock:
            enabled = self._auto_save_enabled
        if not enabled:
            return None
        if not state.history:
            return None
        try:
            path = self.save_session(state, "__autosave__")
            return str(path)
        except OSError:
            return None

    def _ensure_dir(self) -> None:
        self._dir.mkdir(parents=True, exist_ok=True)

    def _session_path(self, name: str) -> Path:
        safe = name.replace("/", "_").replace("\\", "_").replace("..", "_")
        if not safe:
            safe = "unnamed"
        return self._dir / f"{safe}.json"

    def save_session(self, state: ChatState, name: str) -> Path:
        from r105.sessions import _serializable_state as _ser  # local to avoid cycle in docs
        self._ensure_dir()
        path = self._session_path(name)
        data: dict[str, Any] = {
            "version": SESSION_FORMAT_VERSION,
            "history": list(state.history),
            "state": _ser(state),
            "message_count": len(state.history),
            "saved_at": datetime.datetime.now().isoformat(),
        }
        # Atomic write: temp file + rename to avoid torn reads.
        tmp = path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
        tmp.replace(path)
        return path


_default_manager = SessionManager()


def get_session_manager() -> SessionManager:
    """Return the shared default :class:`SessionManager`."""
    return _default_manager


# Backwards-compat global (deprecated: use SessionManager). Kept so
# ``from r105.sessions import _AUTO_SAVE_ENABLED`` keeps working; the
# manager is the source of truth and this mirror is updated on every write.
_AUTO_SAVE_ENABLED: bool = True


def __getattr__(name: str) -> Any:
    # Only needed for type-checkers; _AUTO_SAVE_ENABLED exists above.
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def set_auto_save(enabled: bool) -> None:
    """Enable or disable auto-save on exit (thread-safe via manager)."""
    global _AUTO_SAVE_ENABLED
    _AUTO_SAVE_ENABLED = enabled
    _default_manager.set_auto_save(enabled)


def get_auto_save() -> bool:
    return _default_manager.get_auto_save()


def auto_save(state: ChatState) -> str | None:
    """Auto-save the current conversation if auto-save is enabled.

    Returns the path as a string if saved, None if skipped.
    """
    return _default_manager.auto_save(state)


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
        "quality": state.quality,
        "max_tokens": state.max_tokens,
        "json_mode": state.json_mode,
        "cache_prompt": state.cache_prompt,
        "model": state.model,
        "context_tokens": state.context_tokens,
        "active_skills": state.active_skills,
        "skill_params": state.skill_params,
    }


def _restore_state(state: ChatState, data: dict[str, Any]) -> None:
    """Restore session state into a ChatState object."""
    saved = data.get("state") or {}
    state.profile = saved.get("profile")
    state.quality = saved.get("quality")
    state.max_tokens = saved.get("max_tokens")
    state.json_mode = saved.get("json_mode", False)
    state.cache_prompt = saved.get("cache_prompt", False)
    if isinstance(saved.get("model"), str) and saved["model"].strip():
        state.model = saved["model"]
    if isinstance(saved.get("context_tokens"), int) and saved["context_tokens"] > 0:
        state.context_tokens = saved["context_tokens"]
    state.active_skills = saved.get("active_skills") or []
    state.skill_params = saved.get("skill_params") or {}
    invalidate_backend_usage(state)


def save_session(state: ChatState, name: str) -> Path:
    """Save the current conversation to a session file.

    Returns the path to the saved file.
    """
    _ensure_dir()
    path = _session_path(name)

    data: dict[str, Any] = {
        "version": SESSION_FORMAT_VERSION,
        "history": state.history,
        "state": _serializable_state(state),
        "message_count": len(state.history),
        "saved_at": datetime.datetime.now().isoformat(),
    }

    path.write_text(json.dumps(data, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")
    return path


def migrate_session_data(data: dict[str, Any]) -> dict[str, Any]:
    """Migrate a parsed session file dict forward to the current format.

    Files written before versioning carry no ``"version"`` key and are
    treated as version 0. Each step is applied in order so future format
    changes only need a new branch here.

    Raises ValueError if the file was written by a *newer* r105 than this
    code understands (downgrade protection — loading it could corrupt
    state the old code doesn't know about).
    """
    if not isinstance(data, dict):
        raise ValueError("session file must contain a JSON object")
    version = data.get("version", 0)
    if type(version) is not int or version < 0:  # bool is not a valid version stamp
        raise ValueError(f"session has an invalid version stamp: {version!r}")
    if version > SESSION_FORMAT_VERSION:
        raise ValueError(
            f"session was saved by a newer r105 (format v{version}, "
            f"this build reads up to v{SESSION_FORMAT_VERSION}) — "
            "upgrade r105 to open it"
        )
    migrated = dict(data)
    if version == 0:
        # v0 → v1: identical shape (history/state/message_count/saved_at);
        # stamp the version so the next load skips migration.
        migrated["version"] = SESSION_FORMAT_VERSION
    return migrated


def _read_session_data(path: Path) -> dict[str, Any]:
    """Read and migrate one session file.

    Keeping this boundary shared by load/list/search/diff means a malformed
    or future-format file is handled consistently across all session views.
    """
    raw = json.loads(path.read_text(encoding="utf-8"))
    return migrate_session_data(raw)


def _history_from_session(data: dict[str, Any]) -> list[dict[str, Any]]:
    history = data.get("history")
    if not isinstance(history, list):
        return []
    return [message for message in history if isinstance(message, dict)]


def load_session(state: ChatState, name: str) -> int:
    """Load a saved session, replacing the current conversation history.

    Returns the number of messages loaded.

    Raises FileNotFoundError if the session doesn't exist.
    Raises json.JSONDecodeError if the file is corrupted.
    Raises ValueError if the file uses a newer session format.
    """
    path = _session_path(name)
    if not path.is_file():
        raise FileNotFoundError(f"session not found: {name}")

    data = _read_session_data(path)
    history = _history_from_session(data)

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
            data = _read_session_data(path)
        except (json.JSONDecodeError, OSError, ValueError):
            continue

        history = _history_from_session(data)
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


def search_sessions(
    query: str, *, limit: int = 20, snippets_per_session: int = 3
) -> list[dict[str, Any]]:
    """Full-text search across saved sessions (case-insensitive substring).

    Scans every message in every session file and returns at most *limit*
    sessions that contain *query*, most recently saved first. Each entry is
    a dict with: name, saved_at, message_count, and matches — a list of up
    to *snippets_per_session* ``{"role": ..., "snippet": ...}`` dicts with
    ~60 characters of context around the hit. Corrupt files are skipped.
    """
    normalized_query = query.strip()
    needle = normalized_query.lower()
    if not needle or limit <= 0 or snippets_per_session <= 0:
        return []
    _ensure_dir()
    hits: list[dict[str, Any]] = []

    for path in sorted(SESSION_DIR.glob("*.json"), key=lambda p: p.stat().st_mtime, reverse=True):
        if len(hits) >= limit:
            break
        try:
            data = _read_session_data(path)
        except (json.JSONDecodeError, OSError, ValueError):
            continue
        history = _history_from_session(data)
        matches: list[dict[str, str]] = []
        for msg in history:
            if len(matches) >= snippets_per_session:
                break
            content = str(msg.get("content", ""))
            idx = content.lower().find(needle)
            if idx == -1:
                continue
            start = max(0, idx - 60)
            end = min(len(content), idx + len(normalized_query) + 60)
            snippet = " ".join(content[start:end].split())
            if start > 0:
                snippet = "…" + snippet
            if end < len(content):
                snippet += "…"
            matches.append({"role": str(msg.get("role", "?")), "snippet": snippet})
        if matches:
            hits.append({
                "name": path.stem,
                "saved_at": data.get("saved_at", "unknown"),
                "message_count": len(history),
                "matches": matches,
            })

    return hits


def delete_session(name: str) -> bool:
    """Delete a saved session file. Returns True if deleted, False if not found."""
    path = _session_path(name)
    if not path.is_file():
        return False
    path.unlink()
    return True


def diff_session(state: ChatState, name: str) -> str:
    """Summarize what changed between the live *state* and saved session *name*.

    Reports the message-count delta, previews of unsaved messages, and any
    state-field differences (profile, quality, max_tokens, json_mode, skills).

    Raises FileNotFoundError if the session doesn't exist.
    May raise json.JSONDecodeError or OSError if the file is unreadable.
    """
    path = _session_path(name)
    if not path.is_file():
        raise FileNotFoundError(f"session not found: {name}")
    data = _read_session_data(path)
    saved_history = _history_from_session(data)
    saved_state: dict[str, Any] = data.get("state") or {}

    lines = [f"diff vs saved session '{name}':"]
    saved_count = len(saved_history)
    current_count = len(state.history)
    delta = current_count - saved_count
    if delta == 0:
        lines.append("  messages: no change")
    elif delta > 0:
        lines.append(
            f"  messages: +{delta} unsaved "
            f"({saved_count} saved → {current_count} current)"
        )
        for msg in state.history[saved_count:saved_count + 3]:
            role = str(msg.get("role", "?"))
            content = " ".join(str(msg.get("content", "")).split())
            lines.append(f"    + [{role}] {content[:120]}")
        if delta > 3:
            lines.append(f"    … and {delta - 3} more unsaved message(s)")
    else:
        lines.append(
            f"  messages: {delta} (current is shorter — "
            f"{saved_count} saved vs {current_count} current)"
        )

    current_state = _serializable_state(state)
    changed: list[str] = []
    for key in sorted(set(saved_state) | set(current_state)):
        old = saved_state.get(key)
        new = current_state.get(key)
        if old != new:
            changed.append(f"{key}: {old!r} → {new!r}")
    if changed:
        lines.append("  settings changed since save:")
        lines.extend(f"    ~ {item}" for item in changed)
    else:
        lines.append("  settings: no change")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------


def _ensure_export_deps() -> None:
    """Ensure optional export dependencies are available.

    Raises RuntimeError with a helpful install hint if missing.
    """
    try:
        import docx  # noqa: F401
        import fpdf2  # noqa: F401
        import python_pptx  # noqa: F401
        from PIL import Image  # noqa: F401
    except ImportError as exc:
        raise RuntimeError(
            "Export dependencies are missing. Install with `pip install 'r105[export]'`."
        ) from exc


def export_conversation(state: ChatState, fmt: str = "markdown") -> str:
    """Render conversation history as a string in the requested format.

    Supported formats: text, markdown, json, html.
    """
    # Guard for future binary export formats that require optional deps
    if fmt.lower() in {"pdf", "docx", "pptx"}:
        _ensure_export_deps()

    if fmt == "json":
        return json.dumps(state.history, indent=2, sort_keys=True, ensure_ascii=False)

    if fmt == "html":
        return _export_html(state)

    if fmt == "text":
        return _export_text(state)

    # Default: markdown
    return _export_markdown(state)


def _export_text(state: ChatState) -> str:
    """Render a dependency-free plain-text transcript."""
    lines = [
        "r105 Conversation",
        f"Exported: {datetime.datetime.now().isoformat()}",
        f"Messages: {len(state.history)}",
        "",
    ]
    for index, msg in enumerate(state.history, 1):
        role = str(msg.get("role", "unknown")).upper()
        name = f" ({msg.get('name')})" if msg.get("name") else ""
        lines.extend([f"[{index}] {role}{name}", str(msg.get("content", "")), ""])
    return "\n".join(lines)


def _export_markdown(state: ChatState) -> str:
    """Render conversation as Markdown."""
    lines: list[str] = [
        "# r105 Conversation",
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
<title>r105 Conversation</title>
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
<h1>r105 Conversation</h1>
<p>{len(state.history)} messages · exported {datetime.datetime.now().isoformat()}</p>
{"".join(messages_html)}
</body>
</html>"""
