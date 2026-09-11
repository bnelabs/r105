"""Tests for plugin hot-reload validation (register signature + tool schema)."""

from __future__ import annotations

from pathlib import Path

import pytest

from r105.errors import PluginError
from r105.plugins import PluginRegistry, validate_tool_definition


def _registry() -> PluginRegistry:
    return PluginRegistry(plugins_dir=None)


class TestValidateToolDefinition:
    def test_valid_definition_passes(self) -> None:
        validate_tool_definition(
            "greet",
            "Say hello.",
            {"name": {"type": "string"}},
            ["name"],
            lambda args, ws: "hi",
        )

    def test_empty_name_rejected(self) -> None:
        with pytest.raises(PluginError, match="non-empty string"):
            validate_tool_definition("", "desc", {}, [], None)

    def test_bad_name_characters_rejected(self) -> None:
        with pytest.raises(PluginError, match="invalid"):
            validate_tool_definition("bad name!", "desc", {}, [], None)

    def test_empty_description_rejected(self) -> None:
        with pytest.raises(PluginError, match="description"):
            validate_tool_definition("tool", "  ", {}, [], None)

    def test_non_dict_parameters_rejected(self) -> None:
        with pytest.raises(PluginError, match="parameters must be an object"):
            validate_tool_definition("tool", "desc", ["x"], [], None)  # type: ignore[arg-type]

    def test_required_must_match_declared_params(self) -> None:
        with pytest.raises(PluginError, match="undeclared parameters"):
            validate_tool_definition(
                "tool", "desc", {"a": {"type": "string"}}, ["b"], None
            )

    def test_non_callable_handler_rejected(self) -> None:
        with pytest.raises(PluginError, match="not callable"):
            validate_tool_definition("tool", "desc", {}, [], "nope")  # type: ignore[arg-type]


class TestAddToolValidation:
    def test_add_tool_validates_schema(self) -> None:
        reg = _registry()
        with pytest.raises(PluginError):
            reg.add_tool(name="", description="d", parameters={})
        assert reg.tool_count == 0


def _write_plugin(tmp_path: Path, name: str, body: str) -> Path:
    path = tmp_path / name
    path.write_text(body, encoding="utf-8")
    return path


class TestLoadFileValidation:
    def test_missing_register_rejected(self, tmp_path: Path) -> None:
        _write_plugin(tmp_path, "bad.py", "VALUE = 42\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 0
        assert any("no register" in w for w in reg.warnings)

    def test_register_wrong_arity_rejected(self, tmp_path: Path) -> None:
        _write_plugin(tmp_path, "bad.py", "def register(a, b):\n    pass\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 0
        assert any("exactly one required argument" in w for w in reg.warnings)

    def test_register_zero_args_rejected(self, tmp_path: Path) -> None:
        _write_plugin(tmp_path, "bad.py", "def register():\n    pass\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 0
        assert any("exactly one required argument" in w for w in reg.warnings)

    def test_valid_plugin_loads(self, tmp_path: Path) -> None:
        _write_plugin(
            tmp_path,
            "good.py",
            "def register(registry):\n"
            "    registry.add_tool(\n"
            "        name='shout',\n"
            "        description='Shout text.',\n"
            "        parameters={'text': {'type': 'string'}},\n"
            "        required=['text'],\n"
            "        handler=lambda args, ws: args.get('text', '').upper(),\n"
            "    )\n",
        )
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 1
        assert reg.get_tool("shout") is not None

    def test_register_adding_no_tools_warns(self, tmp_path: Path) -> None:
        _write_plugin(tmp_path, "empty.py", "def register(registry):\n    pass\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 1
        assert any("added no tools" in w for w in reg.warnings)

    def test_invalid_tool_definition_reported(self, tmp_path: Path) -> None:
        _write_plugin(
            tmp_path,
            "bad.py",
            "def register(registry):\n"
            "    registry.add_tool(name='x', description='', parameters={})\n",
        )
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 0
        assert any("description" in w for w in reg.warnings)

    def test_reload_returns_count_and_warnings(self, tmp_path: Path) -> None:
        _write_plugin(tmp_path, "bad.py", "VALUE = 1\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        count, warnings = reg.reload()
        assert count == 0
        assert warnings
