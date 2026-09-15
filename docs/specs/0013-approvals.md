# 0013: Per-action approvals

- Status: landed
- Author: r105
- Scope: `src/approve.rs` (new), `src/tool.rs`, `src/config.rs`, `src/ui/*`

## Problem

Permission postures are coarse and silent: within a posture every tool
runs without an interactive checkpoint. There is no per-category ask
policy and no command allow/deny lists, so a destructive call is one
model decision away with no human in the loop.

## Proposal

- New `src/approve.rs`: `Policy` (per-category `allow|ask|deny` for
  exec/write/read/network/mcp/plugin, plus `command_allowlist` /
  `command_denylist` regexes over `name + args summary`, plus in-memory
  `session_allow`) and pure
  `resolve(name, args, mode, posture, policy) -> Allow | Ask | Deny`.
  Order: mode gate (0012) → posture → denylist → session/allowlist →
  category.
- `tool::execute` enforces `resolve`: `Deny` bails with reason, `Ask`
  without a session allow bails as "requires approval". Config keys:
  `approval_exec` (default ask), `approval_write` (ask),
  `approval_read` (allow), `approval_network` (allow),
  `approval_mcp` (ask), `approval_plugin` (ask).
- TUI pre-checks each call before spawn: `Ask` shows an inline approval
  card (`y` once / `a` session-allow pattern / `n` deny with reason fed
  back as the tool result). Cards resolve sequentially.

## Non-goals

Persistent allowlist file; editing arguments inside the card; approvals
outside the TUI (ask behaves as deny with a message).

## Acceptance

- `approval_deny_beats_allowlist`, `approval_ask_without_session_denies`,
  `approval_session_allow_permits`, `approval_invalid_regex_rejected`,
  `approval_card_keys_resolve`.

## Amendments

(None yet.)
