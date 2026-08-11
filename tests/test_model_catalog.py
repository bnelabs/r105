"""Tests for the model capability catalog (context-window resolution)."""

from __future__ import annotations

import pytest

from r105.model_catalog import (
    _catalog_context,
    _extract_context_from_model_entry,
    normalize_model_name,
    resolve_context_tokens,
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
