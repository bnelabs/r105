# 0022: Native windowed terminal (own window)

- Status: implemented; renderer/input and AI integration are covered by 0023
- Author: r105
- Scope: `src/window.rs`, `src/main.rs` (`window` subcommand), `Cargo.toml` (`winit`, `wgpu`, `glyphon`, `pollster`)

## Problem

r105 runs inside the user's existing terminal. The goal is a
clickable app: open r105, its own window appears, and all work
continues from there. The window must stay a single Rust binary with
no runtime beyond the OS.

## Proposal

- New `r105 window` subcommand opening a native OS window (winit)
  with GPU rendering (wgpu) and shaped text (glyphon over
  cosmic-text, system monospace, free font fallback).
- The window hosts the existing terminal core: `PtySession` spawns
  the shell, the vt100 screen feeds one glyphon buffer per frame,
  window resize reflows the PTY (`cols`/`rows` from cell metrics).
- Keyboard forwards to the PTY (printable text plus escape
  sequences for Enter/Backspace/Tab/arrows/Home/End/F-keys/Ctrl).
  Window close quits; the shell is killed on exit.
- `--smoke N` runs N frames headless-ish then exits with a
  `frames=`/`screen_bytes=` report for CI and agents.
- `chat`, `send`, `sandbox`, `terminal` are untouched. The AI merge
  (composer + blocks in the window) is the next spec, not this one.

## Non-goals

- No cursor quad, no selection/clipboard, no IME positioning, no
  ligature toggles, no tabs, no AI composer in this change.
- No `.app`/installer packaging in this change (binary first).
- No Windows/Linux hardening beyond compiling and basic run.

## Acceptance

- `cargo build` succeeds; `r105 window --smoke 60` exits 0 and
  reports nonzero `screen_bytes` (shell prompt rendered).
- Manual: `r105 window` opens a titled window, typing runs in the
  shell, resize reflows, close quits cleanly.
- `fmt`, `clippy -D warnings`, full test suite green.

## Amendments

- 0023 completed the previously deferred input paths: caret placement, inline
  AI composer/panel/approval chrome, clipboard, drag selection, IME commits,
  smoke snapshots, named window-session checkpoints, and local macOS app
  packaging. The native window remains a single PTY surface; TUI tabs and
  nested split panes are a separate `chat` surface.
