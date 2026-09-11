"""Slash-command handling for interactive chat.

Commands are dispatched via a dictionary mapping command name to handler
function (``COMMAND_DISPATCH``).  Each handler receives a single
:class:`CommandContext` with the parsed args plus the shared context objects,
and returns a result string.
"""

from __future__ import annotations

import datetime
import difflib
import json
import os
from collections.abc import Awaitable, Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit

import httpx

from r105.command_parser import CommandParser, CommandRegistry
from r105.commands_format import (
    format_history as _fmt_history,
)
from r105.commands_format import (
    format_skills as _fmt_skills,
)
from r105.commands_format import (
    format_state as _fmt_state,
)
from r105.commands_format import (
    format_workspace as _fmt_workspace,
)
from r105.commands_format import (
    human_size as _fmt_human_size,
)
from r105.commands_format import (
    status_line as _fmt_status,
)
from r105.config import apply_config_to_state, ensure_config, save_config
from r105.model_catalog import resolve_context_tokens
from r105.plugins import get_registry
from r105.sessions import (
    delete_session,
    diff_session,
    export_conversation,
    list_sessions,
    load_session,
    save_session,
    search_sessions,
)
from r105.skills import list_skills, read_skill
from r105.state import (
    VALID_PERMISSION_POSTURES,
    VALID_PROFILES,
    VALID_QUALITIES,
    VALID_REASONING_EFFORTS,
    ChatState,
    invalidate_backend_usage,
    token_usage,
)
from r105.tools import _cache_clear, approve_execute_python

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
    "/quality",
    "/json",
    "/max",
    "/cache-prompt",
    "/config",
    "/skills",
    "/skill",
    "/health",
    "/profiles",
    "/workspace",
    "/theme",
    "/autocompact",
    "/reasoning",
    "/permissions",
    "/approve",
    "/connect",
    "/provider",
    "/preview",
    "/session",
    "/export",
    "/plugin",
    "/mcp",
    "/copy",
    "/exit",
]

VALID_THEMES = {"r105", "dracula", "solarized-dark", "high-contrast"}

# Provider presets intentionally use environment variables for credentials.
# Keys are never accepted as command arguments or written to config.json.
PROVIDER_PRESETS: dict[str, tuple[str, str, str | None]] = {
    "router": ("router", "http://127.0.0.1:8010", None),
    "llamacpp": ("direct", "http://127.0.0.1:8080/v1", None),
    "ollama": ("direct", "http://127.0.0.1:11434/v1", None),
    "lmstudio": ("direct", "http://127.0.0.1:1234/v1", None),
    "vllm": ("direct", "http://127.0.0.1:8000/v1", "OPENAI_API_KEY"),
    "openai": ("direct", "https://api.openai.com/v1", "OPENAI_API_KEY"),
    "groq": ("direct", "https://api.groq.com/openai/v1", "GROQ_API_KEY"),
    "openrouter": ("direct", "https://openrouter.ai/api/v1", "OPENROUTER_API_KEY"),
    "deepseek": ("direct", "https://api.deepseek.com/v1", "DEEPSEEK_API_KEY"),
    "together": ("direct", "https://api.together.xyz/v1", "TOGETHER_API_KEY"),
}

PROVIDER_ALIASES = {
    "local": "ollama",
    "llama-router": "router",
    "lm-studio": "lmstudio",
    "llama.cpp": "llamacpp",
    "llama-cpp": "llamacpp",
}


def _connect_usage() -> str:
    providers = " | ".join(PROVIDER_PRESETS)
    return (
        "usage: /connect <provider> [base-url]\n"
        f"providers: {providers}\n"
        "custom OpenAI-compatible API: /connect url <https://host/v1>\n"
        "credentials come from the provider's environment variable; use /model after connecting"
    )


def _valid_provider_url(value: str) -> bool:
    """Accept HTTP(S) API URLs without allowing credentials in the URL."""
    parsed = urlsplit(value)
    return (
        parsed.scheme in {"http", "https"}
        and bool(parsed.hostname)
        and parsed.username is None
        and parsed.password is None
        and not any(char.isspace() for char in value)
    )

# Signature for command handler functions.
# All handlers are async functions taking a CommandContext, returning str.
@dataclass
class CommandContext:
    """Everything a slash-command handler needs, in one object."""

    args: list[str]
    state: ChatState
    client: Any | None = None
    workspace_dir: Path | None = None
    http_client: httpx.AsyncClient | None = None


