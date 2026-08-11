"""Tests for the model capability catalog (context-window resolution)."""

from __future__ import annotations

import pytest

from r105.model_catalog import (
    _catalog_context,
    _extract_context_from_model_entry,
    model_family,
    normalize_model_name,
    resolve_context_tokens,
    uses_gemma4_channel_syntax,
)
from r105.state import DEFAULT_CONTEXT_TOKENS


class TestNormalizeModelName:
    def test_strips_gguf_and_quant_suffix(self) -> None:
        assert normalize_model_name("muse-glimmer-30B-kquant-17gb.gguf") == "muse-glimmer-30b"

    def test_strips_ud_and_instruct(self) -> None:
        assert normalize_model_name("gemma-4-12b-it-UD-Q8_K_XL") == "gemma-4-12b"

    def test_strips_long_variant_suffix(self) -> None:
        name = "Qwen3.6-35B-A3B-uncensored-heretic-Native-MTP-Preserved-APEX-I-Quality.gguf"
        assert normalize_model_name(name) == "qwen3.6-35b"

    def test_empty(self) -> None:
        assert normalize_model_name("") == ""
        assert normalize_model_name(None) == ""


class TestCatalogContext:
    def test_muse_glimmer(self) -> None:
        assert _catalog_context("muse-glimmer-30B") == 131072

    def test_gemma_4_12b(self) -> None:
        assert _catalog_context("gemma-4-12b-it-UD-Q8_K_XL.gguf") == 131072

    def test_specific_beats_generic(self) -> None:
        # "gemma-4-12b" (longer) must win over "gemma-4" / "gemma"
        assert _catalog_context("gemma-4-12b") == 131072
        assert _catalog_context("gemma4-v2") == 131072

    def test_unknown_returns_none(self) -> None:
        assert _catalog_context("totally-unknown-model") is None


class TestResolvePriority:
    def test_config_override_wins(self) -> None:
        result = resolve_context_tokens(
            "muse-glimmer-30B",
            config_contexts={"muse-glimmer": 65536},
            backend_context=131072,
        )
        assert result == 65536

    def test_config_fragment_longest_match(self) -> None:
        result = resolve_context_tokens(
            "gemma-4-12b-it",
            config_contexts={"gemma-4-12b": 32768, "gemma": 8192},
        )
        assert result == 32768

    def test_global_override_beats_backend(self) -> None:
        result = resolve_context_tokens("x", global_override=99999, backend_context=131072)
        assert result == 99999

    def test_backend_probe_beats_catalog(self) -> None:
        result = resolve_context_tokens("muse-glimmer-30B", backend_context=16384)
        assert result == 16384

    def test_catalog_fallback(self) -> None:
        assert resolve_context_tokens("muse-glimmer-30B") == 131072

    def test_default_fallback(self) -> None:
        assert resolve_context_tokens("unknown-xyz") == DEFAULT_CONTEXT_TOKENS

    def test_invalid_config_values_ignored(self) -> None:
        result = resolve_context_tokens(
            "muse-glimmer-30B",
            config_contexts={"muse-glimmer": "not-a-number", "other": -5},
        )
        assert result == 131072  # falls through to catalog


class TestExtractContextFromModelEntry:
    def test_llama_cpp_meta_n_ctx(self) -> None:
        entry = {"id": "m", "meta": {"n_ctx": 131072, "n_ctx_train": 131072}}
        assert _extract_context_from_model_entry(entry) == 131072

    def test_openai_context_window(self) -> None:
        entry = {"id": "m", "context_window": 128000}
        assert _extract_context_from_model_entry(entry) == 128000

    def test_ollama_context_length(self) -> None:
        entry = {"id": "m", "context_length": 65536}
        assert _extract_context_from_model_entry(entry) == 65536

    def test_vllm_max_model_len(self) -> None:
        entry = {"id": "m", "max_model_len": 32768}
        assert _extract_context_from_model_entry(entry) == 32768

    def test_missing_returns_none(self) -> None:
        assert _extract_context_from_model_entry({"id": "m"}) is None
        assert _extract_context_from_model_entry({}) is None

    def test_invalid_values_ignored(self) -> None:
        entry = {"id": "m", "context_window": "huge", "meta": {"n_ctx": 0}}
        assert _extract_context_from_model_entry(entry) is None

    def test_negative_ignored(self) -> None:
        entry = {"id": "m", "max_model_len": -1}
        assert _extract_context_from_model_entry(entry) is None


