# 0006: Compaction safety (pair-aware split, empty guard, retry hint)

- Status: proposed
- Author: r105
- Scope: `src/ui.rs` (`start_compaction`, `Compacted`/`ChatError` arms)

## Problem

The compact split can cut between an assistant tool-call message and its
tool results, stranding results whose call was summarized away. An empty
summary is applied blindly, wiping context for nothing. A failed
compaction reports a generic `/retry` that would resend a stale prompt.

## Proposal

- Pure `split_compact(history) -> (older, recent)`: keep the existing
  last-third ratio, then walk the split backward past leading `tool`
  messages so the kept tail never starts mid-exchange.
- The `Compacted` arm rejects a blank summary: history untouched,
  `set_error("Compaction returned an empty summary · /compact to retry")`.
- A `compacting: bool` flag (set in `start_compaction`, cleared in both
  arms) lets the `ChatError` arm report
  `"compaction failed: … · /compact to retry"` instead of the chat
  `/retry` path.

## Non-goals

No automatic retries, no summary quality scoring, no keep-N flag (the
ratio plus pair repair is the policy).

## Acceptance

- `compact_split_keeps_tool_pairs_intact`
- `compact_rejects_empty_summary`

## Amendments

(None yet.)