CommandHandler = Callable[[CommandContext], Awaitable[str]]


def get_mcp_manager() -> Any:
    """Load the MCP manager lazily while preserving the old module API."""
    from r105.mcp_client import get_mcp_manager as _get_mcp_manager

    return _get_mcp_manager()


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


def _apply_bool_toggle(args: list[str], current: bool) -> bool:
    """Shared toggle semantics: no args flips, recognized bool sets.

    Unrecognized values leave the current setting unchanged (callers report
    the effective value so the user sees what stuck).
    """
    if not args:
        return not current
    parsed = _parse_bool(args[0])
    return parsed if parsed is not None else current


def _parse_choice(
    args: list[str], *, field: str, valid: set[str], lower: bool = False
) -> tuple[str | None, str | None]:
    """Validate ``args[0]`` against *valid*.

    Returns ``(value, None)`` on success, ``(None, error_message)`` on an
    unrecognized value, and ``(None, None)`` when no args were given (the
    caller decides between reset-to-auto and show-current).
    """
    if not args:
        return None, None
    value = args[0].lower() if lower else args[0]
    if value not in valid:
        suggestion = difflib.get_close_matches(value, sorted(valid), n=1, cutoff=0.5)
        hint = f" — did you mean {suggestion[0]}?" if suggestion else ""
        return None, (
            f"unknown {field}: {args[0]} "
            f"(valid: {', '.join(sorted(valid))}){hint}"
        )
    return value, None


def _global_context_override() -> int | None:
    """Return the global ``context_tokens`` override from config.json, if any."""
    try:
        value = ensure_config().get("context_tokens")
        if value is None:
            return None
        ivalue = int(value)
        return ivalue if ivalue > 0 else None
    except (TypeError, ValueError):
        return None


def _suggest_command(typed: str) -> str | None:
    """Return the closest matching command for *typed*, or None."""
    return COMMAND_REGISTRY.suggest(typed)


def command_menu() -> str:
    return """r105 Commands

Chat
  /state                         show active settings
  /tokens                        show context usage, source, and confidence
  /model                         show active model and context capacity
  /history                       show compact transcript preview
  /clear                         clear chat history
  /compact                       summarize current history and continue
  /profile <name>                force profile, or omit name for auto
  /quality fast|balanced|best    set quality hint metadata
  /json [on|off]                 toggle JSON response mode
  /max <tokens>                  override max_tokens, or omit for auto
  /cache-prompt [on|off]         enable llama.cpp prompt-prefix caching
  /config reload                 reload config.json into the current session
  /autocompact [on|off]          toggle auto-compaction at 80% context
  /reasoning auto|off|low|med..  set reasoning effort (model-provided)
  /permissions <posture>         set permission posture (full-access|restricted|sandboxed|off)
  /approve execute_python        one-time approval for code execution
  /connect <provider>            connect to a local or cloud OpenAI-compatible provider
  /provider <provider>           alias for /connect

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
  /session search <query>        full-text search across saved sessions
  /session delete <name>         delete a saved session
  /export text|markdown|json|html export conversation to a file
  /plugin list                   list loaded custom tool plugins
  /plugin reload                 reload plugins from disk
  /mcp list                      list connected MCP servers
  /mcp tools <server>            list tools from an MCP server
  /mcp reconnect <server>        reconnect an MCP server and rediscover tools

System
  /health                        show selected backend health
  /profiles                      list router profiles
  /exit                          quit
"""


# ---------------------------------------------------------------------------
# Per-command handler functions (extracted from the former if-elif chain)
# ---------------------------------------------------------------------------


async def _cmd_help(ctx: CommandContext) -> str:
    return command_menu()


async def _cmd_state(ctx: CommandContext) -> str:
    return _format_state(ctx.state)


async def _cmd_tokens(ctx: CommandContext) -> str:
    return _status_line(ctx.state)


