# 0008: Session tree and snapping fork

- Status: landed
- Author: r105
- Scope: `src/session.rs` (parent links, tree render, prefix repair),
  `src/ui.rs` (`/session tree`, fork turns, current tracking),
  `src/command.rs` (registry)

## Problem

Sessions are a flat list: forks lose their source, checkpoints look
like clutter, and there is no way to branch from an earlier turn. Full
forks also copy everything, even when the user only wants the first
few turns.

## Proposal

- `SessionFile` gains root-level `parent: Option<String>`; `save` keeps
  a `None` parent via `save_with_parent(paths, name, state, parent)`,
  and `save_checkpoint` takes `parent: Option<&str>`. `SessionInfo`
  gains `parent`, read by `list()`. The manual `parse_message` loader
  ignores the new key, so old files still load.
- `UiApp.current_session: Option<String>`, set by `/session save|load`
  and left alone by fork (you keep working here). Fork records
  `parent = current_session`; checkpoints record it too.
- `/session tree` renders parent chains as an indented tree with the
  existing 80-char preview per node (cycle-safe via a visited set).
- `/session fork <name> [turns]`: with `turns`, keep only the first N
  user turns (boundary-snapped like rewind), then `repair_prefix`
  drops trailing tool messages whose call left the prefix. `/session`
  arg values gain `tree`.

## Non-goals

No checkpoint hiding/filtering in list, no cross-workspace trees, no
graph rendering beyond indented chains.

## Acceptance

- `repair_prefix_drops_stranded_tool_results`
- `fork_with_turns_snaps_to_boundary`
- `tree_renders_parent_chains`

## Amendments

(None yet.)
