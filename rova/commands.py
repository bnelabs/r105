"""Slash-command handling for interactive chat."""

from __future__ import annotations

import datetime
import json
import shlex
from pathlib import Path
from typing import Any

import httpx

from rova.config import save_config
from rova.sessions import (
    delete_session,
    export_conversation,
    list_sessions,
    load_session,
    save_session,
)
from rova.skills import list_skills, read_skill
from rova.state import (
    DEFAULT_MODEL,
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
    token_usage,
)

SLASH_COMMANDS = [
    "/",
    "/help",
    "/state",
    "/tokens",
    "/model",
    "/history",
    "/clear",
    "/compact",
    "/profile",
    "/rag",
    "/quality",
    "/json",
    "/max",
    "/skills",
    "/skill",
    "/health",
    "/profiles",
    "/workspace",
    "/theme",
    "/autocompact",
    "/preview",
    "/session",
    "/export",
    "/exit",
]

VALID_THEMES = {"rova", "dracula", "solarized-dark", "high-contrast"}


def _parse_bool(value: str | None) -> bool | None:
    """Parse a string as a boolean; returns None if unrecognized."""
    if value is None:
        return None
    v = value.lower()
    if v in {"on", "true", "1", "yes"}:
        return True
    if v in {"off", "false", "0", "no"}:
        return False
    return None


def command_menu() -> str:
    return """Rova Commands

Chat
  /state                         show active settings
  /tokens                        show estimated context usage
  /model                         show active model and context capacity
  /history                       show compact transcript preview
  /clear                         clear chat history
  /compact                       summarize current history and continue
  /profile <name>                force profile, or omit name for auto
  /quality fast|balanced|best    set quality hint metadata
  /json [on|off]                 toggle JSON response mode
  /max <tokens>                  override max_tokens, or omit for auto

RAG
  /rag on|off                    toggle RAG metadata
  /rag ingest <path-or-url>...   ingest local files/directories or URLs
  /rag search <query>            search active RAG index

Skills
  /skills                        list local skills
  /skill use <name>              add a skill to the chat
  /skill drop <name>             remove one active skill
  /skill clear                   remove all active skills
  /skill show <name>             print a skill file

Workspace
  /workspace                     show workspace directory and files

Sessions
  /session save <name>           save conversation to a session file
  /session load <name>           load and restore a saved session
  /session list                  list saved sessions
  /session delete <name>         delete a saved session
  /export markdown|json|html     export conversation to a file

System
  /health                        show router health
  /profiles                      list router profiles
  /exit                          quit
"""