async def _cmd_model(ctx: CommandContext) -> str:
    # /model <name> — switch models
    if ctx.args:
        ctx.state.model = ctx.args[0]
        invalidate_backend_usage(ctx.state)
        save_config({"model": ctx.state.model})
        # Re-resolve the context-window capacity for the new model
        backend_ctx: int | None = None
        if ctx.client is not None:
            try:
                backend_ctx = await ctx.client.async_probe_context(ctx.state.model, client=ctx.http_client)
            except Exception:
                backend_ctx = None
        ctx.state.context_tokens = resolve_context_tokens(
            ctx.state.model,
            config_contexts=ctx.state.model_contexts,
            global_override=_global_context_override(),
            backend_context=backend_ctx,
        )
        return f"model={ctx.state.model} ctx={ctx.state.context_tokens} (saved persistently)"
    # /model — show current model or list available
    if ctx.client is not None:
        try:
            payload = await ctx.client.async_list_models(client=ctx.http_client)
            models_data = payload.get("data") or payload.get("models") or []
            model_ids = [m.get("id", "") for m in models_data if m.get("id")]
            if model_ids:
                current = f"current: {ctx.state.model}\n"
                current += "available:\n  " + "\n  ".join(model_ids)
                return current
        except httpx.HTTPError:
            pass
    return f"model={ctx.state.model} ctx={ctx.state.context_tokens}"


async def _cmd_history(ctx: CommandContext) -> str:
    return _format_history(ctx.state)


async def _cmd_clear(ctx: CommandContext) -> str:
    ctx.state.history.clear()
    invalidate_backend_usage(ctx.state)
    return "history cleared"


async def _cmd_compact(ctx: CommandContext) -> str:
    if ctx.client is None:
        return "client unavailable"
    before = token_usage(ctx.state).used_tokens
    try:
        result = await ctx.client.async_compact(ctx.state, client=ctx.http_client)
    except httpx.HTTPError as exc:
        return f"compact failed: {exc}"
    invalidate_backend_usage(ctx.state)
    after = token_usage(ctx.state).used_tokens
    return f"compacted {before}→{after} tokens\n{result.content}"


async def _cmd_profile(ctx: CommandContext) -> str:
    if not ctx.args:
        ctx.state.profile = None
        return "profile=auto"
    profile, error = _parse_choice(ctx.args, field="profile", valid=VALID_PROFILES)
    if error is not None:
        return error
    if profile is None:  # unreachable: _parse_choice only returns None with no args
        return "internal error: profile choice parsing failed"
    ctx.state.profile = profile
    return f"profile={profile}"


async def _cmd_quality(ctx: CommandContext) -> str:
    if not ctx.args:
        ctx.state.quality = None
        return "quality=auto"
    quality, error = _parse_choice(ctx.args, field="quality", valid=VALID_QUALITIES)
    if error is not None:
        return error
    if quality is None:  # unreachable: _parse_choice only returns None with no args
        return "internal error: quality choice parsing failed"
    ctx.state.quality = quality
    return f"quality={quality}"


async def _cmd_json(ctx: CommandContext) -> str:
    ctx.state.json_mode = _apply_bool_toggle(ctx.args, ctx.state.json_mode)
    return f"json={ctx.state.json_mode}"


async def _cmd_max(ctx: CommandContext) -> str:
    if not ctx.args:
        ctx.state.max_tokens = None
        return "max_tokens=auto"
    try:
        ctx.state.max_tokens = int(ctx.args[0])
    except ValueError:
        return "usage: /max <tokens>"
    return f"max_tokens={ctx.state.max_tokens}"


async def _cmd_cache_prompt(ctx: CommandContext) -> str:
    ctx.state.cache_prompt = _apply_bool_toggle(ctx.args, ctx.state.cache_prompt)
    save_config({"cache_prompt": ctx.state.cache_prompt})
    return (
        f"cache_prompt={ctx.state.cache_prompt} (saved persistently; "
        "use with a llama.cpp-compatible backend)"
    )


async def _cmd_config(ctx: CommandContext) -> str:
    """Reload or inspect the effective config file."""
    action = ctx.args[0] if ctx.args else "reload"
    if action == "show":
        try:
            return json.dumps(ensure_config(strict=True), indent=2, sort_keys=True)
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            return f"config read failed: {exc}"
    if action != "reload":
        return "usage: /config reload|show"

    try:
        config = ensure_config(strict=True)
        changed = apply_config_to_state(ctx.state, config)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        return f"config reload failed: {exc}"

    if "model" in changed:
        invalidate_backend_usage(ctx.state)
    backend_ctx: int | None = None
    if ctx.client is not None:
        try:
            backend_ctx = await ctx.client.async_probe_context(
                ctx.state.model, client=ctx.http_client
            )
        except Exception:
            backend_ctx = None
    resolved_context = resolve_context_tokens(
        ctx.state.model,
        config_contexts=ctx.state.model_contexts,
        global_override=_global_context_override(),
        backend_context=backend_ctx,
    )
    if resolved_context != ctx.state.context_tokens:
        ctx.state.context_tokens = resolved_context
        changed.add("context_tokens")

    if not changed:
        return "config reloaded: no session settings changed"
    fields = ", ".join(sorted(changed))
    return f"config reloaded: {fields}"


