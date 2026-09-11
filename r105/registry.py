"""Generic component registry — unified abstraction for tools and plugins.

Both ``tools.ToolRegistry`` and ``plugins.PluginRegistry`` were nearly
identical registries (dict + add/get/list + definitions). This module provides
a single generic base so future components (MCP tools, skills, commands) can
share registration, shadowing protection, and definition export logic.
"""

from __future__ import annotations

import inspect
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

from r105.tool_protocol import Tool

# Handler signature: (arguments: dict, workspace_dir: Path, **kwargs) -> str.
# Handlers may declare fewer parameters (e.g. ``(arguments)`` or ``()``);
# see :func:`call_tool_handler`.
ToolHandler = Callable[..., str]


def call_tool_handler(handler: Any, arguments: dict[str, Any], workspace_dir: Any, **kwargs: Any) -> Any:
    """Invoke a tool handler, tolerating handlers that omit ``workspace_dir``.

    The documented contract is ``handler(arguments, workspace_dir, **kwargs)``,
    but pure tools (``get_time()``, ``calculate(arguments)``) and simple
    plugin handlers legitimately take fewer parameters. Calling those with the
    full signature raises ``TypeError`` and breaks dispatch, so adapt to the
    handler's declared positional arity instead. Handlers that cannot be
    introspected fall back to the full contract call.
    """
    try:
        parameters = inspect.signature(handler).parameters.values()
    except (TypeError, ValueError):
        return handler(arguments, workspace_dir, **kwargs)
    n_positional = sum(
        1
        for p in parameters
        if p.kind
        in (inspect.Parameter.POSITIONAL_ONLY, inspect.Parameter.POSITIONAL_OR_KEYWORD)
    )
    if n_positional == 0:
        return handler()
    if n_positional == 1:
        return handler(arguments)
    return handler(arguments, workspace_dir, **kwargs)


class ComponentRegistry[T]:
    """Generic name -> component registry with shadowing protection.

    *protected_names* are names owned by another registry (e.g. built-ins).
    Registering a protected name raises ``ValueError`` unless ``allow_overwrite``
    is True. Duplicate registration within the same registry raises as well
    unless explicitly allowed — preventing silent hijacks.
    """

    def __init__(self, *, allow_overwrite: bool = False) -> None:
        self._items: dict[str, T] = {}
        self._allow_overwrite = allow_overwrite
        self._protected_names: frozenset[str] = frozenset()
        self._warnings: list[str] = []

    # -- configuration ----------------------------------------------------

    def set_protected_names(self, names: set[str] | frozenset[str]) -> None:
        self._protected_names = frozenset(names)

    @property
    def allow_overwrite(self) -> bool:
        return self._allow_overwrite

    def set_allow_overwrite(self, allowed: bool) -> None:
        self._allow_overwrite = allowed

    # -- core -------------------------------------------------------------

    def _check_shadow(self, name: str, allow_overwrite: bool | None = None) -> None:
        effective = self._allow_overwrite if allow_overwrite is None else allow_overwrite
        if name in self._protected_names and not effective:
            raise ValueError(
                f"'{name}' shadows a protected component. "
                "Rename it or opt in with allow_overwrite=True."
            )
        if name in self._items and not effective:
            raise ValueError(
                f"'{name}' already registered — refusing to overwrite. "
                "Pass allow_overwrite=True to replace explicitly."
            )

    def register_item(self, name: str, item: T, *, allow_overwrite: bool | None = None) -> None:
        self._check_shadow(name, allow_overwrite)
        if name in self._items:
            self._warnings.append(f"'{name}' overwritten (explicit opt-in)")
        self._items[name] = item

    def get(self, name: str) -> T | None:
        return self._items.get(name)

    def list_items(self) -> list[T]:
        return list(self._items.values())

    def names(self) -> set[str]:
        return set(self._items.keys())

    @property
    def warnings(self) -> list[str]:
        return list(self._warnings)

    def clear(self) -> None:
        self._items.clear()
        self._warnings.clear()


