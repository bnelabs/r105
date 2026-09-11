"""Tests for sandbox fallback transparency (reason API + weak warnings)."""

from __future__ import annotations

import pytest

from r105 import sandbox
from r105.sandbox import (
    BwrapSandbox,
    DockerSandbox,
    NoopSandbox,
    NsjailSandbox,
    RLimitSandbox,
    current_backend_name,
    detect_backend,
    detect_backend_with_reason,
    get_fallback_reason,
    set_sandbox,
    weak_backend_warning,
)


@pytest.fixture(autouse=True)
def _restore_globals():
    """Snapshot/restore module globals so tests don't leak backend state."""
    old_sandbox = sandbox._sandbox
    old_reason = sandbox._fallback_reason
    yield
    sandbox._sandbox = old_sandbox
    sandbox._fallback_reason = old_reason


def _all_unavailable(monkeypatch) -> None:
    for cls in (NsjailSandbox, BwrapSandbox, DockerSandbox, RLimitSandbox):
        monkeypatch.setattr(cls, "is_available", staticmethod(lambda: False))


def test_strongest_backend_has_no_reason(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    monkeypatch.setattr(NsjailSandbox, "is_available", staticmethod(lambda: True))
    backend, reason = detect_backend_with_reason()
    assert isinstance(backend, NsjailSandbox)
    assert reason is None


def test_fallback_reason_names_skipped_backends(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    monkeypatch.setattr(DockerSandbox, "is_available", staticmethod(lambda: True))
    backend, reason = detect_backend_with_reason()
    assert isinstance(backend, DockerSandbox)
    assert reason is not None
    assert "nsjail" in reason
    assert "bwrap" in reason


def test_weak_backend_reason_warns_about_isolation(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    monkeypatch.setattr(RLimitSandbox, "is_available", staticmethod(lambda: True))
    backend, reason = detect_backend_with_reason()
    assert isinstance(backend, RLimitSandbox)
    assert reason is not None
    assert "WARNING" in reason
    assert "no filesystem/network isolation" in reason


def test_no_backend_falls_back_to_none(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    backend, reason = detect_backend_with_reason()
    assert isinstance(backend, NoopSandbox)
    assert reason is not None
    assert "WARNING" in reason


def test_detect_backend_stores_reason(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    backend = detect_backend()
    assert isinstance(backend, NoopSandbox)
    assert get_fallback_reason() is not None


def test_explicit_set_sandbox_clears_reason(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    detect_backend()
    assert get_fallback_reason() is not None
    # Explicitly selecting "none" is an operator choice, not a fallback.
    assert set_sandbox("none") is not None
    assert get_fallback_reason() is None


def test_weak_backend_warning_only_for_weak(monkeypatch) -> None:
    _all_unavailable(monkeypatch)
    monkeypatch.setattr(NsjailSandbox, "is_available", staticmethod(lambda: True))
    sandbox._sandbox = NsjailSandbox()
    assert weak_backend_warning() is None
    assert current_backend_name() == "nsjail"

    sandbox._sandbox = RLimitSandbox()
    warning = weak_backend_warning()
    assert warning is not None
    assert "rlimit" in warning


def test_current_backend_name_none_when_unselected() -> None:
    sandbox._sandbox = None
    assert current_backend_name() is None
    assert weak_backend_warning() is None