async def _cmd_health(ctx: CommandContext) -> str:
    if ctx.client is None:
        return "client unavailable"
    try:
        health_data = await ctx.client.async_health(client=ctx.http_client)
        return json.dumps(health_data, indent=2, sort_keys=True)
    except httpx.HTTPError as exc:
        return f"health check failed: {exc}"


async def _cmd_profiles(ctx: CommandContext) -> str:
    if ctx.client is None:
        return "client unavailable"
    if not hasattr(ctx.client, "async_profiles"):
        return "profiles are only available with llama-router backend (--backend router)"
    try:
        payload = await ctx.client.async_profiles(client=ctx.http_client)
        return "\n".join(sorted((payload.get("profiles") or {}).keys()))
    except httpx.HTTPError as exc:
        return f"profiles fetch failed: {exc}"


async def _cmd_skills(ctx: CommandContext) -> str:
    return _format_skills(list_skills(ctx.state.skills_dir))


async def _cmd_skill(ctx: CommandContext) -> str:
    return _handle_skill_command(ctx.args, ctx.state)


async def _cmd_workspace(ctx: CommandContext) -> str:
    if ctx.workspace_dir is None:
        return "workspace not configured"
    return _format_workspace(ctx.workspace_dir)


async def _cmd_theme(ctx: CommandContext) -> str:
    if not ctx.args:
        return f"theme={ctx.state.theme} (valid: {', '.join(sorted(VALID_THEMES))})"
    theme, error = _parse_choice(ctx.args, field="theme", valid=VALID_THEMES)
    if error is not None:
        return error
    if theme is None:  # unreachable: _parse_choice only returns None with no args
        return "internal error: theme choice parsing failed"
    ctx.state.theme = theme
    save_config({"theme": theme})
    return f"theme={theme} (saved persistently)"


async def _cmd_autocompact(ctx: CommandContext) -> str:
    ctx.state.auto_compact = _apply_bool_toggle(ctx.args, ctx.state.auto_compact)
    save_config({"auto_compact": ctx.state.auto_compact})
    return f"auto_compact={ctx.state.auto_compact} (saved persistently)"


async def _cmd_reasoning(ctx: CommandContext) -> str:
    if not ctx.args:
        return (
            f"reasoning_effort={ctx.state.reasoning_effort} "
            f"(valid: {', '.join(sorted(VALID_REASONING_EFFORTS))})"
        )
    effort, error = _parse_choice(
        ctx.args, field="reasoning effort", valid=VALID_REASONING_EFFORTS, lower=True
    )
    if error is not None:
        return error
    if effort is None:  # unreachable: _parse_choice only returns None with no args
        return "internal error: reasoning effort choice parsing failed"
    ctx.state.reasoning_effort = effort
    save_config({"reasoning_effort": effort})
    return f"reasoning_effort={effort} (saved persistently)"


async def _cmd_permissions(ctx: CommandContext) -> str:
    if not ctx.args:
        return (
            f"permission_posture={ctx.state.permission_posture} "
            f"(valid: {', '.join(sorted(VALID_PERMISSION_POSTURES))})"
        )
    posture, error = _parse_choice(
        ctx.args, field="permission posture", valid=VALID_PERMISSION_POSTURES, lower=True
    )
    if error is not None:
        return error
    if posture is None:  # unreachable: _parse_choice only returns None with no args
        return "internal error: permission posture choice parsing failed"
    ctx.state.permission_posture = posture
    save_config({"permission_posture": posture})
    return f"permission_posture={posture} (saved persistently)"


async def _cmd_approve(ctx: CommandContext) -> str:
    if not ctx.args or ctx.args[0] not in ("execute_python", "python"):
        return "usage: /approve execute_python — one-time approval for code execution"
    approve_execute_python()
    return "execute_python approved for this session"


