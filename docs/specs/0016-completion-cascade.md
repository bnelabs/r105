# 0016: Warp-style completion cascade

- Status: landed
- Author: r105
- Scope: `src/suggest.rs` (new), `src/ui/ghost.rs`, `src/config.rs`, docs

## Problem

Ghost-text runs on a 531MB sidecar model to complete shell lines Warp
answers from history and a signature table with zero weights. The
neural source is disproportionate: slow to provision, heavy resident,
and weaker than frequency on repetitive terminal work.

## Proposal

- New `src/suggest.rs`: pure cascade `suggest(input, history, cwd)`.
  `ghost_prefix` trigger unchanged. Layer 1: shell-history frequency
  (`10×count + 25×cwd_hits + newest_index`, prefix match, returns the
  suffix). Layer 2: path top-hit (last-token fragment, basename
  scored by the existing file scorer, suffix appended). No network.
- `ShellHistory`: capped ring (default 500, `completion_history_max`),
  recorded on every `!` run with the workspace, persisted as
  `<config>/shell_history.json` (load at startup, save on record;
  corrupt file starts empty).
- Ghost tick goes synchronous: history/path resolve in microseconds,
  so `GhostReady`, generations, and in-flight tracking go away. The
  0015 shell (debounce, dim render, Tab/Esc) stays. Dismiss records
  the input so the tick does not resurrect it; edits clear it.
- `/completion [status|clear]`: source counts and history size; the
  sidecar commands go away with the sidecar.

## Non-goals

Backend-assist ghost (matches Warp TUI, which has no AI ghost);
signature DB for flags (later spec); history sync across machines.

## Acceptance

- `suggest_history_prefers_frequent`, `suggest_cwd_boosts_same_dir`,
  `suggest_recency_breaks_ties`, `suggest_path_completes_basename`,
  `suggest_history_cap_truncates`, `ghost_dismiss_stays_dismissed`.

## Amendments

(None yet.)