async def handle_slash_command(
    line: str,
    state: ChatState,
    client: Any | None = None,
    workspace_dir: Path | None = None,
    http_client: httpx.AsyncClient | None = None,
) -> str:
    """Parse and execute a slash command. Returns output text to display.

    Async so commands calling the router API do not block the TUI.
    """
    parts = shlex.split(line)
    if not parts:
        return ""
    command = parts[0]
    args = parts[1:]

    if command in {"/", "/help"}:
        return command_menu()
    if command == "/state":
        return _format_state(state)
    if command == "/tokens":
        return _status_line(state)
    if command == "/model":
        return f"model={DEFAULT_MODEL} ctx={state.context_tokens}"
    if command == "/history":
        return _format_history(state)
    if command == "/clear":
        state.history.clear()
        return "history cleared"
    if command == "/compact":
        if client is None:
            return "client unavailable"
        before = token_usage(state).used_tokens
        try:
            result = await client.async_compact(state, client=http_client)
        except httpx.HTTPError as exc:
            return f"compact failed: {exc}"
        after = token_usage(state).used_tokens
        return f"compacted {before}→{after} tokens\n{result.content}"
    if command == "/profile":
        if not args:
            state.profile = None
            return "profile=auto"
        profile = args[0]
        if profile not in VALID_PROFILES:
            return f"unknown profile: {profile}"
        state.profile = profile
        return f"profile={profile}"
    if command == "/rag":
        return await _handle_rag_command(args, state, client, http_client=http_client)
    if command == "/quality":
        if not args:
            state.quality = None
            return "quality=auto"
        quality = args[0]
        if quality not in VALID_QUALITIES:
            return f"unknown quality: {quality}"
        state.quality = quality
        return f"quality={quality}"
    if command == "/json":
        if not args:
            state.json_mode = not state.json_mode
        else:
            parsed = _parse_bool(args[0])
            state.json_mode = parsed if parsed is not None else state.json_mode
        return f"json={state.json_mode}"
    if command == "/max":
        if not args:
            state.max_tokens = None
            return "max_tokens=auto"
        try:
            state.max_tokens = int(args[0])
        except ValueError:
            return "usage: /max <tokens>"
        return f"max_tokens={state.max_tokens}"
    if command == "/health":
        if client is None:
            return "client unavailable"
        try:
            health_data = await client.async_health(client=http_client)
            return json.dumps(health_data, indent=2, sort_keys=True)
        except httpx.HTTPError as exc:
            return f"health check failed: {exc}"
    if command == "/profiles":
        if client is None:
            return "client unavailable"
        try:
            payload = await client.async_profiles(client=http_client)
            return "\n".join(sorted((payload.get("profiles") or {}).keys()))
        except httpx.HTTPError as exc:
            return f"profiles fetch failed: {exc}"
    if command == "/skills":
        return _format_skills(list_skills(state.skills_dir))
    if command == "/skill":
        return _handle_skill_command(args, state)
    if command == "/exit":
        return ""
    if command == "/workspace":
        if workspace_dir is None:
            return "workspace not configured"
        return _format_workspace(workspace_dir)
    if command == "/theme":
        if not args:
            return f"theme={state.theme} (valid: {', '.join(sorted(VALID_THEMES))})"
        theme = args[0]
        if theme not in VALID_THEMES:
            return f"unknown theme: {theme} (valid: {', '.join(sorted(VALID_THEMES))})"
        state.theme = theme
        save_config({"theme": theme})
        return f"theme={theme} (saved persistently)"
    if command == "/autocompact":
        if not args:
            state.auto_compact = not state.auto_compact
        else:
            parsed = _parse_bool(args[0])
            state.auto_compact = parsed if parsed is not None else state.auto_compact
        save_config({"auto_compact": state.auto_compact})
        return f"auto_compact={state.auto_compact} (saved persistently)"
    if command == "/preview":
        if workspace_dir is None:
            return "workspace not configured"
        if not args:
            return "usage: /preview <filename>"
        file_path = workspace_dir / args[0]
        if not file_path.exists():
            return f"file not found: {args[0]}"
        try:
            content = file_path.read_text(encoding="utf-8", errors="replace")
            return f"--- {args[0]} ---\n{content[:2000]}"
        except Exception as exc:
            return f"error reading {args[0]}: {exc}"
    if command == "/session":
        return _handle_session_command(args, state)
    if command == "/export":
        return _handle_export_command(args, state, workspace_dir)
    return f"unknown command: {command}"