async def _cmd_connect(ctx: CommandContext) -> str:
    """Select and persist a local or cloud OpenAI-compatible provider."""
    if not ctx.args or ctx.args[0].lower() in {"help", "list"}:
        return _connect_usage()

    provider = ctx.args[0].lower()
    if provider in {"status", "show"}:
        config = ensure_config()
        backend = config.get("backend")
        if backend is None and ctx.client is not None:
            backend = type(getattr(ctx.client, "backend", ctx.client)).__name__.removesuffix("Client").lower()
        url = config.get("url")
        if url is None and ctx.client is not None:
            url = getattr(ctx.client, "base_url", "")
        return f"backend={backend or 'auto'}\nurl={url or 'auto'}"

    provider = PROVIDER_ALIASES.get(provider, provider)
    credential_env: str | None = None
    if provider in {"url", "custom"}:
        if len(ctx.args) != 2:
            return "usage: /connect url <https://host/v1>"
        backend = "direct"
        base_url = ctx.args[1]
        credential_env = "OPENAI_API_KEY"
        display_name = "custom"
    else:
        preset = PROVIDER_PRESETS.get(provider)
        if preset is None:
            return f"unknown provider: {ctx.args[0]}\n\n{_connect_usage()}"
        if len(ctx.args) > 2:
            return "usage: /connect <provider> [base-url]"
        backend, default_url, credential_env = preset
        base_url = ctx.args[1] if len(ctx.args) == 2 else default_url
        display_name = provider

    if not _valid_provider_url(base_url):
        return f"invalid provider URL: {base_url!r} (use http:// or https:// without credentials)"

    try:
        save_config({"backend": backend, "url": base_url.rstrip("/")})
    except (OSError, ValueError) as exc:
        return f"provider configuration failed: {exc}"

    if credential_env is None:
        credential_status = "no API key required"
    elif os.environ.get(credential_env):
        credential_status = f"{credential_env}=set"
    else:
        credential_status = f"set {credential_env} before sending requests"

    return (
        f"provider={display_name} backend={backend} url={base_url.rstrip('/')}\n"
        f"{credential_status}\n"
        "connection switched live; use /health to verify and /model to choose a model"
    )


async def _cmd_preview(ctx: CommandContext) -> str:
    if ctx.workspace_dir is None:
        return "workspace not configured"
    if not ctx.args:
        return "usage: /preview <filename>"
    file_path = ctx.workspace_dir / ctx.args[0]
    if not file_path.exists():
        return f"file not found: {ctx.args[0]}"
    try:
        content = file_path.read_text(encoding="utf-8", errors="replace")
        return f"--- {ctx.args[0]} ---\n{content[:2000]}"
    except Exception as exc:
        return f"error reading {ctx.args[0]}: {exc}"


async def _cmd_session(ctx: CommandContext) -> str:
    return _handle_session_command(ctx.args, ctx.state)


async def _cmd_export(ctx: CommandContext) -> str:
    return _handle_export_command(ctx.args, ctx.state, ctx.workspace_dir)


async def _cmd_plugin(ctx: CommandContext) -> str:
    return _handle_plugin_command(ctx.args)


async def _cmd_mcp(ctx: CommandContext) -> str:
    return _handle_mcp_command(ctx.args)


async def _cmd_exit(ctx: CommandContext) -> str:
    return ""


async def _cmd_copy(ctx: CommandContext) -> str:
    """Copy the last assistant message to the system clipboard."""
    # Find the last assistant message in history
    last_content = ""
    for msg in reversed(ctx.state.history):
        if msg.get("role") == "assistant":
            last_content = msg.get("content", "")
            break

    if not last_content:
        return "nothing to copy — no assistant message found"

    success = copy_to_clipboard(last_content)
    if success:
        return f"copied {len(last_content)} chars to clipboard"
    return "clipboard unavailable (install xclip or wl-copy on Linux)"


def copy_to_clipboard(text: str) -> bool:
    """Copy *text* to the system clipboard. Returns True on success."""
    import shutil as _shutil
    import subprocess as _sp

    # Try platform-specific clipboard tools
    for tool_cmd in [
        ["xclip", "-selection", "clipboard"],
        ["wl-copy"],
        ["pbcopy"],
        ["clip"],
    ]:
        tool = _shutil.which(tool_cmd[0])
        if tool:
            try:
                _sp.run([tool, *tool_cmd[1:]], input=text, text=True, timeout=5, check=True)
                return True
            except Exception:
                pass
    return False


# -- Command dispatch table -----------------------------------------------

