# 0019: Reasoning traces and cache telemetry

- Status: implemented
- Author: r105
- Scope: `src/model.rs`, `src/sse.rs`, `src/backend.rs`, `src/config.rs`, `src/command.rs`, `src/session.rs`, `src/ui/mod.rs`, `src/ui/pane.rs`, `src/ui/render.rs`, `src/ui/commands.rs`, `src/ui/complete.rs`

## Problem

Reasoning-capable models return a trace alongside the reply. r105 folded that trace into the visible text or dropped it, so tool loops could fail when the trace was required back, the reply was polluted, and prefix-cache savings were invisible.

## Proposal

- Parse `reasoning_content` and `reasoning` deltas separately from `content`; keep them in `ChatResult.reasoning` and `Message.reasoning_content`.
- History echoes the trace verbatim when tools are present and strips it when no tools are requested. Request order stays stable: mode preamble, skill instructions, history, new turn.
- Effort dial `auto|off|none|disabled|low|medium|high|max|xhigh` with wire normalization (`none|disabled` to `off`, `xhigh` to `max`, `auto` omitted).
- Live reasoning renders as a collapsed preview; stored traces render collapsed with the reply below. Footer shows session cache hits and `/tokens` reports last hit/miss.

## Non-goals

- No full terminal emulation or background agents in this change.
- No new wire fields beyond the existing effort hint.
- No cloud sync or sharing.

## Acceptance

- `stream_parses_both_reasoning_keys`, `usage_parses_cache_hits`, `chat_response_keeps_reasoning_separate` pass.
- `reasoning_effort_normalizes_aliases`, `prompt_messages_keep_stable_prefix_order` pass.
- `payload_keeps_reasoning_only_for_tool_loops`, `payload_normalizes_effort_aliases` pass.
- Manual: reasoning-only reply collapses, tool loop preserves the trace, `/tokens` shows cache numbers.

## Amendments

(None yet.)
