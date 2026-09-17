# docs/specs — change specs

One spec per user-visible change expected to exceed ~100 lines of diff
(counting code + tests). Smaller fixes ride along without a spec. Specs are
implementation records as well as proposals: current behavior belongs in
the completed Acceptance or Amendments sections, while the original Problem
and Proposal remain stable.

## Naming

`docs/specs/NNNN-short-name.md`, zero-padded sequence, e.g.
`0007-per-block-expand.md`. The next free number is one past the highest
file present (see this directory listing, not git log).

## Template

Copy `docs/specs/_template.md`. Keep every section; keep each section
short. A spec that needs more than ~80 lines is describing too much —
split it.

## Lifecycle

- Proposed → landed → implemented: the spec is written *before* the code,
  reviewed with the change, and stays as the record. Do not rewrite history;
  if behavior changes later, add a short "Amendments" section at the bottom.
- A spec is done when its Acceptance section is covered by tests named in
  the spec.

Older specs may mention modules that were later split (for example the TUI
module split). Those paths describe the change as it was designed; follow the
current source tree in `ARCHITECTURE.md` for implementation ownership.

## Current completed work

Specs 0019–0023 cover the current reasoning, patch-edit, PTY, native-window,
and inline-AI surfaces. In particular:

- 0021 documents the shared PTY and command-block core.
- 0022 documents the GPU window and platform input foundation.
- 0023 documents the assistant lifecycle, approvals, window persistence,
  selection/clipboard/IME behavior, smoke snapshots, local app packaging, and
  the versioned cross-platform Help/About menu.
