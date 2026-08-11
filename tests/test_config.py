"""Tests for config.json validation and ChatState override mapping."""

from __future__ import annotations

import json

import pytest

from r105.config import _validate_config, load_state_overrides
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
