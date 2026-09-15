# 0015: Ghost-text completion sidecar

- Status: landed
- Author: r105
- Scope: `src/ghost.rs` (new), `src/config.rs`, `src/ui/input.rs`, `src/ui/render.rs`, `src/ui/mod.rs`

## Problem

Completion is deterministic only (palette fuzzy + recency, file/arg
menus). No neural assist exists for shell one-liners or code drafts,
and embedding inference in-binary would bloat the build and the
platform matrix.

## Proposal

- New `src/ghost.rs`: `GhostClient` POSTs `{endpoint}/completion` with
  a Qwen FIM prompt (`<|fim_prefix|>{prefix}<|fim_suffix|><|fim_middle|>`,
  empty suffix in v1), `n_predict<=24`, low temperature, `cache_prompt`,
  1500ms timeout. Pure `fim_prompt` / `clean_completion` (first line,
  strip echo, reject empty/identical) unit-tested.
- Trigger v1: input starts with `!` or `/sh `, length ≥ 3, 250ms
  debounce on the 45ms tick, generation counter cancels stale requests,
  max one in flight, all failures silent (deterministic completion is
  the fallback).
- Render dimmed suffix after the cursor; `Tab` accepts when ghost is
  visible (precedence over mode cycle), `Esc` dismisses. Sidecar:
  lazy-spawn `llama-server -m <model> --port 11438 -c 2048`, health
  check, kill on exit. No auto-download: `/completion` reports status
  and prints the one-line `curl` fetch when weights are missing.
- Config: `completion_enabled` (default true, inert without sidecar),
  `completion_endpoint`, `completion_model_path` (default
  `<config>/models/qwen2.5-coder-0.5b-q8_0.gguf`), timeout/debounce ms.
  Default model: `ggml-org/Qwen2.5-Coder-0.5B-Q8_0-GGUF` (base, Q8_0).

## Non-goals

In-binary inference; auto-downloading weights; suffix context;
editor-file ghost text; GPU requirements (CPU + Metal both fine).

## Acceptance

- `ghost_fim_prompt_shape`, `ghost_clean_rejects_echo`,
  `ghost_stale_generation_dropped`, `ghost_tab_accepts_esc_dismisses`,
  live: `llama-server` + Q8_0 first-token p50 < 500ms warm on CPU/Metal.

## Amendments

- 2026-09-15 (live): `llama-server` (brew 0.4.0) + Q8_0 on Apple M1:
  p50 ≈ 350ms end-to-end for 24 tokens (prompt eval ≈ 29ms warm),
  RSS ≈ 578MB. Quality: `docker ps --format` → exact Go template;
  `git checkout -b feat/trust-` → `new-feature`; short prefixes
  (`git sta` → `rt`) stay weak — debounce + longer prefixes mitigate.
  Verdict: Q8_0 keeps its default; Q4_K_M stays a one-line config swap.
