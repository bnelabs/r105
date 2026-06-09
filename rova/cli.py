"""CLI entry point for Rova."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

import httpx

from rova import __version__
from rova.client import RouterClient
from rova.commands import _format_ingest, _format_search, _split_paths_and_urls
from rova.config import ensure_config, load_state_overrides
from rova.state import (
    VALID_PROFILES,
    VALID_QUALITIES,
    ChatState,
)

DEFAULT_URL = "http://127.0.0.1:8010"
DEFAULT_WORKSPACE = Path.home() / "rova-workspace"
DEFAULT_SKILLS_DIR = Path("skills")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="rova",
        description="Rova — rich terminal front end for llama-router.",
    )
    parser.add_argument(
        "--url", default=DEFAULT_URL, help="Router base URL, default: %(default)s"
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
        "--profile", choices=sorted(VALID_PROFILES), help="Force a router task profile"
    )
    parser.add_argument("--rag", action="store_true", help="Enable RAG for chat requests")
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
        "--version", action="version", version=f"rova {__version__}"
    )

    subparsers = parser.add_subparsers(dest="command")
    send_parser = subparsers.add_parser("send", help="Send one prompt and exit")
    send_parser.add_argument("message", nargs="+")
    subparsers.add_parser("chat", help="Start interactive chat (default)")
    subparsers.add_parser("health", help="Check router health")
    profiles_parser = subparsers.add_parser("profiles", help="Show router profiles")
    profiles_parser.add_argument("--raw", action="store_true", help="Print raw JSON")
    ingest_parser = subparsers.add_parser("ingest", help="Ingest paths or URLs for RAG")
    ingest_parser.add_argument("items", nargs="+")
    search_parser = subparsers.add_parser("search", help="Search the active RAG index")
    search_parser.add_argument("query", nargs="+")
    search_parser.add_argument("--top-k", type=int, default=5)
    parser.set_defaults(command="chat")
    return parser


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    return build_parser().parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)

    # Load config file for defaults (CLI args take precedence)
    config = ensure_config()
    url = args.url if args.url != DEFAULT_URL else config.get("url", DEFAULT_URL)
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

    client = RouterClient(url)
    workspace_dir = Path(workspace_str).expanduser().resolve()
    skills_dir = Path(skills_str).expanduser().resolve()
    workspace_dir.mkdir(parents=True, exist_ok=True)

    # Load state-level overrides from config, then apply CLI args on top
    state_overrides = load_state_overrides()
    state = ChatState(
        profile=args.profile or state_overrides.get("profile"),
        rag=True if args.rag else None,
        quality=args.quality or state_overrides.get("quality"),
        max_tokens=args.max_tokens,
        json_mode=args.json_mode,
        auto_compact=state_overrides.get("auto_compact", True),
        theme=state_overrides.get("theme", "rova"),
        skills_dir=skills_dir,
    )

    try:
        if args.command == "send":
            result = client.send(" ".join(args.message), state)
            _print_chat_result(result)
            return 0
        if args.command == "health":
            print(json.dumps(client.health(), indent=2, sort_keys=True))
            return 0
        if args.command == "profiles":
            payload = client.profiles()
            if args.raw:
                print(json.dumps(payload, indent=2, sort_keys=True))
            else:
                for name, profile in sorted(
                    (payload.get("profiles") or {}).items()
                ):
                    print(
                        f"{name}: max_tokens={profile.get('max_tokens')} "
                        f"rag={profile.get('rag')} "
                        f"reasoning={profile.get('reasoning')}"
                    )
            return 0
        if args.command == "ingest":
            paths, urls = _split_paths_and_urls(args.items)
            payload = client.ingest(paths=paths, urls=urls)
            print(_format_ingest(payload))
            return 0
        if args.command == "search":
            payload = client.search(" ".join(args.query), top_k=args.top_k)
            print(_format_search(payload))
            return 0
        return _run_tui(client, state, workspace_dir)
    except (httpx.HTTPError, OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


def _run_tui(client: RouterClient, state: ChatState, workspace_dir: Path) -> int:
    from rova.tui.app import run_app

    run_app(client, state, workspace_dir)
    return 0


def _print_chat_result(result: Any) -> None:
    print(result.content)
    parts = [f"wall={result.wall_seconds:.2f}s"]
    if result.prompt_tps is not None:
        parts.append(f"prompt_tps={result.prompt_tps:.1f}")
    if result.generation_tps is not None:
        parts.append(f"gen_tps={result.generation_tps:.1f}")
    print("[" + " ".join(parts) + "]", file=sys.stderr)
