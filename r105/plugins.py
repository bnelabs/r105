"""Plugin system — load and register custom tools from Python files.

Plugins are Python files in the plugins directory (default:
~/.config/r105/plugins/) that expose a ``register(registry)`` function.

Example plugin::

    # ~/.config/r105/plugins/hello.py
    def register(registry):
        registry.add_tool(
            name="hello",
            description="Say hello to someone.",
            parameters={
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name to greet."},
                },
                "required": ["name"],
            },
            handler=lambda args, ws: f"Hello, {args.get('name', 'world')}!",
        )
"""

from __future__ import annotations

import importlib.util
import inspect
import re
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, cast

from r105.config import CONFIG_DIR
from r105.errors import PluginError
from r105.registry import ComponentRegistry, call_tool_handler

DEFAULT_PLUGINS_DIR = CONFIG_DIR / "plugins"

# Handler signature: (arguments: dict, workspace_dir: Path) -> str
ToolHandler = Callable[[dict[str, Any], Path], str]

# Plugin tool names: non-empty, no whitespace (LLM tool-call compatible).
_TOOL_NAME_RE = re.compile(r"^[A-Za-z0-9_.-]+$")


def validate_tool_definition(
    name: str,
    description: str,
    parameters: dict[str, Any],
    required: list[str] | None,
    handler: ToolHandler | None,
) -> None:
    """Validate a plugin tool definition against the expected schema.

    Raises :class:`PluginError` with a specific message on the first
    violation so ``/plugin reload`` can show exactly what is wrong instead
    of silently skipping the plugin.
    """
    if not isinstance(name, str) or not name.strip():
        raise PluginError(f"tool name must be a non-empty string, got {name!r}")
    if not _TOOL_NAME_RE.match(name):
        raise PluginError(
            f"tool name {name!r} is invalid — use letters, digits, '_', '-', '.' only"
        )
    if not isinstance(description, str) or not description.strip():
        raise PluginError(f"tool '{name}' needs a non-empty description string")
    if not isinstance(parameters, dict):
        raise PluginError(
            f"tool '{name}' parameters must be an object mapping names to schemas, "
            f"got {type(parameters).__name__}"
        )
    for param_name, schema in parameters.items():
        if not isinstance(param_name, str) or not param_name.strip():
            raise PluginError(f"tool '{name}' has an invalid parameter name: {param_name!r}")
        if not isinstance(schema, dict):
            raise PluginError(
                f"tool '{name}' parameter {param_name!r} schema must be an object, "
                f"got {type(schema).__name__}"
            )
    required_list = required or []
    if not isinstance(required_list, list) or any(
        not isinstance(item, str) for item in required_list
    ):
        raise PluginError(f"tool '{name}' required must be a list of strings")
    unknown = [item for item in required_list if item not in parameters]
    if unknown:
        raise PluginError(
            f"tool '{name}' marks undeclared parameters as required: {unknown} "
            f"(declared: {sorted(parameters)})"
        )
    if handler is not None and not callable(handler):
        raise PluginError(f"tool '{name}' handler is not callable")


@dataclass
class ToolPlugin:
    """A custom tool registered by a plugin."""

    name: str
    description: str
    parameters: dict[str, Any]  # JSON Schema properties object
    required: list[str] = field(default_factory=list)
    handler: ToolHandler | None = None
    needs_network: bool = False
    source_file: str = ""  # for /plugin list display

    def to_definition(self) -> dict[str, Any]:
        """Convert to the standard TOOL_DEFINITIONS format."""
        return {
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": {
                    "type": "object",
                    "properties": self.parameters,
                    "required": self.required,
                },
            },
        }