async def _handle_rag_command(
    args: list[str], state: ChatState, client: Any | None,
    http_client: httpx.AsyncClient | None = None,
) -> str:
    if not args:
        state.rag = not bool(state.rag)
        return f"rag={state.rag}"
    action = args[0]
    if action in {"on", "true", "1", "yes"}:
        state.rag = True
        return "rag=True"
    if action in {"off", "false", "0", "no"}:
        state.rag = False
        return "rag=False"
    if action == "ingest":
        if client is None:
            return "client unavailable"
        if len(args) < 2:
            return "usage: /rag ingest <path-or-url> [more...]"
        paths, urls = _split_paths_and_urls(args[1:])
        try:
            return _format_ingest(
                await client.async_ingest(paths=paths, urls=urls, client=http_client)
            )
        except httpx.HTTPError as exc:
            return f"rag ingest failed: {exc}"
    if action == "search":
        if client is None:
            return "client unavailable"
        if len(args) < 2:
            return "usage: /rag search <query>"
        try:
            return _format_search(
                await client.async_search(" ".join(args[1:]), top_k=5, client=http_client)
            )
        except httpx.HTTPError as exc:
            return f"rag search failed: {exc}"
    if action == "list":
        if client is None:
            return "client unavailable"
        try:
            payload = await client.async_list_rag_documents(client=http_client)
            return _format_rag_list(payload)
        except Exception as exc:
            return f"rag list failed: {exc}"
    if action == "delete":
        if client is None:
            return "client unavailable"
        if len(args) < 2:
            return "usage: /rag delete <id>"
        try:
            payload = await client.async_delete_rag_document(args[1], client=http_client)
            return _format_rag_delete(payload)
        except Exception as exc:
            return f"rag delete failed: {exc}"
    if action == "update":
        if client is None:
            return "client unavailable"
        if len(args) < 2:
            return "usage: /rag update <path>"
        paths, _urls = _split_paths_and_urls(args[1:])
        try:
            return _format_ingest(
                await client.async_ingest(paths=paths, client=http_client)
            )
        except httpx.HTTPError as exc:
            return f"rag update failed: {exc}"
    return "usage: /rag on|off|ingest|search|list|delete|update"


def _handle_skill_command(args: list[str], state: ChatState) -> str:
    if not args or args[0] == "list":
        return _format_skills(list_skills(state.skills_dir))
    action = args[0]
    if action == "use":
        if len(args) < 2:
            return "usage: /skill use <name> [key=value ...]"
        name = args[1]
        if name not in list_skills(state.skills_dir):
            return f"unknown skill: {name}"
        if name not in state.active_skills:
            state.active_skills.append(name)
        # Parse key=value parameters from remaining args
        params: dict[str, str] = {}
        for arg in args[2:]:
            if "=" in arg:
                k, v = arg.split("=", 1)
                k = k.strip().rstrip()
                v = v.strip().lstrip()
                params[k] = v
        if params:
            state.skill_params[name] = params
        return f"skill added: {name}" + (f" (params: {params})" if params else "")
    if action == "drop":
        if len(args) < 2:
            return "usage: /skill drop <name>"
        name = args[1]
        state.active_skills = [s for s in state.active_skills if s != name]
        state.skill_params.pop(name, None)
        return f"skill dropped: {name}"
    if action == "clear":
        state.active_skills.clear()
        state.skill_params.clear()
        return "skills cleared"
    if action == "show":
        if len(args) < 2:
            return "usage: /skill show <name>"
        text = read_skill(state.skills_dir, args[1])
        return text if text else f"unknown skill: {args[1]}"
    return "usage: /skill list|use|drop|clear|show"


def _handle_session_command(args: list[str], state: ChatState) -> str:
    """Handle /session save|load|list|delete commands."""
    if not args:
        return "usage: /session save|load|list|delete"

    action = args[0]

    if action == "save":
        if len(args) < 2:
            return "usage: /session save <name>"
        name = args[1]
        try:
            path = save_session(state, name)
            return f"session saved: {name} ({len(state.history)} messages → {path})"
        except OSError as exc:
            return f"session save failed: {exc}"

    if action == "load":
        if len(args) < 2:
            return "usage: /session load <name>"
        name = args[1]
        try:
            count = load_session(state, name)
            return f"session loaded: {name} ({count} messages restored)"
        except FileNotFoundError:
            return f"session not found: {name}"
        except (json.JSONDecodeError, OSError) as exc:
            return f"session load failed: {exc}"

    if action == "list":
        sessions = list_sessions()
        if not sessions:
            return "no saved sessions"
        lines = [f"{len(sessions)} session(s):"]
        for s in sessions:
            name = s["name"]
            count = s["message_count"]
            when = s.get("saved_at", "?")[:16]
            preview = s.get("preview", "")
            lines.append(f"  {name}  ({count} msgs, {when})")
            if preview:
                lines.append(f"    {preview}")
        return "\n".join(lines)

    if action == "delete":
        if len(args) < 2:
            return "usage: /session delete <name>"
        name = args[1]
        if delete_session(name):
            return f"session deleted: {name}"
        return f"session not found: {name}"

    return "usage: /session save|load|list|delete"


