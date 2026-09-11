"""CLI entry point for r105."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import httpx

from r105 import __version__
from r105.client import BaseClient, create_client
from r105.config import ensure_config, export_config_schema, load_state_overrides
from r105.mcp_client import load_mcp_servers
from r105.model_catalog import resolve_context_tokens
from r105.plugins import init_registry
from r105.sandbox import detect_backend, get_fallback_reason, set_posture
from r105.sessions import load_session
from r105.state import (
    DEFAULT_MODEL,
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
)

DEFAULT_URL = "http://127.0.0.1:8010"
DEFAULT_WORKSPACE = Path.home() / "r105-workspace"
DEFAULT_SKILLS_DIR = Path("skills")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="r105",
        description="r105 — Beyond the prompt. Rich terminal AI assistant for any OpenAI-compatible backend.",
    )
    parser.add_argument(
        "--url", default=DEFAULT_URL, help="API base URL, default: %(default)s"
    )
    parser.add_argument(
        "--workspace",
        default=str(DEFAULT_WORKSPACE),
        help="Workspace for generated files, default: %(default)s",
    )
    parser.add_argument(
        "--skills-dir",
        default=str(DEFAULT_SKILLS_DIR),
        help="Skills directory, default: %(default)s",
    )
    parser.add_argument(
        "--plugins-dir",
        default=None,
        help="Custom tool plugins directory, default: ~/.config/r105/plugins",
    )
    parser.add_argument(
        "--profile", choices=sorted(VALID_PROFILES), help="Force a router task profile"
    )
    parser.add_argument(
        "--quality", choices=sorted(VALID_QUALITIES), help="Set quality hint metadata"
    )
    parser.add_argument(
        "--max-tokens", type=int, help="Override max_tokens for chat requests"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_mode",
        help="Ask for JSON object responses",
    )
    parser.add_argument(
        "--model", default=None, help="Model name to use for chat requests",
    )
    parser.add_argument(
        "--backend", default=None, choices=["direct", "router"],
        help="Backend type (router for profiles, direct for any OpenAI API)",
    )
    parser.add_argument(
        "--version", action="version", version=f"r105 {__version__}"
    )
    parser.add_argument(
        "--session",
        default=None,
        help="Load a saved session on startup",
    )
    parser.add_argument(
        "--yes", "-y",
        action="store_true",
        help="Auto-approve execute_python calls (skip the confirmation gate)",
    )

    subparsers = parser.add_subparsers(dest="command")
    send_parser = subparsers.add_parser("send", help="Send one prompt and exit")
    send_parser.add_argument("message", nargs="+")
    subparsers.add_parser("chat", help="Start interactive chat (default)")
    subparsers.add_parser("health", help="Check router health")
    subparsers.add_parser("doctor", help="Diagnose environment: config, sandbox, backend, workspace")
    profiles_parser = subparsers.add_parser("profiles", help="Show router profiles")
    profiles_parser.add_argument("--raw", action="store_true", help="Print raw JSON")
    schema_parser = subparsers.add_parser(
        "config-schema", help="Print or write the config.json JSON Schema"
    )
    schema_parser.add_argument(
        "--output", default=None, help="Write the schema to this path instead of stdout"
    )
    parser.set_defaults(command="chat")
    return parser


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    return build_parser().parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    if args.command == "config-schema":
        schema = export_config_schema(Path(args.output) if args.output else None)
        if args.output:
            print(f"wrote config schema to {Path(args.output).expanduser()}")
        else:
            print(json.dumps(schema, indent=2, sort_keys=True))
        return 0

    # Load config file for defaults (CLI args take precedence)
    config = ensure_config()
    url = args.url if args.url != DEFAULT_URL else config.get("url") or args.url
    workspace_str = (
        args.workspace
        if args.workspace != str(DEFAULT_WORKSPACE)
        else config.get("workspace", str(DEFAULT_WORKSPACE))
    )
    skills_str = (
        args.skills_dir
        if args.skills_dir != str(DEFAULT_SKILLS_DIR)
        else config.get("skills_dir", str(DEFAULT_SKILLS_DIR))
    )

    # Initialize permission posture + sandbox backend
    posture = config.get("permission_posture", "sandboxed")
    sandbox_name = config.get("sandbox_backend", "auto")
    try:
        set_posture(posture, backend=sandbox_name)
    except Exception:
        # Fall back to auto-detection if the posture cannot be applied
        detect_backend()
    # Surface sandbox downgrades: a silent fallback to rlimit/none means
    # tool code runs without filesystem/network isolation.
    try:
        fallback_reason = get_fallback_reason()
    except Exception:
        fallback_reason = None
    if fallback_reason:
        print(f"warning: {fallback_reason}", file=sys.stderr)

    # Initialize plugin registry
    plugins_str = (
        args.plugins_dir
        if args.plugins_dir
        else config.get("plugins_dir")
    )
    if plugins_str:
        init_registry(Path(plugins_str).expanduser().resolve())
    else:
        init_registry()  # uses default path

    # Load MCP servers
    mcp_configs: list[dict[str, Any]] = config.get("mcp_servers") or []
    if mcp_configs:
        mcp_errors = load_mcp_servers(mcp_configs)
        for err in mcp_errors:
            print(f"warning: {err}", file=sys.stderr)

    workspace_dir = Path(workspace_str).expanduser().resolve()
    skills_dir = Path(skills_str).expanduser().resolve()
    workspace_dir.mkdir(parents=True, exist_ok=True)

    # Create the API client (auto-detects router vs direct)
    client = create_client(base_url=url, backend=args.backend)

    # Load state-level overrides from config, then apply CLI args on top
    state_overrides = load_state_overrides()
    model = args.model or state_overrides.get("model") or DEFAULT_MODEL

    # Resolve the model's context-window capacity:
    #   config override > backend probe > built-in catalog > default
    backend_context: int | None = None
    try:
        backend_context = client.probe_context(model)
    except Exception:
        backend_context = None
    context_tokens = resolve_context_tokens(
        model,
        config_contexts=state_overrides.get("model_contexts"),
        global_override=state_overrides.get("context_tokens"),
        backend_context=backend_context,
    )

    state = ChatState(
        profile=args.profile or state_overrides.get("profile"),
        quality=args.quality or state_overrides.get("quality"),
        max_tokens=args.max_tokens,
        json_mode=args.json_mode,
        auto_compact=state_overrides.get("auto_compact", True),
        cache_prompt=state_overrides.get("cache_prompt", False),
        theme=state_overrides.get("theme", "r105"),
        model=model,
        reasoning_effort=state_overrides.get("reasoning_effort", "auto"),
        show_thinking=state_overrides.get("show_thinking", True),
        thinking_default_expanded=state_overrides.get("thinking_default_expanded", False),
        permission_posture=state_overrides.get("permission_posture", "full-access"),
        model_contexts=state_overrides.get("model_contexts") or {},
        context_tokens=context_tokens,
        skills_dir=skills_dir,
    )

    # Load session if requested
    if args.session:
        try:
            count = load_session(state, args.session)
            print(f"Loaded session '{args.session}': {count} messages restored", file=sys.stderr)
        except FileNotFoundError:
            print(f"warning: session not found: {args.session}", file=sys.stderr)
        except (json.JSONDecodeError, OSError) as exc:
            print(f"warning: failed to load session: {exc}", file=sys.stderr)

    # --yes bypasses the execute_python confirmation gate for this run.
    from r105.tools import set_execute_python_auto_approve
    set_execute_python_auto_approve(
        bool(args.yes or config.get("auto_approve_execute_python", False))
    )

    try:
        if args.command == "send":
            result = client.send(" ".join(args.message), state)
            _print_chat_result(result)
            return 0
        if args.command == "doctor":
            from r105.doctor import collect_probes, run_doctor

            report = run_doctor(**collect_probes(
                client=client,
                backend_url=client.base_url,
                workspace_dir=workspace_dir,
                skills_dir=skills_dir,
            ))
            print(report.render())
            return 0 if report.passed else 1
        if args.command == "health":
            print(json.dumps(client.health(), indent=2, sort_keys=True))  # type: ignore[attr-defined]
            return 0
        if args.command == "profiles":
            if not hasattr(client, "profiles"):
                print("profiles are only available with llama-router backend (--backend router)", file=sys.stderr)
                return 1
            payload = client.profiles()
            if args.raw:
                print(json.dumps(payload, indent=2, sort_keys=True))
            else:
                for name, profile in sorted(
                    (payload.get("profiles") or {}).items()
                ):
                    print(
                        f"{name}: max_tokens={profile.get('max_tokens')} "
                        f"reasoning={profile.get('reasoning')}"
                    )
            return 0
        return _run_tui(client, state, workspace_dir)
    except (httpx.HTTPError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


def _run_tui(client: BaseClient, state: ChatState, workspace_dir: Path) -> int:
    from r105.tui.app import run_app

    run_app(client, state, workspace_dir)  # type: ignore[arg-type]
    return 0


def _print_chat_result(result: Any) -> None:
    print(result.content)
    parts = [f"wall={result.wall_seconds:.2f}s"]
    if result.prompt_tps is not None:
        parts.append(f"prompt_tps={result.prompt_tps:.1f}")
    if result.generation_tps is not None:
        parts.append(f"gen_tps={result.generation_tps:.1f}")
    print("[" + " ".join(parts) + "]", file=sys.stderr)
