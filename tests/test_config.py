"""Tests for config.json validation and ChatState override mapping."""

from __future__ import annotations

import json

import pytest

from r105.config import (
    _validate_config,
    apply_config_to_state,
    config_schema,
    ensure_config,
    load_state_overrides,
)
from r105.state import ChatState


class TestModelFamiliesValidation:
    """The ``model_families`` key maps name fragments to family names or null."""

    def test_valid_mapping_accepted(self) -> None:
        _validate_config(
            {"model_families": {"my-gemma4-finetune": "gemma-4", "opaque-model": None}}
        )

    def test_non_dict_rejected(self) -> None:
        with pytest.raises(ValueError, match="model_families must be an object"):
            _validate_config({"model_families": ["gemma-4"]})

    def test_empty_fragment_rejected(self) -> None:
        with pytest.raises(ValueError, match="keys must be non-empty strings"):
            _validate_config({"model_families": {"": "gemma-4"}})

    def test_non_string_family_rejected(self) -> None:
        with pytest.raises(ValueError, match="family name string or null"):
            _validate_config({"model_families": {"m": 42}})

    def test_unknown_keys_still_rejected(self) -> None:
        with pytest.raises(ValueError, match="Unknown config key"):
            _validate_config({"model_familiez": {}})

    def test_cache_prompt_must_be_boolean(self) -> None:
        with pytest.raises(ValueError, match="cache_prompt must be true or false"):
            _validate_config({"cache_prompt": "yes"})

    def test_schema_is_closed_and_contains_cache_prompt(self) -> None:
        schema = config_schema()
        assert schema["additionalProperties"] is False
        assert schema["properties"]["cache_prompt"] == {
            "type": "boolean",
            "default": False,
        }

    def test_keybindings_validate_known_ids(self) -> None:
        _validate_config({"keybindings": {"show_tools": "ctrl+o"}})
        with pytest.raises(ValueError, match="Unknown keybinding ID"):
            _validate_config({"keybindings": {"show_toolz": "ctrl+o"}})
        with pytest.raises(ValueError, match="non-empty key string"):
            _validate_config({"keybindings": {"show_tools": ""}})

    def test_apply_config_to_state_reports_changed_fields(self) -> None:
        state = ChatState(model="old-model", theme="dracula")
        changed = apply_config_to_state(
            state,
            {
                "theme": "solarized-dark",
                "model": "new-model",
                "cache_prompt": True,
                "keybindings": {"show_tools": "ctrl+o"},
            },
        )
        assert {"theme", "model", "cache_prompt", "keybindings"} <= changed
        assert state.theme == "solarized-dark"
        assert state.model == "new-model"
        assert state.cache_prompt is True
        assert state.keybindings == {"show_tools": "ctrl+o"}


class TestLoadStateOverrides:
    """``model_families`` flows from config.json into ChatState."""

    def test_model_families_loaded_into_state(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_dir = tmp_path / "r105-config"
        config_path = config_dir / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(
            json.dumps({"model_families": {"finetune": "gemma-4", "opaque": None}}),
            encoding="utf-8",
        )

        overrides = load_state_overrides()
        state = ChatState(**overrides)
        assert state.model_families == {"finetune": "gemma-4", "opaque": None}

    def test_no_model_families_keeps_default(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_dir = tmp_path / "r105-config"
        config_path = config_dir / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps({"theme": "dracula"}), encoding="utf-8")

        overrides = load_state_overrides()
        state = ChatState(**overrides)
        assert state.model_families == {}

    def test_cache_prompt_flows_into_state(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_dir = tmp_path / "r105-config"
        config_path = config_dir / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps({"cache_prompt": True}), encoding="utf-8")

        assert load_state_overrides()["cache_prompt"] is True

    def test_keybindings_flow_into_state(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_dir = tmp_path / "r105-config"
        config_path = config_dir / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path.write_text(
            json.dumps({"keybindings": {"show_tools": "ctrl+o"}}),
            encoding="utf-8",
        )

        assert load_state_overrides()["keybindings"] == {"show_tools": "ctrl+o"}

    def test_strict_mode_surfaces_invalid_config(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_dir = tmp_path / "r105-config"
        config_path = config_dir / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_DIR", config_dir)
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_dir.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps({"typoed_key": True}), encoding="utf-8")
        monkeypatch.setenv("R105_STRICT_CONFIG", "1")

        with pytest.raises(ValueError, match="Unknown config key"):
            ensure_config()

    def test_explicit_strict_read_ignores_environment_default(self, tmp_path, monkeypatch) -> None:
        from r105 import config as r105_config

        config_path = tmp_path / "r105-config" / "config.json"
        monkeypatch.setattr(r105_config, "CONFIG_PATH", config_path)
        config_path.parent.mkdir(parents=True, exist_ok=True)
        config_path.write_text(json.dumps({"unknown": True}), encoding="utf-8")
        monkeypatch.delenv("R105_STRICT_CONFIG", raising=False)

        with pytest.raises(ValueError, match="Unknown config key"):
            ensure_config(strict=True)
