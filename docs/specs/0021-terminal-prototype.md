# 0021: Terminal core (PTY + blocks, behind a flag)

- Status: implemented
- Author: r105
- Scope: `src/terminal.rs`, `src/main.rs` (`terminal` subcommand), `Cargo.toml` (`portable-pty`, `vt100`)

## Problem

The harness has no real terminal: no PTY, no interactive shell, no
command blocks with working directory and exit status. The full
terminal canvas (persistent shell, block list, AI blocks) is the
largest change and must land without breaking the working `chat` flow.

## Proposal

Land the core behind a separate subcommand; `chat` stays untouched.

- New `src/terminal.rs`: PTY session via `portable-pty`, screen via
  `vt100::Parser`, block store with `seq`, `command`, `cwd`,
  `started`, `ended`, `exit_code: Option<i32>`, output tail.
- `PtySession::spawn(cwd, rows, cols)`: launches `$SHELL` (fallback
  `sh`, Windows `cmd.exe`), non-blocking reader thread, `write`,
  `drain` into the vt100 parser, `resize`, `screen_text`.
- `run_command_in_pty(program, args, cwd, timeout)`: one-shot PTY run
  used by `terminal -- <cmd>` and tests. Returns exact `cwd` (known)
  and `exit_code` (waited). Captures output tail (last 32 KiB).
- Interactive `r105 terminal` (no args): minimal Ratatui loop owning
  the alternate screen, forwarding keys to the PTY, rendering the
  vt100 screen, status bar with `block #N`, `cwd`, last exit.
  `Enter` snapshots a block (command line mirror, spawn cwd, start
  time); `Ctrl+Q` quits; `Ctrl+B` toggles the block list overlay;
  resize events forward to the PTY.
- Recognized interactive shells install and parse OSC 7/133/633 markers for
  cwd, command boundaries, and exit codes. Shells without integration fall
  back to a completed block with no fabricated exit code; the one-shot path
  records exact exits independently.

## Non-goals

- No `chat` UI changes in this change.
- No persistent allowlists or block-session persistence.
- No AI blocks, no block search, no session persistence for blocks.
- No Windows PTY hardening beyond `cmd.exe` fallback.

## Acceptance

- `cargo test terminal::` passes: one-shot exit 0/nonzero, echo,
  block store, resize, ANSI parse, split OSC marker parsing, and
  recognized-shell exit/cwd integration.
- Manual: `r105 terminal -- echo hi` prints command, cwd, exit 0,
  output; `r105 terminal` opens a shell, typing works, recognized shells
  report interactive exit/cwd markers, `Ctrl+Q` exits, and `--help` lists
  the subcommand.
- `chat`, `send`, `sandbox` behave exactly as before.

## Amendments

(None yet.)
