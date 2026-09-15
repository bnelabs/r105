# 0014: Todo renderer

- Status: landed
- Author: r105
- Scope: `src/tool.rs`, `src/model.rs`, `src/ui/transcript.rs`, `src/ui/render.rs`

## Problem

Multi-step tasks have no visible plan: progress lives in prose and is
lost on compaction. Warp renders a server-emitted todo list; r105 can do
the model-driven half locally with the section machinery from 0004.

## Proposal

- New builtin tool `todo_write` with
  `{items: [{content, status: pending|in_progress|completed}]}`. Calling
  it replaces `ChatState.todos` (supersede, max 20 items, exactly one
  `in_progress` enforced by demoting extras to pending).
- Transcript renders the list as one collapsible section (reuses
  `section_state`); footer shows `tasks done/total` when non-empty.
- `todo_write` is allowed in every mode including ask/plan (it mutates
  no workspace state) and never needs approval.

## Non-goals

Subtasks, priorities, persistence beyond the session file, automatic
model compliance (a model may still ignore the tool).

## Acceptance

- `todo_write_replaces_list`, `todo_single_in_progress_enforced`,
  `todo_render_section_collapses`, `todo_footer_counts`,
  `todo_allowed_in_plan_mode`.

## Amendments

(None yet.)
