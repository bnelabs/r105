# 0011: Drop the Python legacy (bridge, `execute_python`, migration doc)

- Status: landed
- Author: r105
- Scope: `src/python_bridge.rs`, `bridge/`, `build/`,
  `docs/RUST_MIGRATION.md`, `execute_python` tool, `/approve`, `/bridge`
  command, `r105 bridge` subcommand, `--yes` python half, config keys,
  README/`TOOLS.md`/CI references

## Problem

r105 is a native Rust binary, but it still carries the full Python
compatibility layer: an `execute_python` tool, an external-bridge
protocol module, a reference bridge script, two slash commands, a CLI
subcommand, two config keys, and migration docs. The layer widens the
tool surface the model sees and the config surface operators maintain,
for a legacy nobody should adopt now.

## Proposal

Delete, don't deprecate:

- Delete `src/python_bridge.rs`, `bridge/`, on-disk `build/`,
  `docs/RUST_MIGRATION.md`.
- Remove the `execute_python` tool definition, dispatch arm, and
  function; `ToolContext` loses `python_bridge_command`/`python_approved`.
- Remove `/approve` and `/bridge` commands, the `r105 bridge`
  subcommand, and `python_bridge_status`; `/state` drops the bridge line.
- Remove `python_bridge_command` / `auto_approve_execute_python`
  config keys, schema entries, and tests (old configs warn as unknown
  keys, per existing behavior).
- `--yes` keeps selecting full-access; it no longer approves anything.
- Remove `Sandbox::run_with_input` (sole caller was the bridge).
- Scrub README, `docs/TOOLS.md`, CI, `.gitignore`, and the README
  source tree; record under CHANGELOG `Unreleased` → `Removed`.

Kept deliberately: the `#`-classify `python`/`python3` binary names
(shell detection, unrelated to the bridge), the MCP "no Python
dependency" note (still true), and CHANGELOG/release-notes history.

## Non-goals

No replacement code-execution tool; `execute_rust` covers sandboxed
execution. No config migration beyond the existing unknown-key warning.

## Acceptance

- `grep -ri python|bridge docs README src` shows only the kept items.
- `cargo test --locked`, `clippy --all-targets -- -D warnings`,
  `cargo fmt --check` clean.
