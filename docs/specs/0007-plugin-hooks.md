# 0007: Plugin before/after_tool hooks

- Status: proposed
- Author: r105
- Scope: `src/plugin.rs` (manifest, invoke, hook runners),
  `src/tool.rs` (`execute`)

## Problem

Plugins can only add tools; they cannot observe, gate, or rewrite tool
execution. There is no way to add a policy ("never write outside src"),
audit logging, or result post-processing without forking r105.

## Proposal

- Manifests gain optional `hooks: ["before_tool", "after_tool"]`
  (serde default empty; only declaring plugins are ever spawned).
- `plugin.rs` extracts `invoke_plugin(manifest, workspace, request,
  timeout)`, reused by `call_from` (30s) and hooks (5s).
- `tool::execute` runs `before_tool {tool, arguments}` after argument
  repair: `{"deny": reason}` aborts the call with a visible error,
  `{"arguments": {...}}` replaces the arguments (chained across
  plugins); then the tool runs; then `after_tool
  {tool, arguments, result}` may replace `result` via `{"result": …}`.
- Hook transport failures fail the tool visibly (`tool error: … hook
  …`), never silently. Pure decision helpers
  (`apply_before_response`, `apply_after_response`) carry the unit
  tests; one unix-gated integration test proves end-to-end deny.

## Non-goals

No hook chaining UI, no per-tool hook subscriptions (all declared hooks
see all tools), no hook for chat/completion events.

## Acceptance

- `before_hook_deny_blocks_tool`
- `before_hook_rewrites_arguments`
- `after_hook_rewrites_result`
- `deny_hook_blocks_execute_end_to_end` (unix-only)

## Amendments

(None yet.)
