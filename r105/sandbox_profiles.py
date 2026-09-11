"""Declarative sandbox profiles for r105 tool categories.

Execution backends consume these profiles, but the policy definitions do not
need to know whether nsjail, bubblewrap, Docker, or the fallback backend is
selected. Keeping them separate makes capability changes easier to review.
"""

from __future__ import annotations

from dataclasses import dataclass

from r105.constants import (
    SANDBOX_CPU_SECONDS,
    SANDBOX_MEMORY_MB,
    SANDBOX_TIMEOUT,
)


@dataclass(frozen=True)
class SandboxProfile:
    """Isolation and resource requirements for one tool execution."""

    needs_network: bool = False
    needs_filesystem: bool = False
    needs_write: bool = False
    seccomp: bool = True
    seccomp_policy: str = ""
    timeout: float = SANDBOX_TIMEOUT
    memory_mb: int = SANDBOX_MEMORY_MB
    cpu_seconds: int = SANDBOX_CPU_SECONDS


PROFILE_EXECUTE_PYTHON = SandboxProfile()
PROFILE_FILE_TOOLS = SandboxProfile(needs_filesystem=True, needs_write=True)
PROFILE_WEB_TOOLS = SandboxProfile(needs_network=True)
PROFILE_SYSTEM_TOOLS = SandboxProfile(seccomp=False)


_PROFILES: dict[str, SandboxProfile] = {
    "execute_python": PROFILE_EXECUTE_PYTHON,
    "write_file": PROFILE_FILE_TOOLS,
    "read_file": PROFILE_FILE_TOOLS,
    "list_files": PROFILE_FILE_TOOLS,
    "web_search": PROFILE_WEB_TOOLS,
    "web_fetch": PROFILE_WEB_TOOLS,
    "get_time": PROFILE_SYSTEM_TOOLS,
    "calculate": PROFILE_SYSTEM_TOOLS,
    "system_info": PROFILE_SYSTEM_TOOLS,
}


def profile_for_tool(name: str) -> SandboxProfile:
    """Return the strictest profile requested by a built-in tool."""
    return _PROFILES.get(name, PROFILE_EXECUTE_PYTHON)


__all__ = [
    "PROFILE_EXECUTE_PYTHON",
    "PROFILE_FILE_TOOLS",
    "PROFILE_SYSTEM_TOOLS",
    "PROFILE_WEB_TOOLS",
    "SandboxProfile",
    "profile_for_tool",
]
