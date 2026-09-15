# 0005: Checkpoints and rewind

- Status: proposed
- Author: r105
- Scope: `src/session.rs` (`save_checkpoint`), `src/ui.rs`
  (`/rewind`, `/compact`, `/clear`)

## Problem

`/clear` destroys the transcript irreversibly, `/compact` replaces it
with a summary, and there is no way to step back more than one exchange
without replaying prompts. Every destructive command is a leap of faith.

## Proposal

- `session::save_checkpoint(paths, state, reason) -> Result<String>`
  saves the full state as `checkpoint-{reason}-{epoch}` inside the
  sessions dir (so `/session load` restores it with existing code) and
  prunes `checkpoint-*` to the newest 10.
- `/rewind [n=1]` (busy-guarded): checkpoints with reason `rewind`,
  drains from the nth-from-last user message into the redo stack without
  touching the composer, pushes a persistent boundary notice naming the
  backup, and reports `/redo to re-apply`.
- `/compact` and non-empty `/clear` checkpoint first (`compact`,
  `clear` reasons). No auto-checkpoint timer.

## Non-goals

No checkpoint restore command (use `/session load`), no timed
snapshots, no diff view of checkpoints.

## Acceptance

- `rewind_truncates_and_pushes_redo`
- `rewind_writes_checkpoint_backup`
- `save_checkpoint_prunes_to_ten` (session tests)

## Amendments

(None yet.)