def _handle_export_command(
    args: list[str], state: ChatState, workspace_dir: Path | None
) -> str:
    """Handle /export markdown|json|html command."""
    if workspace_dir is None:
        return "workspace not configured"

    fmt = args[0] if args else "markdown"
    if fmt not in {"markdown", "json", "html"}:
        return f"unknown format: {fmt} (valid: markdown, json, html)"

    if not state.history:
        return "nothing to export — conversation is empty"

    try:
        content = export_conversation(state, fmt)
    except Exception as exc:
        return f"export failed: {exc}"

    output_path = workspace_dir / f"conversation-{datetime.datetime.now():%Y%m%d-%H%M%S}.{fmt if fmt != 'markdown' else 'md'}"
    output_path.write_text(content, encoding="utf-8")

    return f"exported {len(state.history)} messages to {output_path}"



def _split_paths_and_urls(items: list[str]) -> tuple[list[str], list[str]]:
    paths: list[str] = []
    urls: list[str] = []
    for item in items:
        if item.startswith("http://") or item.startswith("https://"):
            urls.append(item)
        else:
            paths.append(item)
    return paths, urls


def _format_ingest(payload: dict[str, Any]) -> str:
    return (
        f"indexed files={payload.get('files_indexed', 0)} "
        f"chunks={payload.get('chunks_indexed', 0)}"
    )


def _format_search(payload: dict[str, Any]) -> str:
    results = payload.get("results") or []
    if not results:
        return "no results"
    lines: list[str] = []
    for result in results:
        tag = result.get("source_tag", "")
        score = result.get("score", 0)
        text = " ".join(str(result.get("text", "")).split())
        lines.append(f"{tag} score={score:.3f}\n{text[:700]}")
    return "\n\n".join(lines)


def _format_state(state: ChatState) -> str:
    usage = token_usage(state)
    return (
        f"profile={state.profile or 'auto'} "
        f"rag={state.rag if state.rag is not None else 'auto'} "
        f"quality={state.quality or 'auto'} "
        f"max_tokens={state.max_tokens or 'auto'} "
        f"json={state.json_mode} "
        f"skills={','.join(state.active_skills) if state.active_skills else 'none'} "
        f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%) "
        f"turns={len(state.history) // 2}"
    )


def _status_line(state: ChatState) -> str:
    usage = token_usage(state)
    return f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)"


def _format_history(state: ChatState) -> str:
    if not state.history:
        return "history empty"
    lines: list[str] = []
    for index, message in enumerate(
        state.history[-12:], start=max(1, len(state.history) - 11)
    ):
        role = message.get("role", "unknown")
        content = " ".join(str(message.get("content", "")).split())
        lines.append(f"{index}. {role}: {content[:220]}")
    return "\n".join(lines)


def _format_skills(names: list[str]) -> str:
    return "\n".join(names) if names else "no skills"


def _format_rag_list(payload: dict[str, Any]) -> str:
    documents = payload.get("documents") or []
    if not documents:
        return "no indexed documents"
    lines = [f"{len(documents)} document(s):"]
    for doc in documents:
        doc_id = doc.get("id", "?")
        source = doc.get("source", "?")
        chunks = doc.get("chunks", "?")
        lines.append(f"  [{doc_id}] {source} ({chunks} chunks)")
    return "\n".join(lines)


def _format_rag_delete(payload: dict[str, Any]) -> str:
    deleted = payload.get("deleted", 0)
    return f"deleted {deleted} document(s)"


def _format_workspace(workspace_dir: Path) -> str:
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
        lines.append(f"  {f.name}  ({_human_size(size)})")
    return "\n".join(lines)


def _human_size(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024:
            return f"{size}{unit}"
        size //= 1024
    return f"{size}TB"
