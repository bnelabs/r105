# 0010: Split `src/ui.rs` into `src/ui/` modules

- Status: landed
- Author: r105
- Scope: `src/ui.rs` → `src/ui/{mod,input,commands,complete,transcript,render}.rs`

## Problem

`src/ui.rs` is 5,386 lines: a 91-field `UiApp` god-object with 113
methods plus ~45 free helpers and the test module in one file. Every
feature batch makes navigation, review, and parallel work harder.

## Proposal

Mechanical, namespace-preserving split — no behavior change:

- `mod.rs`: `UiApp` struct (fields become `pub(crate)`), lifecycle
  (`new`, `event_loop`, `process_events`, chat start/cancel, refresh),
  `run`, `MAX_TOOL_ROUNDS`, supporting types, `mod tests` unchanged.
- `input.rs`: key/mouse handling, composer editing, submit paths
  (including `#` classify and Tab-accept), key-spec helpers.
- `commands.rs`: `handle_command`, all `command_*` handlers,
  provider/model/health/profile starters, settings rows, workspace and
  config helpers.
- `complete.rs`: palette, `@` file menu, first-arg value menu,
  custom-command lookup, fuzzy/file scoring helpers.
- `transcript.rs`: notices, status tone, section state, prune/reseed,
  `/expand`, text helpers (`split_compact`, `split_repeat_suffix`, …).
- `render.rs`: all `draw_*`, theme palette, picker/centering helpers.
- `mod.rs` re-exports each module (`pub(crate) use …::*`) and every
  child opens with `use super::*`, so the merged namespace — and every
  name resolution — is identical to the single file.

## Non-goals

No dispatch table, no test moves, no logic edits. Follow-up refactors
build on the modules; they are not this change.

## Acceptance

- `cargo test --locked` green (88 tests, incl. render-to-lines frames).
- `cargo clippy --locked --all-targets -- -D warnings` clean.
- `cargo fmt --check` clean; `crate::ui::run` the only external path.