COMMAND_DISPATCH: dict[str, CommandHandler] = {
    "/": _cmd_help,
    "/help": _cmd_help,
    "/state": _cmd_state,
    "/tokens": _cmd_tokens,
    "/model": _cmd_model,
    "/history": _cmd_history,
    "/clear": _cmd_clear,
    "/compact": _cmd_compact,
    "/profile": _cmd_profile,
    "/quality": _cmd_quality,
    "/json": _cmd_json,
    "/max": _cmd_max,
    "/cache-prompt": _cmd_cache_prompt,
    "/config": _cmd_config,
    "/health": _cmd_health,
    "/profiles": _cmd_profiles,
    "/skills": _cmd_skills,
    "/skill": _cmd_skill,
    "/workspace": _cmd_workspace,
    "/theme": _cmd_theme,
    "/autocompact": _cmd_autocompact,
    "/reasoning": _cmd_reasoning,
    "/permissions": _cmd_permissions,
    "/approve": _cmd_approve,
    "/connect": _cmd_connect,
    "/provider": _cmd_connect,
    "/preview": _cmd_preview,
    "/session": _cmd_session,
    "/export": _cmd_export,
    "/plugin": _cmd_plugin,
    "/mcp": _cmd_mcp,
    "/copy": _cmd_copy,
    "/exit": _cmd_exit,
}

# The mapping remains public for existing integrations; the registry is the
# parser-facing source of truth for new callers.
COMMAND_REGISTRY = CommandRegistry(COMMAND_DISPATCH)
COMMAND_PARSER = CommandParser()


# -- Main entry point -----------------------------------------------------


async def handle_slash_command(
    line: str,
    state: ChatState,
    client: Any | None = None,
    workspace_dir: Path | None = None,
    http_client: httpx.AsyncClient | None = None,
) -> str:
    """Parse and execute a slash command. Returns output text to display.

    Dispatches via ``COMMAND_DISPATCH`` — each handler is an async function
    so commands that call the router API do not block the TUI.
    """
    parsed = COMMAND_PARSER.parse(line)
    if parsed is None:
        return ""
    command = parsed.name
    args = parsed.args

    handler = COMMAND_REGISTRY.get(command)
    if handler is None:
        suggestion = _suggest_command(command)
        if suggestion and suggestion != command:
            return f"unknown command: {command} — did you mean {suggestion}?"
        return f"unknown command: {command}"

    return await handler(CommandContext(args, state, client, workspace_dir, http_client))


def _handle_skill_command(args: list[str], state: ChatState) -> str:
    if not args or args[0] == "list":
        return _format_skills(list_skills(state.skills_dir))
    action = args[0]
    if action == "use":
        if len(args) < 2:
            return "usage: /skill use <name> [key=value ...]"
        name = args[1]
        if name not in list_skills(state.skills_dir):
            suggestion = difflib.get_close_matches(name, list_skills(state.skills_dir), n=1, cutoff=0.5)
            hint = f" — did you mean {suggestion[0]}?" if suggestion else ""
            return f"unknown skill: {name}{hint}"
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
        name = args[1]
        text = read_skill(state.skills_dir, name)
        if text:
            return text
        suggestion = difflib.get_close_matches(name, list_skills(state.skills_dir), n=1, cutoff=0.5)
        hint = f" — did you mean {suggestion[0]}?" if suggestion else ""
        return f"unknown skill: {name}{hint}"
    return "usage: /skill list|use|drop|clear|show"


def _handle_session_command(args: list[str], state: ChatState) -> str:
    """Handle /session save|load|list|search|delete commands."""
    if not args:
        return "usage: /session save|load|list|search|delete"

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
            summary = diff_session(state, name)
        except FileNotFoundError:
            return f"session not found: {name}"
        except (json.JSONDecodeError, OSError) as exc:
            return f"session load failed: {exc}"
        try:
            count = load_session(state, name)
            _cache_clear()  # stale tool results must not leak into the new context
            return f"session loaded: {name} ({count} messages restored)\n{summary}"
        except (json.JSONDecodeError, OSError) as exc:
            return f"session load failed: {exc}"
        except ValueError as exc:
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

    if action == "search":
        if len(args) < 2:
            return "usage: /session search <query>"
        hits = search_sessions(" ".join(args[1:]))
        if not hits:
            return "no sessions match"
        lines = [f"{len(hits)} session(s) match:"]
        for hit in hits:
            lines.append(f"  {hit['name']}  ({hit['message_count']} msgs)")
            for match in hit["matches"]:
                lines.append(f"    [{match['role']}] {match['snippet']}")
        return "\n".join(lines)

    if action == "delete":
        if len(args) < 2:
            return "usage: /session delete <name>"
        name = args[1]
        if delete_session(name):
            return f"session deleted: {name}"
        return f"session not found: {name}"

    return "usage: /session save|load|list|search|delete"


