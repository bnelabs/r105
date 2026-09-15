# 0009: `#` classify prefix

- Status: landed
- Author: r105
- Scope: `src/command.rs` (`classify_input`), `src/ui.rs` (submit path)

## Problem

Everything that is not `!` or `/` goes to the model, so `ls -la` typed
without `!` becomes a (confusing) agent prompt. There is no cheap local
step that notices shell-shaped input before spending a model round trip.

## Proposal

- Pure `command::classify_input(text) -> Route { Shell, Agent,
  Ambiguous }` with cheap gates only, in order: blank/ambiguous guard,
  one-off shell keywords (`ls`, `cd`, `git`, …) and shell-syntax scan
  (`|`, `>`, `&&`, `$`, …) → Shell; installed-binary first token →
  Shell; question/letter shape with no shell syntax → Agent; otherwise
  Ambiguous. No model call, no PATH writes, no learning.
- `# <text>` strips the prefix and routes: Shell prefills the composer
  with `!<text>` plus a "looks like shell — Enter to run" hint (never
  auto-runs); Agent submits as a prompt directly; Ambiguous prefills the
  plain text with a "Enter sends to agent, ! runs shell" hint.
- The `#` form is also recorded for palette recency as `#classify`? No:
  it is not a slash command and stays out of the palette entirely.

## Non-goals

No auto-execution of anything, no model-based routing, no persistent
statistics, no bare-input (prefix-less) reclassification.

## Acceptance

- `classify_routes_shell_agent_ambiguous`
- `hash_prefix_prefills_shell_for_ls`
- `hash_prefix_submits_question_to_agent` (prompt path, no I/O)

## Amendments

(None yet.)
