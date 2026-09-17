# 0023: AI merge — composer and harness blocks in the native window

- Status: implemented
- Author: r105
- Scope: `src/assistant.rs` (new), `src/window.rs` (chrome), `src/main.rs` (pass backend/state/config), `src/ui/approve.rs` (share card text)

## Problem

The native window (0022) runs a shell but has no AI; the harness
lives in the separate `chat` TUI. The goal is one surface: terminal
canvas plus inline AI, sharing one block timeline.

## Proposal

- New `src/assistant.rs`: headless orchestrator owning a `ChatState`
  for the window session. `ask(prompt)` queues work; a tokio task
  runs the same loop as the TUI: `stream_chat`, history push with
  reasoning echo, up to 8 tool rounds, `approve::resolve` precheck
  (Allow runs, Deny becomes a tool error, Ask emits
  `ApprovalNeeded` and waits for a `Once`/`Always`/`Deny` verdict),
  parallel `execute_calls`, `Message::tool` results,
  `stream_continue`. Events (`Token`, `Reasoning`, `Status`,
  `ToolStarted`, `ApprovalNeeded`, `Done`, `Error`) flow back over a
  channel; cancellation via token.
- Pure, tested helpers in the same module: `ComposerState`
  (multiline insert/newline/backspace/arrows/word-kill) and
  `AiHistory` (prompt/response/rounds/wall per block).
- `src/window.rs` chrome, all GPU: status bar (always, PTY grid
  shrinks by one line), composer overlay (`Ctrl+J`, `Enter`
  submits, `Alt+Enter` newline, `Esc` closes, `Ctrl+C` cancels a
  run), AI panel (`Ctrl+K`, block list plus selected full text),
  approval bar (`y` once, `a` always, `n` deny). Solid-rect pipeline
  backs panels; cursor located from cosmic-text layout runs.
- Card text shared with the TUI: `call_text` and
  `approval_preview` become `pub(crate)` in `src/ui/approve.rs` so
  window approvals read identically.
- `main.rs` passes `backend`, `state`, `config` into `WindowOptions`;
  policy and sandbox build from config exactly like `UiApp::new`.

## Non-goals

- No session persistence for window AI history in this change
  (in-memory; TUI sessions untouched).
- No markdown rendering, no selection/clipboard, no IME, no tabs,
  no auto-compaction in the window.
- No `.app` packaging in this change.

## Acceptance

- Unit: composer editing, AI history, approval-verdict plumbing pass.
- `r105 window --smoke 60` still exits 0 with nonzero screen bytes.
- Manual: `Ctrl+J`, ask a question, tokens stream into the AI panel;
  a write tool raises the approval bar; `a` runs it and later writes
  skip; `Esc`/`Ctrl+C` stop a run; shell keeps working throughout.
- `fmt`, `clippy -D warnings`, full suite green.

## Amendments

- Follow-up: cancellation uses a fresh token for each request, preserving
  queued prompts and subsequent use of the window. Idle cancellation is harmless.
- Once approvals grant only the current execution batch; Deny creates no
  grants; Always retains exact-call and touched-file session grants.
- Interrupted or capped tool rounds receive missing tool-result messages
  before the next prompt, keeping backend history structurally complete.
- Regression coverage includes a local HTTP/SSE server exercising cancellation
  followed by a queued streamed reply, approval grant lifetimes, and interrupted
  history repair. A real-model interactive acceptance run remains opt-in
  (`R105_TEST_URL`/`R105_TEST_MODEL`) and passed against a local llama.cpp
  server: streamed reply, a `write_file` approval, and the approved write.
- Window AI history persists: each window session checkpoints to a
  `window-<uuid>` session and restores into the panel on `--session`.
- Window usability: clipboard copy/paste (Cmd+C/V on macOS,
  Ctrl+Shift+C/V on Linux/Windows), terminal drag
  selection with scrollback, IME preedit input, and smoke snapshots
  (`--smoke-snapshot`) backed by explicit GPU acceptance tests.
- macOS `.app` bundling via `packaging/build_macos_app.sh` (ad-hoc signed;
  distribution still requires Developer ID signing and notarization) and
  app-bundle launch defaults to the window surface.
- Current readiness check (2026-09-17): release build, exact 120-frame smoke
  runs, clean PPM capture, local HTTP/SSE orchestration tests, ignored GPU
  tests, and the rebuilt ad-hoc app bundle pass. The configured
  `192.168.68.57:8001` llama.cpp router is reachable; its loaded
  `Qwen3.8-27B-GSQ-RCO-IQ3_S-mtp` model passed the live streamed reply plus
  approved `write_file` acceptance test and the release CLI `READY` probe.
  Other catalog entries may remain unloaded, so live checks must name a loaded
  model explicitly.
