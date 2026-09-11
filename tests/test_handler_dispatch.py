"""Handler-arity dispatch tests.

Regression coverage: builtin handlers such as ``get_time()`` (0 args) and
``calculate(arguments)`` (1 arg) must work through registry dispatch, which
invokes handlers with the full ``(arguments, workspace_dir)`` contract.
Previously this raised ``TypeError`` for every 0/1-arg tool.
"""

import json
from pathlib import Path

from r105.plugins import PluginRegistry, ToolPlugin
from r105.registry import call_tool_handler
from r105.tools import ToolRegistry, execute_tool_call, get_tool_registry


def _make_registry() -> ToolRegistry:
    registry = ToolRegistry()

    @registry.register(name="zero")
    def _zero() -> str:
        return "zero-ok"

    @registry.register(name="one")
    def _one(arguments: dict) -> str:
        return f"one-ok:{arguments.get('x')}"

    @registry.register(name="two")
    def _two(arguments: dict, workspace_dir: Path) -> str:
        return f"two-ok:{workspace_dir.name}"

    return registry


class TestCallToolHandler:
    def test_zero_arg(self):
        assert call_tool_handler(lambda: "z", {}, Path(".")) == "z"

    def test_one_arg(self):
        assert call_tool_handler(lambda a: a["x"], {"x": 1}, Path(".")) == 1

    def test_two_arg(self):
        assert (
            call_tool_handler(lambda a, w: (a["x"], w.name), {"x": 1}, Path("wd"))
            == (1, "wd")
        )


class TestBuiltinRegistryDispatch:
    def test_all_arities(self, tmp_path: Path):
        registry = _make_registry()
        assert registry.execute("zero", {}, tmp_path) == "zero-ok"
        assert registry.execute("one", {"x": "1"}, tmp_path) == "one-ok:1"
        assert registry.execute("two", {}, tmp_path) == f"two-ok:{tmp_path.name}"
        assert registry.execute("missing", {}, tmp_path) is None

    def test_real_builtin_handlers(self, tmp_path: Path):
        registry = get_tool_registry()
        # get_time() takes no arguments at all — must survive dispatch.
        assert registry.execute("get_time", {}, tmp_path).strip()
        assert "4" in registry.execute("calculate", {"expression": "2+2"}, tmp_path)


class TestEndToEndToolCalls:
    def _call(self, name: str, arguments: dict) -> dict:
        return {
            "id": "call_dispatch_1",
            "function": {"name": name, "arguments": json.dumps(arguments)},
        }

    def test_calculate_through_dispatch(self, tmp_path: Path):
        result = execute_tool_call(self._call("calculate", {"expression": "2+2"}), tmp_path)
        assert "4" in result["content"]

    def test_get_time_through_dispatch(self, tmp_path: Path):
        result = execute_tool_call(self._call("get_time", {}), tmp_path)
        assert result["content"].strip()

    def test_unknown_tool_still_reported(self, tmp_path: Path):
        result = execute_tool_call(self._call("no_such_tool", {}), tmp_path)
        assert "unknown tool" in result["content"]


class TestPluginRegistryDispatch:
    def test_one_arg_plugin_handler(self, tmp_path: Path):
        registry = PluginRegistry()
        registry._tools["echo"] = ToolPlugin(
            name="echo",
            description="echo",
            parameters={},
            handler=lambda arguments: f"echo:{arguments.get('v')}",
        )
        assert registry.execute("echo", {"v": "hi"}, tmp_path) == "echo:hi"
