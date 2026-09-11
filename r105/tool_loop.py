"""Tool-loop mechanics — pure, UI-independent pieces of the agent loop.

The async orchestration (streaming widgets, cancellation, thread pool) stays
in ``r105.tui.screens.chat``; everything here is deterministic and unit
tested: signature parsing, repeat/dedup tracking, per-tool timeouts, and
the synthetic result messages for timeouts, crashes, and loop breaks.
"""

from __future__ import annotations

import json
from collections import deque
from typing import Any

from r105.constants import (
    MAX_REPEATED_TOOL_CALLS,
    RECENT_CALL_TRACKING_SIZE,
    TOOL_TIMEOUT_DEFAULT,
    TOOL_TIMEOUT_EXECUTE_PYTHON,
    TOOL_TIMEOUT_FILE_OPS,
    TOOL_TIMEOUT_WEB_FETCH,
    TOOL_TIMEOUT_WEB_SEARCH,
)
from r105.tools import repair_tool_arguments

# (tool_call, name, args_str) — args_str is canonical JSON for dedup keys.
ToolSignature = tuple[dict[str, Any], str, str]


def parse_tool_signatures(tool_calls: list[dict[str, Any]]) -> list[ToolSignature]:
    """Pre-parse tool-call signatures once per loop iteration.

    Uses structured-output repair so malformed LLM JSON doesn't crash the
    loop; unparseable arguments fall back to ``{}``.
    """
    signatures: list[ToolSignature] = []
    for tc in tool_calls:
        func = tc.get("function", {})
        name = func.get("name", "unknown")
        try:
            args = repair_tool_arguments(func.get("arguments", {}))
        except Exception:
            args = {}
        args_str = json.dumps(args, indent=2, sort_keys=True)
        signatures.append((tc, name, args_str))
    return signatures


class LoopDedupTracker:
    """Cross-iteration duplicate detection for tool calls.

    Tracks ``(name, args)`` keys with consecutive-repeat counts plus a
    bounded window of recent calls. ``check()`` returns ``(stuck, repeat)``:
    *stuck* means the loop must break now, *repeat* means warn once.
    """

    def __init__(self) -> None:
        self.repeat_counts: dict[tuple[str, str], int] = {}
        self.recent_calls: deque[tuple[str, str]] = deque()

    def check(self, name: str, args_str: str) -> tuple[bool, bool]:
        """Record a call; return ``(stuck, repeat)`` flags."""
        call_key = (name, args_str)
        count = self.repeat_counts.get(call_key, 0) + 1
        self.repeat_counts[call_key] = count
        repeat = count > 1 and call_key in self.recent_calls
        self.recent_calls.append(call_key)
        while len(self.recent_calls) > RECENT_CALL_TRACKING_SIZE:
            self.recent_calls.popleft()
        stuck = count > MAX_REPEATED_TOOL_CALLS
        return stuck, repeat

    def count_for(self, name: str, args_str: str) -> int:
        """Consecutive-repeat count for a call key (0 when unseen)."""
        return self.repeat_counts.get((name, args_str), 0)

    @staticmethod
    def stop_loop_message(name: str, count: int) -> str:
        """System/history message injected when forcing a loop break."""
        return (
            f"[STOP LOOP] You called {name} with the same arguments "
            f"{count} times in a row. Do NOT repeat it again. "
            "Answer directly or try a completely different approach."
        )

    @staticmethod
    def duplicate_note(name: str) -> str:
        """Note appended to a repeated tool result."""
        return (
            f"\n\n[SYSTEM NOTE: You just called {name} with "
            "the same arguments. This already failed or returned no useful "
            "result. Do NOT repeat this call. Try a different approach.]"
        )

    def is_duplicate_result(self, name: str, args_str: str) -> bool:
        """Whether a result should carry the duplicate warning note."""
        return self.recent_calls.count((name, args_str)) >= 2


def tool_timeout(tool_name: str) -> float:
    """Per-tool execution timeout in seconds."""
    if tool_name == "execute_python":
        return TOOL_TIMEOUT_EXECUTE_PYTHON
    if tool_name == "web_search":
        return TOOL_TIMEOUT_WEB_SEARCH
    if tool_name == "web_fetch":
        return TOOL_TIMEOUT_WEB_FETCH
    if tool_name in ("write_file", "read_file", "list_files"):
        return TOOL_TIMEOUT_FILE_OPS
    return TOOL_TIMEOUT_DEFAULT


def timeout_result(tc: dict[str, Any], tool_name: str, timeout: float) -> dict[str, Any]:
    """Synthetic tool result for an execution timeout."""
    return {
        "role": "tool",
        "tool_call_id": tc.get("id", ""),
        "name": tool_name,
        "content": f"error: {tool_name} timed out after {timeout}s",
    }


def exception_result(tc: dict[str, Any], tool_name: str, exc: BaseException) -> dict[str, Any]:
    """Synthetic tool result for a crashed execution."""
    return {
        "role": "tool",
        "tool_call_id": tc.get("id", ""),
        "name": tool_name,
        "content": f"error: {exc}",
    }
