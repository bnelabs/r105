# 0001: Render-to-lines test harness

- Status: landed
- Author: r105
- Scope: `src/ui.rs` tests module only

## Problem

UI behavior is verified through unit tests of helpers (`ensure_visible`,
`clean_shell_draft`), but no test ever renders a frame. Palette, status,
and transcript regressions are invisible until manual runs.

## Proposal

Add a `render_lines(app, width, height) -> Vec<String>` helper to the
`ui.rs` tests module backed by `ratatui::backend::TestBackend`: draw one
frame, then collect each buffer row into a `String` with trailing
whitespace trimmed. Tests mutate `UiApp` state directly (input text,
status, transcript) and assert on the joined rows. Cell styles stay
reachable via the terminal object for the rare color assertion.

## Non-goals

No golden screenshot files, no simulated key-event runs, no harness
outside `ui.rs` tests.

## Acceptance

- `harness_renders_status_and_composer_text`
- `harness_renders_palette_rows_for_slash_query`

## Amendments

(None yet.)