def _handle_export_command(
    args: list[str], state: ChatState, workspace_dir: Path | None
) -> str:
    """Handle /export text|markdown|json|html command."""
    if workspace_dir is None:
        return "workspace not configured"

    fmt = args[0] if args else "markdown"
    if fmt not in {"text", "markdown", "json", "html"}:
        return f"unknown format: {fmt} (valid: text, markdown, json, html)"

    if not state.history:
        return "nothing to export — conversation is empty"

    try:
        content = export_conversation(state, fmt)
    except RuntimeError as exc:
        return str(exc)
    except Exception as exc:
        return f"export failed: {exc}"

    extension = {"markdown": "md", "text": "txt"}.get(fmt, fmt)
    output_path = workspace_dir / f"conversation-{datetime.datetime.now():%Y%m%d-%H%M%S}.{extension}"
    output_path.write_text(content, encoding="utf-8")

    return f"exported {len(state.history)} messages to {output_path}"


def _handle_plugin_command(args: list[str]) -> str:
    """Handle /plugin list|reload commands."""
    registry = get_registry()

    if not args or args[0] == "list":
        tools = registry.list_tools()
        if not tools:
            return "no custom plugins loaded"
        lines = [f"{len(tools)} plugin tool(s) loaded:"]
        for t in tools:
            src = f" ({t.source_file})" if t.source_file else ""
            warn = " ⚠️ network" if t.needs_network else ""
            lines.append(f"  {t.name}{src}{warn}")
        if registry.warnings:
            lines.append("")
            lines.append("warnings:")
            for w in registry.warnings:
                lines.append(f"  ⚠️ {w}")
        return "\n".join(lines)

    if args[0] == "reload":
        count, warnings = registry.reload()
        msg = f"plugins reloaded: {count} plugin(s) loaded"
        if warnings:
            msg += "\n" + "\n".join(f"  ⚠️ {w}" for w in warnings)
        return msg

    return "usage: /plugin list|reload"


def _handle_mcp_command(args: list[str]) -> str:
    """Handle /mcp list|tools|reconnect commands."""
    manager = get_mcp_manager()

    if not args or args[0] == "list":
        servers = manager.list_servers()
        if not servers:
            return "no MCP servers connected (configure mcp_servers in config.json)"
        lines = [f"{len(servers)} MCP server(s):"]
        for s in servers:
            status = "connected" if s["connected"] else "disconnected"
            lines.append(f"  {s['name']}  ({status}, {s['tool_count']} tools)")
        return "\n".join(lines)

    if args[0] == "tools":
        if len(args) < 2:
            return "usage: /mcp tools <server>"
        server_name = args[1]
        client = manager.get_client(server_name)
        if client is None:
            return f"MCP server not found: {server_name}"
        tools = client.tools
        if not tools:
            return f"no tools from MCP server '{server_name}'"
        lines = [f"{len(tools)} tool(s) from '{server_name}':"]
        for t in tools:
            lines.append(f"  {t.name}: {t.description[:100]}")
        return "\n".join(lines)

    if args[0] == "reconnect":
        if len(args) < 2:
            return "usage: /mcp reconnect <server>"
        server_name = args[1]
        error = manager.reconnect_server(server_name)
        if error is not None:
            return f"MCP reconnect failed: {error}"
        return f"MCP server reconnected: {server_name} (tools rediscovered)"

    return "usage: /mcp list|tools|reconnect"


def _format_state(state: ChatState) -> str:
    return _fmt_state(state)


def _status_line(state: ChatState) -> str:
    return _fmt_status(state)


def _format_history(state: ChatState) -> str:
    return _fmt_history(state)


def _format_skills(names: list[str]) -> str:
    return _fmt_skills(names)


def _format_workspace(workspace_dir: Path) -> str:
    return _fmt_workspace(workspace_dir)


def _human_size(size: int) -> str:
    return _fmt_human_size(size)
