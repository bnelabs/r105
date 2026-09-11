"""Generic component registry — unified abstraction for tools and plugins.

Both ``tools.ToolRegistry`` and ``plugins.PluginRegistry`` were nearly
identical registries (dict + add/get/list + definitions). This module provides
a single generic base so future components (MCP tools, skills, commands) can
share registration, shadowing protection, and definition export logic.
"""

from __future__ import annotations

import inspect
from typing import Any


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
