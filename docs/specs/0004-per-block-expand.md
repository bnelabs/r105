# 0004: Per-section transcript expand

- Status: proposed
- Author: r105
- Scope: `src/model.rs` (`Message`), `src/session.rs` (`parse_message`),
  `src/ui.rs` (transcript render, `/expand`)

## Problem

`Ctrl+O` (and the thinking toggle) flip every tool result and thinking
block at once. Reading one long tool output means expanding all of them
and losing your scroll position in noise.

## Proposal

- `Message` gains `#[serde(default)] pub id: String` (old files load
  with `""`); the four constructors set it empty and `parse_message`
  reads it back, so IDs survive save/load round trips.
- `UiApp` lazily assigns `m<N>` IDs (`next_msg_id` counter, reseeded
  from loaded history) the first time a message renders as a section.
  Sections are tool messages plus assistant messages carrying a
  `<thinking>` body.
- `section_state: HashMap<String, bool>` stores only divergences from
  the global defaults (`show_details`, `thinking_default_expanded`);
  `section_order: Vec<String>` rebuilt each draw maps gutter numbers to
  IDs. Collapsed rows render `[n] ▸ …`, expanded `[n] ▾ …`. Failed tool
  results always expand; overrides never hide them.
- `/expand [n|all|none]`: bare toggles the most recent section, `n`
  toggles the nth, `all`/`none` set every current section ID.

## Non-goals

No transcript cursor or keyboard nav, no per-line folding, no splitting
thinking out of its assistant message.

## Acceptance

- `expand_toggles_single_tool_section`
- `expand_all_and_none`
- `message_ids_survive_session_round_trip`
- Render test shows gutter markers for a collapsed tool section.

## Amendments

(None yet.)
