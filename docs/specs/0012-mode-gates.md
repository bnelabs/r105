# 0012: Modes with teeth

- Status: landed
- Author: r105
- Scope: `src/command.rs`, `src/model.rs`, `src/tool.rs`, `src/backend.rs`, `src/ui/*`

## Problem

`Mode` (build/plan/ask, Tab-cycled) is write-only: nothing reads it, so
plan/ask promise restraint they do not deliver. A user in plan mode gets
the same tool execution as build mode. Comparable tools keep modes as
soft prompts; r105 is local so it can enforce for real.

## Proposal

- `ChatState.mode: String` (serde default `"build"`), synced from
  `UiApp.mode` on Tab/`/plan`/`/build`/`/ask`; persists with sessions.
- `ToolContext` gains `mode: String`. `tool::execute` gates before hooks:
  ask denies every tool; plan allows read-only tools (`read_file`,
  `list_files`, `get_time`, `calculate`, `convert`, `system_info`) plus
  web research (`web_search`, `web_fetch`) and denies the rest with a
  message naming the mode.
- `prompt_messages` prepends a one-line mode preamble (plan: investigate
  only; ask: answer directly) so the model does not spam denied calls.
- Ask mode sends no tool definitions (empty `tools` in payload).

## Non-goals

Per-action approval cards (0013); new modes; backend-side knowledge.

## Acceptance

- `mode_ask_denies_all_tools`, `mode_plan_allows_reads_denies_writes`,
  `mode_build_unchanged`, `mode_preamble_present_only_when_set`,
  `mode_persists_in_session_file`.

## Amendments

(None yet.)