class PluginRegistry(ComponentRegistry["ToolPlugin"]):
    """Manages plugin loading and tool dispatch.

    Subclasses :class:`r105.registry.ComponentRegistry` so plugins and
    built-in tools share a single registration abstraction with identical
    shadowing protection semantics.

    Plugins are discovered from a directory of Python files. Each file
    must expose a ``register(registry)`` function.
    """

    def __init__(self, plugins_dir: Path | None = None, *, allow_overwrite: bool = False) -> None:
        super().__init__(allow_overwrite=allow_overwrite)
        self._dir = Path(plugins_dir) if plugins_dir else DEFAULT_PLUGINS_DIR
        self._loaded_files: set[str] = set()
        # NOTE: ``_items`` (from ComponentRegistry) holds the tools; ``_tools``
        # is kept as a backwards-compat alias via property below.

    @property
    def _tools(self) -> dict[str, ToolPlugin]:
        return self._items

    @_tools.setter
    def _tools(self, value: dict[str, ToolPlugin]) -> None:
        self._items = value

    # -- Registration --------------------------------------------------------

    def add_tool(
        self,
        name: str,
        description: str,
        parameters: dict[str, Any],
        *,
        required: list[str] | None = None,
        handler: ToolHandler | None = None,
        needs_network: bool = False,
        source_file: str = "",
        allow_overwrite: bool | None = None,
    ) -> None:
        """Register a new tool. Called from plugin files' register() function.

        SECURITY: silently overwriting an existing tool (built-in or another
        plugin) would let a malicious plugin hijack core tools such as
        ``execute_python``. By default overwrites are REJECTED with
        ``PluginError``; pass ``allow_overwrite=True`` (or enable it on the
        registry via ``set_allow_overwrite(True)`` / config
        ``allow_plugin_overrides``) to opt in explicitly.
        """
        effective_allow = self._allow_overwrite if allow_overwrite is None else allow_overwrite
        validate_tool_definition(name, description, parameters, required, handler)
        if name in self._protected_names and not effective_allow:
            raise PluginError(
                f"Plugin tool '{name}' shadows a built-in tool. "
                "Refusing to overwrite. Rename the plugin tool or set "
                "allow_plugin_overrides=true / R105_ALLOW_PLUGIN_OVERRIDE=1 "
                "to allow shadowing explicitly."
            )
        if name in self._tools and not effective_allow:
            raise PluginError(
                f"Tool '{name}' already registered — refusing to overwrite. "
                "Pass allow_overwrite=True to replace it explicitly."
            )
        if name in self._tools:
            self._warnings.append(f"Tool '{name}' already registered — overwriting (explicit opt-in)")
        self._tools[name] = ToolPlugin(
            name=name,
            description=description,
            parameters=parameters,
            required=required or [],
            handler=handler,
            needs_network=needs_network,
            source_file=source_file,
        )

    def set_protected_names(self, names: set[str] | frozenset[str]) -> None:
        """Mark built-in tool names as protected against shadowing."""
        self._protected_names = frozenset(names)

    @property
    def allow_overwrite(self) -> bool:
        return self._allow_overwrite

    def set_allow_overwrite(self, allowed: bool) -> None:
        """Allow/disallow plugins overwriting existing tools (default: disallowed)."""
        self._allow_overwrite = allowed

    def discover(self) -> int:
        """Scan the plugins directory and load all .py files.

        Returns the number of plugins successfully loaded.
        """
        if not self._dir.exists():
            return 0

        loaded = 0
        for path in sorted(self._dir.glob("*.py")):
            if not path.is_file():
                continue
            if path.name.startswith("_"):
                continue
            try:
                self._load_file(path)
                loaded += 1
            except Exception as exc:
                self._warnings.append(f"Failed to load {path.name}: {exc}")

        return loaded

    def reload(self) -> tuple[int, list[str]]:
        """Re-discover all plugins (clear + reload).

        Returns (count, warnings).
        """
        self._tools.clear()
        self._loaded_files.clear()
        self._warnings.clear()
        count = self.discover()
        return count, list(self._warnings)

    def _load_file(self, path: Path) -> None:
        """Import a single plugin file and call its register() function.

        The module must expose ``register(registry)`` taking exactly one
        required argument; every tool it adds is schema-validated. Problems
        raise :class:`PluginError` with a specific message so ``discover()``
        can report *why* the plugin was rejected.
        """
        module_name = f"r105_plugin_{path.stem}"
        # Use a unique module name to avoid collisions on reload
        spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None:
            raise PluginError(f"Cannot load spec for {path}")

        module = importlib.util.module_from_spec(spec)
        sys.modules[module_name] = module
        try:
            spec.loader.exec_module(module)
            register = getattr(module, "register", None)
            if register is None:
                raise PluginError(
                    f"{path.name} has no register(registry) function — skipped"
                )
            if not callable(register):
                raise PluginError(
                    f"{path.name} 'register' is not callable — skipped"
                )
            try:
                signature = inspect.signature(register)
            except (TypeError, ValueError) as exc:
                raise PluginError(
                    f"{path.name} register() signature is not introspectable: {exc}"
                ) from exc
            required_params = [
                param
                for param in signature.parameters.values()
                if param.default is inspect.Parameter.empty
                and param.kind
                in (
                    inspect.Parameter.POSITIONAL_ONLY,
                    inspect.Parameter.POSITIONAL_OR_KEYWORD,
                )
            ]
            if len(required_params) != 1:
                raise PluginError(
                    f"{path.name} register() must take exactly one required "
                    f"argument (the registry), got {len(required_params)}"
                )
            before = set(self._tools)
            register(self)
            if set(self._tools) == before:
                self._warnings.append(
                    f"{path.name} register() added no tools — check add_tool() calls"
                )
        finally:
            # Keep module alive but remove from sys.modules to allow reload
            pass

        self._loaded_files.add(path.name)

    # -- Query ---------------------------------------------------------------

    def get_tool(self, name: str) -> ToolPlugin | None:
        """Return a registered tool by name, or None."""
        return self._tools.get(name)

    def get_definitions(self) -> list[dict[str, Any]]:
        """Return all registered tools as TOOL_DEFINITIONS entries."""
        return [t.to_definition() for t in self._tools.values()]

    def list_tools(self) -> list[ToolPlugin]:
        """Return all registered tools."""
        return list(self._tools.values())

    @property
    def warnings(self) -> list[str]:
        return list(self._warnings)

    @property
    def tool_count(self) -> int:
        return len(self._tools)

    @property
    def dir_path(self) -> Path:
        return self._dir

    # -- Execution -----------------------------------------------------------

    def execute(self, name: str, arguments: dict[str, Any], workspace_dir: Path) -> str | None:
        """Execute a plugin tool. Returns the result string, or None if not found."""
        tool = self._tools.get(name)
        if tool is None or tool.handler is None:
            return None
        try:
            result = call_tool_handler(tool.handler, arguments, workspace_dir)
            return cast("str | None", result)
        except Exception as exc:
            return f"plugin error ({name}): {exc}"


# -- Module-level registry (shared singleton) --------------------------------

_registry: PluginRegistry | None = None


def get_registry(plugins_dir: Path | None = None) -> PluginRegistry:
    """Return the shared plugin registry, creating it if needed."""
    global _registry
    if _registry is None:
        _registry = PluginRegistry(plugins_dir)
        # Honor explicit opt-in for shadowing via env/config.
        import os as _os
        if _os.environ.get("R105_ALLOW_PLUGIN_OVERRIDE", "").lower() in {"1", "true", "yes", "on"}:
            _registry.set_allow_overwrite(True)
        else:
            try:
                from r105.config import ensure_config as _ensure_config
                if bool(_ensure_config().get("allow_plugin_overrides", False)):
                    _registry.set_allow_overwrite(True)
            except Exception:
                pass
    return _registry


def init_registry(plugins_dir: Path | None = None, *, allow_overwrite: bool | None = None) -> PluginRegistry:
    """Initialize (or reinitialize) the shared registry with discovery."""
    global _registry
    _registry = PluginRegistry(plugins_dir)
    if allow_overwrite is not None:
        _registry.set_allow_overwrite(allow_overwrite)
    _registry.discover()
    return _registry
