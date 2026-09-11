"""Confirmation gate, rate limiting, and doctor command tests."""

import json
from pathlib import Path

from r105.commands import handle_slash_command
from r105.doctor import DoctorCheck, DoctorReport, run_doctor
from r105.state import ChatState
from r105.tools import (
    _WEB_RATE_LIMITS,
    approve_execute_python,
    execute_python_needs_approval,
    execute_tool_call,
    reset_execute_python_approval,
    reset_rate_limits,
    set_execute_python_auto_approve,
)


def _py_call(code: str) -> dict:
    return {
        "id": "call_gate_1",
        "function": {"name": "execute_python", "arguments": json.dumps({"code": code})},
    }


class TestConfirmationGate:
    def setup_method(self):
        reset_execute_python_approval()
        reset_rate_limits()

    def teardown_method(self):
        reset_execute_python_approval()
        reset_rate_limits()

    def test_gated_by_default(self, tmp_path: Path):
        assert execute_python_needs_approval() is True
        result = execute_tool_call(_py_call("print(1)"), tmp_path)
        assert "one-time approval" in result["content"]

    def test_approve_unblocks(self, tmp_path: Path):
        approve_execute_python()
        assert execute_python_needs_approval() is False
        result = execute_tool_call(_py_call("print(42)"), tmp_path)
        # Gate passed (execution itself may fail on platforms without a
        # sandbox backend — what matters here is no approval block).
        assert "one-time approval" not in result["content"]

    def test_auto_approve_bypasses(self, tmp_path: Path):
        set_execute_python_auto_approve(True)
        result = execute_tool_call(_py_call("print(7)"), tmp_path)
        assert "one-time approval" not in result["content"]

    def test_gate_message_guides_user(self, tmp_path: Path):
        result = execute_tool_call(_py_call("print(1)"), tmp_path)
        assert "/approve execute_python" in result["content"]
        assert "--yes" in result["content"]


class TestApproveCommand:
    def test_approve_usage(self):
        import asyncio

        state = ChatState(skills_dir=Path("skills"))
        out = asyncio.run(handle_slash_command("/approve", state))
        assert "usage" in out

    def test_approve_python(self):
        import asyncio

        reset_execute_python_approval()
        try:
            state = ChatState(skills_dir=Path("skills"))
            out = asyncio.run(handle_slash_command("/approve execute_python", state))
            assert "approved" in out
            assert execute_python_needs_approval() is False
        finally:
            reset_execute_python_approval()


def _web_call(name: str, args: dict) -> dict:
    return {
        "id": "call_rl_1",
        "function": {"name": name, "arguments": json.dumps(args)},
    }


class TestRateLimit:
    def setup_method(self):
        reset_rate_limits()

    def teardown_method(self):
        reset_rate_limits()

    def test_exhaustion_returns_error(self, monkeypatch):
        import r105.tools as tools_mod

        monkeypatch.setitem(tools_mod._WEB_RATE_LIMITS, "web_search", (2, 60.0))
        # Prime the budget with direct bookkeeping (no network).
        from r105.tools import _check_rate_limit

        assert _check_rate_limit("web_search") is None
        assert _check_rate_limit("web_search") is None
        err = _check_rate_limit("web_search")
        assert err is not None and "rate limited" in err

    def test_unknown_tool_unlimited(self):
        from r105.tools import _check_rate_limit

        assert _check_rate_limit("get_time") is None

    def test_reset_clears_budget(self, monkeypatch):
        import r105.tools as tools_mod
        from r105.tools import _check_rate_limit

        monkeypatch.setitem(tools_mod._WEB_RATE_LIMITS, "web_search", (1, 60.0))
        assert _check_rate_limit("web_search") is None
        assert _check_rate_limit("web_search") is not None
        reset_rate_limits()
        assert _check_rate_limit("web_search") is None

    def test_defaults_documented(self):
        assert _WEB_RATE_LIMITS["web_search"][0] >= 1
        assert _WEB_RATE_LIMITS["web_fetch"][0] >= 1


class TestDoctorReport:
    def _probes(self, **overrides):
        base = {
            "version": "0.6.0",
            "config": {},
            "config_error": None,
            "config_path": "/cfg/config.json",
            "sandbox_backends": [("rlimit", True, "")],
            "selected_backend": "rlimit",
            "fallback_reason": None,
            "backend_health": {"ok": True, "models_available": 3},
            "backend_error": None,
            "backend_url": "http://127.0.0.1:8010",
            "workspace_dir": Path("/tmp/ws"),
            "workspace_writable": True,
            "skills_dir": Path("/tmp/sk"),
            "skills_count": 2,
            "api_keys": ["OPENAI_API_KEY"],
        }
        base.update(overrides)
        return base

    def test_all_green(self):
        report = run_doctor(**self._probes())
        assert report.passed is True
        assert "all checks passed" in report.render()

    def test_config_error_fails(self):
        report = run_doctor(**self._probes(config=None, config_error="boom"))
        assert report.passed is False
        assert "boom" in report.render()

    def test_fallback_warns(self):
        report = run_doctor(**self._probes(fallback_reason="using none (no backend)"))
        assert report.passed is False

    def test_backend_down_fails(self):
        report = run_doctor(**self._probes(
            backend_health=None, backend_error="connection refused",
        ))
        assert report.passed is False
        assert "connection refused" in report.render()

    def test_unwritable_workspace_fails(self):
        report = run_doctor(**self._probes(workspace_writable=False))
        assert report.passed is False

    def test_check_dataclass(self):
        assert DoctorCheck("x", True, "d").ok is True
        assert DoctorReport([DoctorCheck("x", False)]).passed is False
