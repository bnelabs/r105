# docs/specs — change specs

One spec per user-visible change expected to exceed ~100 lines of diff
(counting code + tests). Smaller fixes ride along without a spec.

## Naming

`docs/specs/NNNN-short-name.md`, zero-padded sequence, e.g.
`0007-per-block-expand.md`. The next free number is one past the highest
file present (see this directory listing, not git log).

## Template

Copy `docs/specs/_template.md`. Keep every section; keep each section
short. A spec that needs more than ~80 lines is describing too much —
split it.

## Lifecycle

- Proposed → landed: the spec is written *before* the code, reviewed with
  the PR, and stays as the record. Do not rewrite history; if behavior
  changes later, add a short "Amendments" section at the bottom.
- A spec is done when its Acceptance section is covered by tests named in
  the spec.
