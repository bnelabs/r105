"""Plugin system — load and register custom tools from Python files.

Plugins are Python files in the plugins directory (default:
~/.config/rova/plugins/) that expose a ``register(registry)`` function.

Example plugin::

    # ~/.config/rova/plugins/hello.py
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
import sys
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from rova.config import CONFIG_DIR

DEFAULT_PLUGINS_DIR = CONFIG_DIR / "plugins"

# Handler signature: (arguments: dict, workspace_dir: Path) -> str
ToolHandler = Callable[[dict[str, Any], Path], str]


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


class PluginRegistry:
    """Manages plugin loading and tool dispatch.

    Plugins are discovered from a directory of Python files. Each file
    must expose a ``register(registry)`` function.
    """

    def __init__(self, plugins_dir: Path | None = None) -> None:
        self._dir = Path(plugins_dir) if plugins_dir else DEFAULT_PLUGINS_DIR
        self._tools: dict[str, ToolPlugin] = {}
        self._loaded_files: set[str] = set()
        self._warnings: list[str] = []

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
    ) -> None:
        """Register a new tool. Called from plugin files' register() function."""
        if name in self._tools:
            self._warnings.append(f"Tool '{name}' already registered — overwriting")
        self._tools[name] = ToolPlugin(
            name=name,
            description=description,
            parameters=parameters,
            required=required or [],
            handler=handler,
            needs_network=needs_network,
            source_file=source_file,
        )

    # -- Discovery -----------------------------------------------------------

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
        """Import a single plugin file and call its register() function."""
        module_name = f"rova_plugin_{path.stem}"
        # Use a unique module name to avoid collisions on reload
        spec = importlib.util.spec_from_file_location(module_name, path)
        if spec is None or spec.loader is None:
            raise ImportError(f"Cannot load spec for {path}")

        module = importlib.util.module_from_spec(spec)
        sys.modules[module_name] = module
        try:
            spec.loader.exec_module(module)
            if hasattr(module, "register") and callable(module.register):
                module.register(self)
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
            return tool.handler(arguments, workspace_dir)
        except Exception as exc:
            return f"plugin error ({name}): {exc}"


# -- Module-level registry (shared singleton) --------------------------------

_registry: PluginRegistry | None = None


def get_registry(plugins_dir: Path | None = None) -> PluginRegistry:
    """Return the shared plugin registry, creating it if needed."""
    global _registry
    if _registry is None:
        _registry = PluginRegistry(plugins_dir)
    return _registry


def init_registry(plugins_dir: Path | None = None) -> PluginRegistry:
    """Initialize (or reinitialize) the shared registry with discovery."""
    global _registry
    _registry = PluginRegistry(plugins_dir)
    _registry.discover()
    return _registry
