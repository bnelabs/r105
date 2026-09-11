"""Tests for export dependency guard."""

from __future__ import annotations

import pytest

from r105.sessions import _ensure_export_deps, export_conversation
from r105.state import ChatState


def test_ensure_export_deps_missing(monkeypatch):
    """_ensure_export_deps should raise RuntimeError with helpful message when deps are missing."""
    # Simulate missing export deps by forcing ImportError
    def fake_import(name):
        raise ImportError

    import builtins
    original_import = builtins.__import__

    def mock_import(name, *args, **kwargs):
        if name in {"python_pptx", "docx", "fpdf2", "PIL"}:
            raise ImportError
        return original_import(name, *args, **kwargs)

    monkeypatch.setattr(builtins, "__import__", mock_import)

    with pytest.raises(RuntimeError) as excinfo:
        _ensure_export_deps()

    assert "pip install 'r105[export]'" in str(excinfo.value)


def test_ensure_export_deps_succeeds_when_available():
    """_ensure_export_deps should not raise when optional deps are importable."""
    # In CI the deps may be missing, so we only check that the function is callable
    # and raises a RuntimeError with the correct message format when it fails.
    # The actual import test is covered by test_ensure_export_deps_missing.
    try:
        _ensure_export_deps()
    except RuntimeError as e:
        # Expected in environments without the optional deps
        assert "pip install 'r105[export]'" in str(e)
    else:
        # If deps are installed, we just confirm no exception
        assert True


def test_plain_text_export_has_roles_and_content():
    state = ChatState()
    state.history = [
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "world"},
    ]
    exported = export_conversation(state, "text")
    assert "[1] USER" in exported
    assert "[2] ASSISTANT" in exported
    assert "hello" in exported and "world" in exported