class TestModelFamily:
    """Model-family detection — the gate for model-specific behavior."""

    def test_gemma4_detected(self) -> None:
        assert model_family("gemma-4-12b-it-UD-Q8_K_XL") == "gemma-4"
        assert model_family("gemma4-v2-Q8_0.gguf") == "gemma-4"
        assert model_family("gemma-4-31b-q4_0-it-fixed.gguf") == "gemma-4"

    def test_glimmer_is_gemma4_family(self) -> None:
        # muse-glimmer uses Gemma-4-style chat templates
        assert model_family("muse-glimmer-30B-kquant-17gb.gguf") == "gemma-4"

    def test_other_families(self) -> None:
        assert model_family("qwen3.6-35B") == "qwen"
        assert model_family("deepseek-v3.1") == "deepseek"
        assert model_family("llama-3.1-8b") == "llama"
        assert model_family("gpt-4o") == "gpt"

    def test_unknown_returns_none(self) -> None:
        assert model_family("totally-unknown-model") is None
        assert model_family("") is None


class TestGemma4ChannelSyntaxGate:
    """Only Gemma-4-family models may get channel-syntax handling."""

    def test_gemma4_models_opt_in(self) -> None:
        assert uses_gemma4_channel_syntax("gemma-4-12b-it") is True
        assert uses_gemma4_channel_syntax("gemma4-v2") is True
        assert uses_gemma4_channel_syntax("muse-glimmer-30B") is True

    def test_all_other_models_are_opaque(self) -> None:
        for name in (
            "qwen3.6-35B",
            "qwen2.5",
            "deepseek-v3",
            "llama-3.1",
            "mistral",
            "gpt-4o",
            "gpt-4.1",
            "claude-sonnet",
            "glm-5",
            "phi-4",
            "totally-unknown-model",
            "",
        ):
            assert uses_gemma4_channel_syntax(name) is False, f"{name!r} must be opaque"


class TestConfigFamilyOverrides:
    """``model_families`` config overrides family classification.

    Keys are model-name fragments (longest match wins); values are family
    names (opt-in to a family's handling) or None (force opaque passthrough).
    Overrides take precedence over the built-in catalog.
    """

    def test_override_opts_custom_finetune_into_gemma4(self) -> None:
        overrides = {"my-gemma4-finetune": "gemma-4"}
        assert model_family("my-gemma4-finetune-v2", config_families=overrides) == "gemma-4"
        assert uses_gemma4_channel_syntax("my-gemma4-finetune-v2", overrides) is True

    def test_override_reclassifies_catalog_model(self) -> None:
        overrides = {"gemma-4-12b": "qwen"}
        assert model_family("gemma-4-12b-it", config_families=overrides) == "qwen"
        assert uses_gemma4_channel_syntax("gemma-4-12b-it", overrides) is False

    def test_null_override_forces_opaque(self) -> None:
        overrides = {"gemma-4-12b": None}
        assert model_family("gemma-4-12b-it", config_families=overrides) is None
        assert uses_gemma4_channel_syntax("gemma-4-12b-it", overrides) is False

    def test_longest_override_fragment_wins(self) -> None:
        overrides = {"gemma": "qwen", "gemma-4-r1": "gemma-4"}
        assert model_family("gemma-4-r1-7b", config_families=overrides) == "gemma-4"

    def test_unrelated_models_unaffected(self) -> None:
        overrides = {"gemma-4": "gemma-4"}
        assert model_family("qwen3.6-35B", config_families=overrides) == "qwen"
        assert uses_gemma4_channel_syntax("qwen3.6-35B", overrides) is False

    def test_no_overrides_leaves_builtin_behavior(self) -> None:
        assert uses_gemma4_channel_syntax("gemma-4-12b-it") is True
        assert model_family("gemma-4-12b-it", config_families={}) == "gemma-4"
