# r105 architecture

r105 is a Rust terminal AI harness. One binary owns the TUI, native window,
PTY terminal, backend protocol, tool loop, persistence, security checks,
native plugins, and MCP transports.

```
┌─────────────────────────────┐       HTTP/SSE       ┌───────────────────────┐
│ TUI or native window        │ ◄──────────────────► │ OpenAI-compatible API │
│ session / panes / composer  │                      │ direct or router      │
└──────────────┬──────────────┘                      └───────────────────────┘
               │
               ├── ChatState + Assistant lifecycle
               │    ├── streamed tokens and reasoning
               │    ├── approval precheck and verdicts
               │    ├── parallel tool execution
               │    └── bounded follow-up rounds
               │
               ├── PTY terminal and vt100 screen
               ├── native tools and sandbox boundary
               ├── native executable plugins
               └── MCP stdio or HTTP servers
```

## Module boundaries

| Module | Responsibility |
| --- | --- |
| main.rs | CLI parsing, startup configuration, command dispatch |
| app.rs | doctor diagnostics |
| approve.rs | approval policy, per-call grants, and touched-file grants |
| assistant.rs | headless window AI lifecycle, cancellation, persistence checkpoints |
| config.rs | paths, schema validation, atomic JSON writes |
| provider.rs | provider presets, environment credentials, connection selection |
| backend.rs | OpenAI-compatible payloads, health, models, profiles, streaming |
| sse.rs | split frame parsing and streaming tool call accumulation |
| model.rs | messages, chat state, usage estimates, skill injection |
| ui/ | Ratatui event loop, panes, tabs, overlays, palette, session view, cancellation |
| command.rs | slash registry, shell word parsing, fuzzy ranking, visible scrolling |
| custom.rs | Markdown workflow discovery and argument expansion |
| suggest.rs | deterministic shell-history and argument completion |
| tool.rs | native tool schemas, dispatch, parallel tool execution |
| edit.rs | anchored edits and structured patches |
| instructions.rs | global plus workspace instruction chain |
| terminal.rs | PTY sessions, vt100 screen, OSC shell markers, command blocks, bounded output and scrollback |
| window.rs | native OS window, layered GPU text/ANSI renderer, PTY input, selection, IME, and AI chrome |
| window/input.rs | native-window clipboard, selection, paste sanitization, and pointer mapping |
| window/rect.rs | solid-rectangle GPU pipeline for window chrome |
| security.rs | workspace containment, DNS/IP blocklist, input limits |
| sandbox.rs | nsjail, bubblewrap, Docker, or timeout fallback execution |
| plugin.rs | executable plugin manifests and JSON stdin/stdout protocol |
| mcp.rs | MCP initialize, tools/list, tools/call, and in memory discovery cache |
| session.rs | versioned session compatibility and atomic persistence |
| export.rs | Markdown, text, JSON, HTML, and dependency-free PDF output |

The code is deliberately a binary crate at this stage. Keeping the modules private prevents accidental coupling while the Rust API settles. The wire and persistence types are serde based so the old config and session formats remain readable.

## Request flow

A normal prompt follows this path:

```
composer
  -> slash command / shell classifier / prompt
  -> ChatState::prompt_messages()
  -> Backend::stream_chat()
  -> SseParser
  -> session or AssistantEvent token/reasoning events
  -> ChatResult
  -> tool calls, if present
  -> approval policy precheck
  -> parallel ToolContext workers
  -> tool messages
  -> Backend::stream_continue()
  -> final session block and checkpoint
```

The backend sends the same OpenAI-compatible request shape for direct and router connections. Router only adds the profile, quality, and routing metadata used by llama-router. The client uses a shared reqwest connection pool and disables automatic redirects for model traffic.

Streaming is cancellation aware. A cancellation token is selected against both
the request body and the response byte stream. The TUI keeps pending prompts
in a queue, and Ctrl+X or Esc cancels the active request and its local tool
batch. The native-window assistant uses the same lifecycle, queues follow-up
prompts, preserves complete tool-call/result pairs, and caps a request at
eight tool rounds.

## UI model

The UI is designed around the working loop used by modern coding harnesses:

- the session view remains the main surface;
- prompts and answers render as compact conversational blocks (`>` and `●`),
  while reasoning and tool output remain expandable metadata;
- the TUI session view is answer-first: `>` prompts and `●` replies are primary,
  while reasoning and tool output remain compact expandable sections;
- TUI tabs own a persistent split tree: right/down splits, compact headers,
  active-adjacent dividers, and a reversible focused-pane zoom keep each live
  session visible without boxing every surface;
