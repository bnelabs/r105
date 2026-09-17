# 0020: Patch edits and diff approvals

- Status: implemented
- Author: r105
- Scope: `src/edit.rs`, `src/tool.rs`, `src/approve.rs`, `src/ui/approve.rs`, `src/ui/render.rs`, `src/instructions.rs`, `src/model.rs`, `src/main.rs`, `src/sandbox.rs`

## Problem

The harness could only rewrite whole files, so small fixes re-sent entire contents. Approval cards showed a one-line summary with no diff, and repeated writes to the same file re-prompted every turn. There was no project instruction chain and no way to probe the sandbox boundary.

## Proposal

- New `edit_file` (anchored `old_text` to `new_text`, `replace_all` for multiples) and `apply_patch` (`*** Begin Patch` with Add/Update/Delete File and `@@` hunks using ` `, `-`, `+`). Relative paths only, ambiguous anchors fail.
- Approval cards preview writes: line counts for `write_file`, anchored diff for `edit_file`, per-file lines for `apply_patch`. `a` grants the exact call plus per-file grants so later writes to the same path skip cards this run.
- Instruction chain prepended per request: global `AGENTS.md`, workspace `AGENTS.md`, `AGENTS.override.md`, `.r105/AGENTS.md`, each capped at 32 KiB.
- New `sandbox` subcommand runs a command in the selected backend or prints the backend when empty.

## Non-goals

- No interactive terminal emulation in this change.
- No persistent allowlists across runs.
- No multi-hunk fuzzy matching; hunks must match exactly once.

## Acceptance

- `edit_applies_anchored_replacement`, `edit_rejects_ambiguous_anchor`, `patch_add_update_delete_roundtrip`, `patch_rejects_absolute_and_ambiguous` pass.
- `edit_tools_are_write_gated`, `patch_summary_lists_files` pass.
- `mode_plan_allows_reads_denies_writes` covers the new tools.
- Manual: card shows a preview, `a` on a file skips later cards for that path, `sandbox -- date` reports the backend.

## Amendments

(None yet.)
