# Rust migration record

This document records the completed Rust-only transition from the
Python/Textual implementation shipped in r105 0.8.x to the native r105 1.0
release.

## Decisions

- The final application is Rust-only. It does not embed Python and does not
  require a Python runtime for normal use.
- The existing `~/.config/r105/config.json` and session JSON files remain the
  compatibility boundary. New Rust code reads and writes them with explicit
  schema versions and atomic replacement.
- The Rust 1.0 line is the supported product. The 0.8.x Python line remains in
  the git history for compatibility reference only.
- Python plugins, Python document exporters, and `execute_python` were removed.
  Native Rust plugins, `execute_rust`, and dependency-free Markdown, text, JSON,
  HTML, and PDF exporters replace them.

## UX direction

The current Textual interface has too much permanent chrome and makes the
command palette compete with the composer. The Rust UI uses a focused work
surface:

```
┌ r105 · provider/model · workspace · context ──────────────── ? help ┐
│                                                                    │
│  conversation transcript                                           │
│  user prompt                                                       │
│  assistant response                                                │
│    collapsed tool activity  [expand]                               │
│                                                                    │
├────────────────────────────────────────────────────────────────────┤
│  > Write a prompt…                                      send ↵      │
├────────────────────────────────────────────────────────────────────┤
│  ready · model · 12k/32k · sandbox · / for commands · Esc cancel   │
└────────────────────────────────────────────────────────────────────┘
```

The design borrows interaction patterns that work well in established coding
harnesses: OpenCode's explicit Plan/Build mode switch and file fuzzy search,
Claude Code's interrupt, queue, transcript viewer, task status, and reverse
history search, and Aider's visible chat modes, `/diff`, `/map`, `/model`, and
external-editor escape hatch. These patterns are adapted to r105's local-first
provider and tool model rather than copied visually.

UX requirements for the Rust implementation:

1. The transcript is the primary surface. Sidebars are opt-in overlays or
   narrow panels, never a permanent 40-column tax on small terminals.
2. The composer remains visible while the user browses commands, models,
   sessions, files, and tools. Every picker owns its scroll position and keeps
   the highlighted row visible on Windows and Unix terminals.
3. Long-running work is explicit: the footer shows the active operation,
   elapsed time, cancellation key, and queued prompts. `Esc` cancels the active
   operation; `Ctrl+C` remains the terminal interrupt fallback.
4. Tool activity is compact by default and expandable on demand. Errors expose
   the failed operation, a useful next action, and a copyable detail view.
5. Provider and model selection is a guided picker with live connectivity and
   model discovery. Keys remain session-only and are never rendered into the
   transcript or persisted configuration.
6. All important actions have keyboard and command paths. Mouse support is an
   enhancement, not a dependency.
7. Themes are token-based Rust data, with light/dark/accessible defaults and a
   terminal ANSI fallback.

## Native architecture

The first Rust crate is intentionally split by responsibility:

| Module | Responsibility |
| --- | --- |
| `config` | Versioned config, defaults, validation, atomic writes, schema export |
| `model` | Chat state, messages, tool calls, usage, provider metadata |
| `backend` | OpenAI-compatible and llama-router requests, model listing, health |
| `sse` | Cancellable SSE parser with malformed-frame and retry handling |
| `provider` | Built-in provider catalog and guided connection data |
| `session` | Versioned session files, search, diff, and export |
| `command` | Slash parser, registry, completion metadata, and handlers |
| `tool` | Formal Rust tool trait, schema, validation, dispatch, and loop |
| `security` | URL/IP checks, workspace containment, argument limits, permissions |
| `sandbox` | Native process limits and optional nsjail/bwrap/docker backends |
| `mcp` | stdio and HTTP MCP client with explicit transport compatibility |
| `plugin` | Signed/declared Rust executable plugin protocol |
| `ui` | Ratatui renderer, event loop, overlays, transcript, composer |
| `export` | Native text, Markdown, JSON, HTML, and PDF output |

The application uses Tokio for cancellable async work, Reqwest for pooled HTTP,
Serde for wire/config/session data, and Ratatui/Crossterm for the terminal
surface. A static musl build is produced for Alpine; platform builds keep the
existing macOS, Windows, Linux, and FreeBSD matrix.

## Completed migration gates

The Rust implementation is now the product entry point. The completed release
checks are:

- `cargo fmt --check`, `cargo check`, `cargo clippy --all-targets
  --all-features`, and `cargo test --all-targets` pass on the local supported
  toolchain.
- Unit coverage exercises config defaults/schema, provider aliases and URL
  validation, session round-tripping, SSE frame and tool-call parsing, command
  filtering/scroll visibility, workspace containment, and sandbox selection.
- A local OpenAI-compatible mock has passed the non-streaming CLI path; live
  provider, streaming, tool-loop, and native package checks remain release or
  environment checks.
- Release CI defines the supported binary and native package matrix and runs
  per-target smoke tests when a version tag is pushed.
- The Rust CLI, guided provider flow, native tool loop, MCP client, plugin
  protocol, sessions, exporters, and release matrix are the supported paths.

## Delivery record

1. Added the Rust crate and compatibility boundary for config and sessions.
2. Implemented config, sessions, models, provider catalog, HTTP, and SSE.
3. Implemented the focused TUI shell, composer, transcript, pickers, and
   status surface.
4. Implemented the native tool registry, security checks, sandbox runner, and
   tool loop.
5. Implemented MCP transports and the Rust executable plugin protocol.
6. Implemented native exporters and Rust binary/package release workflows.
