"""Slash-command handling for interactive chat."""

from __future__ import annotations

import json
import re
import shlex
from pathlib import Path
from typing import Any

from rova.state import (
    DEFAULT_MODEL,
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
    token_usage,
)
from rova.skills import list_skills, read_skill

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
    "/exit",
]


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

System
  /health                        show router health
  /profiles                      list router profiles
  /exit                          quit
"""


def handle_slash_command(
    line: str,
    state: ChatState,
    client: Any | None = None,
    workspace_dir: Path | None = None,
) -> str:
    """Parse and execute a slash command. Returns output text to display."""
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
        result = client.compact(state)
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
        return _handle_rag_command(args, state, client)
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
            state.json_mode = args[0].lower() in {"on", "true", "1", "yes"}
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
        return json.dumps(client.health(), indent=2, sort_keys=True)
    if command == "/profiles":
        if client is None:
            return "client unavailable"
        payload = client.profiles()
        return "\n".join(sorted((payload.get("profiles") or {}).keys()))
    if command == "/skills":
        return _format_skills(list_skills(state.skills_dir))
    if command == "/skill":
        return _handle_skill_command(args, state)
    if command == "/workspace":
        if workspace_dir is None:
            return "workspace not configured"
        return _format_workspace(workspace_dir)
    return f"unknown command: {command}"


def _handle_rag_command(
    args: list[str], state: ChatState, client: Any | None
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
        return _format_ingest(client.ingest(paths=paths, urls=urls))
    if action == "search":
        if client is None:
            return "client unavailable"
        if len(args) < 2:
            return "usage: /rag search <query>"
        return _format_search(client.search(" ".join(args[1:]), top_k=5))
    return "usage: /rag on|off|ingest|search"


def _handle_skill_command(args: list[str], state: ChatState) -> str:
    if not args or args[0] == "list":
        return _format_skills(list_skills(state.skills_dir))
    action = args[0]
    if action == "use":
        if len(args) < 2:
            return "usage: /skill use <name>"
        name = args[1]
        if name not in list_skills(state.skills_dir):
            return f"unknown skill: {name}"
        if name not in state.active_skills:
            state.active_skills.append(name)
        return f"skill added: {name}"
    if action == "drop":
        if len(args) < 2:
            return "usage: /skill drop <name>"
        name = args[1]
        state.active_skills = [s for s in state.active_skills if s != name]
        return f"skill dropped: {name}"
    if action == "clear":
        state.active_skills.clear()
        return "skills cleared"
    if action == "show":
        if len(args) < 2:
            return "usage: /skill show <name>"
        text = read_skill(state.skills_dir, args[1])
        return text if text else f"unknown skill: {args[1]}"
    return "usage: /skill list|use|drop|clear|show"


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
