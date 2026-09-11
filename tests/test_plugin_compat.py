"""Plugin host-compatibility metadata and reload-drain behavior."""

from __future__ import annotations

import threading
import time
from pathlib import Path

import pytest

from r105 import __version__ as host_version
from r105.errors import PluginError
from r105.plugins import (
    PluginRegistry,
    _parse_version,
    check_plugin_compatibility,
)


def _write(tmp_path: Path, name: str, body: str) -> Path:
    path = tmp_path / name
    path.write_text(body, encoding="utf-8")
    return path


def _ns(**attrs):
    return type("Module", (), attrs)()


class TestParseVersion:
    def test_dotted_versions_compare(self):
        assert _parse_version("0.6.0", what="v", source="t") == (0, 6, 0)
        assert _parse_version("1.10", what="v", source="t") > _parse_version("1.9.9", what="v", source="t")

    def test_garbage_rejected(self):
        with pytest.raises(PluginError, match="invalid"):
            _parse_version("soon", what="__r105_min_version__", source="p.py")


class TestCompatibility:
    def test_no_metadata_passes(self):
        check_plugin_compatibility(_ns(), "p.py")

    def test_current_host_version_passes(self):
        check_plugin_compatibility(_ns(__r105_min_version__=host_version), "p.py")

    def test_future_host_version_rejected(self):
        with pytest.raises(PluginError, match="needs r105"):
            check_plugin_compatibility(_ns(__r105_min_version__="999.0.0"), "p.py")

    def test_stdlib_requirement_passes(self):
        check_plugin_compatibility(_ns(PLUGIN_REQUIREMENTS=["json", "os"]), "p.py")

    def test_missing_requirement_rejected(self):
        with pytest.raises(PluginError, match=r"missing package.*no_such_mod_xyz"):
            check_plugin_compatibility(_ns(PLUGIN_REQUIREMENTS=["no_such_mod_xyz_abc"]), "p.py")

    def test_malformed_requirements_rejected(self):
        with pytest.raises(PluginError, match="must be a list"):
            check_plugin_compatibility(_ns(PLUGIN_REQUIREMENTS="json"), "p.py")


class TestLoadEnforcement:
    def test_incompatible_plugin_skipped_with_warning(self, tmp_path: Path):
        _write(
            tmp_path,
            "future.py",
            '__r105_min_version__ = "999.0.0"\n'
            "def register(registry):\n"
            "    registry.add_tool(name='f', description='f', parameters={})\n",
        )
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 0
        assert any("needs r105" in w for w in reg.warnings)

    def test_compatible_metadata_loads(self, tmp_path: Path):
        _write(
            tmp_path,
            "ok.py",
            f'__r105_min_version__ = "{host_version}"\n'
            "PLUGIN_REQUIREMENTS = ['json']\n"
            "def register(registry):\n"
            "    registry.add_tool(name='ok', description='ok', parameters={})\n",
        )
        reg = PluginRegistry(plugins_dir=tmp_path)
        assert reg.discover() == 1
        assert reg.get_tool("ok") is not None


class TestReloadDrain:
    def test_reload_waits_for_inflight_call(self, tmp_path: Path):
        started = threading.Event()
        release = threading.Event()

        def slow_handler(args, ws):
            started.set()
            assert release.wait(timeout=10)
            return "slow-done"

        _write(tmp_path, "slow.py", "def register(registry):\n    registry.add_tool(name='slow', description='s', parameters={})\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        reg.discover()
        # Swap in a blocking handler to simulate a long tool run.
        reg.get_tool("slow").handler = slow_handler  # type: ignore[union-attr]

        result: dict[str, str | None] = {}
        worker = threading.Thread(
            target=lambda: result.setdefault("out", reg.execute("slow", {}, tmp_path))
        )
        worker.start()
        assert started.wait(timeout=10)

        reloaded: dict[str, object] = {}
        reloader = threading.Thread(
            target=lambda: reloaded.setdefault("ret", reg.reload(drain_timeout=10.0))
        )
        reloader.start()
        time.sleep(0.2)
        assert reloader.is_alive()  # blocked draining the slow call
        release.set()
        worker.join(timeout=10)
        reloader.join(timeout=10)
        assert result["out"] == "slow-done"
        count, warnings = reloaded["ret"]  # type: ignore[misc]
        assert count == 1 and not warnings

    def test_reload_timeout_warns_but_proceeds(self, tmp_path: Path):
        started = threading.Event()
        release = threading.Event()

        def stuck_handler(args, ws):
            started.set()
            release.wait(timeout=10)
            return "late"

        _write(tmp_path, "s.py", "def register(registry):\n    registry.add_tool(name='s', description='s', parameters={})\n")
        reg = PluginRegistry(plugins_dir=tmp_path)
        reg.discover()
        reg.get_tool("s").handler = stuck_handler  # type: ignore[union-attr]
        worker = threading.Thread(target=lambda: reg.execute("s", {}, tmp_path))
        worker.start()
        assert started.wait(timeout=10)
        try:
            count, warnings = reg.reload(drain_timeout=0.1)
            assert count == 1
            assert any("still running" in w for w in warnings)
        finally:
            release.set()
            worker.join(timeout=10)
