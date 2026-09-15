# 0003: Status tone and consecutive-notice dedup

- Status: landed
- Author: r105
- Scope: `src/ui.rs` (status field, footer, `push_system`)

## Problem

The status line is an untyped string: errors render exactly like idle
notes, so failures are easy to miss. Repeated identical transcript
notices (reconnect loops, fallback warnings) each append a full line.

## Proposal

- `StatusTone { Muted, Success, Error }` plus a `status_tone` field
  (default `Muted`). Helpers `set_status` / `set_ok` / `set_error`
  replace every direct `self.status = …` assignment; the footer renders
  the status span white / green / red by tone.
- `push_system` folds a note identical to the last system message into a
  ` (×N)` suffix instead of appending a duplicate line.

## Non-goals

No auto-expiry timers, no notice queue or stacking, no tone on
transcript lines.

## Acceptance

- `status_error_renders_red`
- `push_system_folds_consecutive_duplicates`
- Full suite plus `cargo clippy -- -D warnings` clean.

## Amendments

(None yet.)