@dataclass
class RegisteredTool:
    """Metadata + handler for a registered built-in tool."""

    name: str
    description: str
    parameters: dict[str, Any]
    required: list[str] = field(default_factory=list)
    needs_network: bool = False
    needs_filesystem: bool = False
    needs_output_truncation: bool = True
    needs_external_wrapping: bool = False  # XML <tool_output> tags
    handler: ToolHandler | None = None

    @property
    def parameters_schema(self) -> dict[str, Any]:
        """JSON-schema properties exposed by this tool."""
        return {
            "type": "object",
            "properties": self.parameters,
            "required": self.required,
        }

    def execute(
        self,
        arguments: dict[str, Any],
        workspace_dir: Path,
        **kwargs: Any,
    ) -> str:
        """Execute the registered handler through the shared tool contract."""
        if self.handler is None:
            return ""
        return cast(str, call_tool_handler(self.handler, arguments, workspace_dir, **kwargs))

    def to_definition(self) -> dict[str, Any]:
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters_schema,
            },
        }


class ToolRegistry(ComponentRegistry["RegisteredTool"]):
    """Decorator-based registry for built-in tools (unified abstraction).

    Usage::

        registry = ToolRegistry()

        @registry.register(
            name="my_tool",
            description="Does something useful.",
            parameters={
                "input": {"type": "string", "description": "Input value."},
            },
            required=["input"],
        )
        def my_tool(arguments: dict, workspace_dir: Path) -> str:
            return f"result: {arguments['input']}"
    """

    def __init__(self, *, allow_overwrite: bool = False) -> None:
        super().__init__(allow_overwrite=allow_overwrite)

    @property
    def _tools(self) -> dict[str, RegisteredTool]:
        # Backwards-compat: existing code touches ``registry._tools`` directly.
        return self._items

    @_tools.setter
    def _tools(self, value: dict[str, RegisteredTool]) -> None:
        self._items = value

    def register(
        self,
        name: str,
        description: str = "",
        parameters: dict[str, Any] | None = None,
        *,
        required: list[str] | None = None,
        needs_network: bool = False,
        needs_filesystem: bool = False,
        needs_output_truncation: bool = True,
        needs_external_wrapping: bool = False,
        allow_overwrite: bool | None = None,
    ) -> Callable[[ToolHandler], ToolHandler]:
        """Decorator that registers a function as a tool handler."""
        def decorator(handler: ToolHandler) -> ToolHandler:
            tool = RegisteredTool(
                name=name,
                description=description,
                parameters=parameters or {},
                required=required or [],
                needs_network=needs_network,
                needs_filesystem=needs_filesystem,
                needs_output_truncation=needs_output_truncation,
                needs_external_wrapping=needs_external_wrapping,
                handler=handler,
            )
            # Route through the unified registry so shadowing is checked.
            self.register_item(name, tool, allow_overwrite=allow_overwrite)
            return handler
        return decorator

    def get(self, name: str) -> RegisteredTool | None:
        return self._tools.get(name)

    def execute(self, name: str, arguments: dict[str, Any], workspace_dir: Path, **kwargs: Any) -> str | None:
        """Execute a registered tool. Returns None if not found."""
        tool = self._tools.get(name)
        if tool is None or tool.handler is None:
            return None
        return tool.execute(arguments, workspace_dir, **kwargs)

    def get_definitions(self) -> list[dict[str, Any]]:
        return [t.to_definition() for t in self._tools.values()]

    def get_tools(self) -> list[Tool]:
        """Return registered tools through the shared protocol surface."""
        return cast(list[Tool], self.list_tools())

    def list_tools(self) -> list[RegisteredTool]:
        return list(self._tools.values())


# Module-level singleton
_TOOL_REGISTRY = ToolRegistry()


def get_tool_registry() -> ToolRegistry:
    """Return the global tool registry singleton."""
    return _TOOL_REGISTRY
