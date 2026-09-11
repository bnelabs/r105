"""Tests for provider presets and guided connection helpers."""

from __future__ import annotations

from r105.connections import (
    CONNECTION_PRESETS,
    get_connection_preset,
    provider_options,
    resolve_api_key,
    valid_connection_url,
)
from r105.tui.screens.connection_screen import extract_model_ids


def test_catalog_starts_with_opencode_connections() -> None:
    assert [preset.id for preset in CONNECTION_PRESETS[:2]] == ["opencode", "opencode-go"]
    assert provider_options()[0][1] == "opencode"


def test_provider_aliases_and_key_resolution(monkeypatch) -> None:
    preset = get_connection_preset("opencode-zen")
    assert preset is not None
    assert preset.id == "opencode"

    monkeypatch.delenv("OPENCODE_API_KEY", raising=False)
    assert resolve_api_key(preset) == ""
    assert resolve_api_key(preset, "  entered-key  ") == "entered-key"

    monkeypatch.setenv("OPENCODE_API_KEY", "environment-key")
    assert resolve_api_key(preset) == "environment-key"

    monkeypatch.setenv("OPENAI_API_KEY", "unrelated-key")
    monkeypatch.delenv("GROQ_API_KEY", raising=False)
    groq = get_connection_preset("groq")
    assert groq is not None
    assert resolve_api_key(groq) == ""


def test_explicit_session_key_reaches_backend_headers() -> None:
    from r105.client import DirectClient

    client = DirectClient(base_url="https://provider.example/v1", api_key="session-key")
    assert client._headers()["Authorization"] == "Bearer session-key"


def test_model_list_shapes_are_normalized() -> None:
    assert extract_model_ids({"data": [{"id": "a"}, {"name": "b"}, {"id": "a"}]}) == [
        "a",
        "b",
    ]
    assert extract_model_ids({"models": {"local": {}, "named": {"name": "display"}}}) == [
        "local",
        "named",
    ]
    assert extract_model_ids({"models": ["one", {"model": "two"}]}) == ["one", "two"]


def test_connection_url_rejects_embedded_credentials() -> None:
    assert valid_connection_url("https://api.example/v1")
    assert not valid_connection_url("https://user:pass@api.example/v1")
    assert not valid_connection_url("ftp://api.example/v1")
