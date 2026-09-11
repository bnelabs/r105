"""Shared protocol for built-in, plugin, and MCP tool metadata."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any, Protocol


class Tool(Protocol):
    """Minimal contract required by tool registries and documentation."""

    name: str
    description: str
    parameters_schema: dict[str, Any]

    def to_definition(self) -> dict[str, Any]:
        """Return the OpenAI-compatible function definition."""
        ...

    def execute(
        self,
        arguments: dict[str, Any],
        workspace_dir: Path,
        **kwargs: Any,
    ) -> str:
        """Execute the tool with validated arguments."""
        ...


ToolExecutor = Callable[..., str]

__all__ = ["Tool", "ToolExecutor"]
