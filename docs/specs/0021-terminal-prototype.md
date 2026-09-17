# 0021: Terminal core prototype (PTY + blocks, behind a flag)

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
- Exit codes for interactive blocks stay `None` until shell
  integration (OSC 7 / 633 markers) lands; the one-shot path already
  records exact exits. This is honest scaffolding, not fake data.

## Non-goals

- No `chat` UI changes in this change.
- No prompt detection, no OSC parsing, no persistent allowlists.
- No AI blocks, no block search, no session persistence for blocks.
- No Windows PTY hardening beyond `cmd.exe` fallback.

## Acceptance

- `cargo test terminal::` passes: one-shot exit 0/nonzero, echo,
  block store, resize, ANSI parse.
- Manual: `r105 terminal -- echo hi` prints command, cwd, exit 0,
  output; `r105 terminal` opens a shell, typing works, `Ctrl+Q`
  exits, `--help` lists the subcommand.
- `chat`, `send`, `sandbox` behave exactly as before.

## Amendments

(None yet.)
