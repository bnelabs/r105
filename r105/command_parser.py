"""Parsing and registration primitives for r105 slash commands.

The command handlers still live in :mod:`r105.commands` for compatibility,
but parsing and lookup are kept independent so a new frontend can reuse the
same command grammar without importing the handler module's implementation
details.
"""

from __future__ import annotations

import difflib
import shlex
from collections.abc import Mapping
from dataclasses import dataclass


@dataclass(frozen=True)
class ParsedCommand:
    """A command name and its shell-like argument list."""

    name: str
    args: list[str]


class CommandParser:
    """Parse one slash-command line using shell-style quoting."""

    def parse(self, line: str) -> ParsedCommand | None:
        """Return a parsed command, or ``None`` for blank input."""
        parts = shlex.split(line)
        if not parts:
            return None
        return ParsedCommand(parts[0], parts[1:])


class CommandRegistry[HandlerT]:
    """Name-to-handler registry with fuzzy suggestions.

    Registration is explicit and duplicate names are rejected.  This keeps
    command additions reviewable and prevents a later import from silently
    replacing an existing handler.
    """

    def __init__(self, handlers: Mapping[str, HandlerT] | None = None) -> None:
        self._handlers: dict[str, HandlerT] = {}
        if handlers:
            for name, handler in handlers.items():
                self.register(name, handler)

    def register(self, name: str, handler: HandlerT) -> None:
        """Register *handler* under *name* once."""
        if not name.startswith("/"):
            raise ValueError(f"command names must start with '/': {name!r}")
        if name in self._handlers:
            raise ValueError(f"command already registered: {name}")
        self._handlers[name] = handler

    def get(self, name: str) -> HandlerT | None:
        """Return a handler by exact command name."""
        return self._handlers.get(name)

    def names(self) -> list[str]:
        """Return registered command names in registration order."""
        return list(self._handlers)

    def suggest(self, name: str, *, cutoff: float = 0.6) -> str | None:
        """Return the closest registered command, if it is a good match."""
        matches = difflib.get_close_matches(name, self._handlers, n=1, cutoff=cutoff)
        return matches[0] if matches else None

    def __contains__(self, name: object) -> bool:
        return name in self._handlers


def parse_command(line: str) -> ParsedCommand | None:
    """Convenience wrapper for callers that do not need a parser instance."""
    return CommandParser().parse(line)


__all__ = ["CommandParser", "CommandRegistry", "ParsedCommand", "parse_command"]