- the native window is a separate single PTY surface with status bar,
  composer, AI panel, inline approval bar, selection, clipboard, IME, and
  scrollback; it does not share the TUI tab tree;
- the composer is always available;
- slash commands open a fuzzy palette instead of a separate screen;
- provider and model setup use focused, scrollable pickers;
- build, plan, and ask are explicit modes;
- context usage, sandbox selection, queue length, and cancellation state stay visible in the footer;
- tool output is collapsed until Ctrl+O expands details.

The command palette uses one shared `ensure_visible` calculation for keyboard
movement and rendering. This keeps the selected item visible when the list
extends below the terminal and avoids the Windows bottom-border failure from
the previous UI. The window renderer keeps terminal text, overlays, caret,
selection rectangles, and status bars in separate layers so a chrome update
does not corrupt PTY rows.

## Tool and security flow

Every model tool call is parsed into a native ToolCall. Built in tools validate their input before touching the filesystem, network, or process table.

Workspace paths are resolved against a canonical workspace root. Absolute paths, parent traversal, and symlink escapes fail. Web requests allow only HTTP and HTTPS, reject credentials and metadata hostnames, resolve all addresses, reject any blocked answer, pin the chosen public address for the request, and validate every redirect target before the next request.

The expression parser uses a bounded recursive descent grammar. It caps input size, nesting, numeric literals, powers, intermediate values, and factorial arguments. Tool output is truncated before it enters the session or a follow-up model request.

Rust source sent to execute_rust is written under the workspace .r105/runs directory, compiled, run, and removed. The process is launched through the selected sandbox wrapper with a cleared environment and a hard timeout. The rlimit fallback provides process timeout and output bounds; nsjail, bubblewrap, or Docker provide stronger OS isolation when installed.

## Providers

provider.rs separates user facing preset selection from transport details. A preset contains its backend kind, default URL, credential environment variable, and whether a key is required. The guided /connect flow:

1. selects a preset;
2. asks for a missing cloud key before making a request;
3. creates an in memory candidate connection;
4. calls /v1/models;
5. lets the user choose a returned model;
6. persists provider metadata, URL, backend, and model without credentials.

The llama.cpp preset points at its OpenAI-compatible /v1 endpoint. OpenCode Zen and OpenCode Go use their own base URLs and OPENCODE_API_KEY.

## Persistence

Config writes are serialized to a temporary file, fsynced, renamed into place, and followed by a directory sync where supported. Session writes use the same atomic path.

Session files have a version field and preserve the history, model, context
window, active skills, skill parameters, mode, quality/profile settings, and
trace id. The loader accepts old messages whose content is null or a
structured JSON value and converts them to display text.

The TUI writes an `__autosave__` session before leaving when a pane is
nonempty. `tabs.json` stores tab order, pane-session references, focus, zoom,
and the nested layout; tab switches autosave every live pane before swapping
the in-memory sessions. Explicit `/session save` remains available for named
sessions, search, diff, load, fork, tree, and delete operations.

The native window creates a named `window-<id>` session unless the caller
supplies `--session`. The assistant checkpoints window AI history after each
turn and repairs incomplete tool pairs before the next request.

## Extension protocols

Native plugins are described by JSON files in the configured plugins directory. A plugin executable receives one JSON request on stdin and returns one JSON object on stdout. Tool names are namespaced by plugin.

MCP servers are configured in config.json. /mcp reconnect performs the initialize handshake and tools/list discovery for one or all configured servers. Discovered schemas stay in memory and are included with the next prompt. Tool calls create a short lived stdio process or HTTP request, depending on transport.

All extension paths are explicit and local. Native plugins and MCP remain Rust hosted.

## Release architecture

Cargo produces one native executable per target. Unix archives use tar.gz and
Windows archives use zip. Linux distribution packages reuse the x86_64 GNU
binary for Ubuntu/Debian, Arch, and Fedora; FreeBSD and Alpine build natively
for their different ABIs. macOS arm64 and Windows arm64 are separate release
assets for Apple silicon and Windows on ARM. The macOS packaging helper also
builds a local ad-hoc signed `.app` that launches the window surface; public
distribution still requires Developer ID signing and notarization.

## Verification boundaries

Deterministic checks cover formatting, all-target compilation, Clippy, unit and
integration tests, release metadata, release builds, and window smoke frames
(with and without seeded chrome). A live model endpoint is required only for
streaming, tool-loop, and approval acceptance; endpoint availability and model
loading are deployment concerns, not build invariants.
