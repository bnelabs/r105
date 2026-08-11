# Changelog

All notable changes to r105 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Collapsible reasoning panel (roadmap): thinking blocks now render as an
  interactive `💭 THINKING` panel that can be expanded/folded with a click or
  `t`/`Enter`/`Space` — previously the panel was a static folded preview.
  `thinking_default_expanded` still controls the initial state
- Virtualized transcript (roadmap): the chat view materializes only the
  messages visible in the viewport (plus an overscan window) as widgets, so
  very long sessions stay fast and memory-bounded while the full transcript
  is preserved and re-rendered on scroll; auto-follow pins to the newest
  message until the user scrolls up
- Config-driven family overrides (roadmap): new `model_families` config key
  maps model-name fragments to families (or `null` to force opaque
  passthrough), overriding the built-in catalog — e.g. a Gemma-4 fine-tune
  with a custom name can be forced into channel-syntax handling, and a
  misclassified model can be opted out. Applies to the TUI thinking capture
  and the client-side native tool-call parsing, and is re-resolved on
  `/model`

## [0.5.0] — 2026-08

### Added
- Model-agnostic context window: context capacity is resolved from the active
  model (config override → backend probe → built-in catalog → default) instead
  of a hardcoded 262144 — fixes wrong ctx reported for models like
  muse-glimmer-30B (131072). New `r105/model_catalog.py`; overridable via
  `model_contexts` and `context_tokens` in config.json
- `reasoning_effort` chat setting (`auto|off|low|medium|high`): explicit levels
  are sent to reasoning-capable backends; `/reasoning` slash command; persists
  to config.json
- Thinking-model support in the TUI: Gemma-4-style thinking blocks
  (`<|channel|>thought…<channel|>`) are captured across stream chunks and
  rendered as a collapsible `💭 THINKING` panel (folded by default) instead of
  being silently stripped; `show_thinking` / `thinking_default_expanded`
  settings
- Permission posture (`full-access|restricted|sandboxed|off`): user-selectable
  tool-execution policy mapped onto the sandbox backends; `restricted` blocks
  code execution and network tools, `off` disables all tools; `/permissions`
  slash command
- HelpScreen (F1) is now dismissible with Escape or `q` — previously the modal
  could only be closed via the Close button

### Changed
- `/model <name>` re-resolves the context capacity for the newly selected model
- Default permission posture is `sandboxed` (preserves prior auto-detect
  behavior); `sandbox_backend` config still selects the backend
- Gemma-4 native tool-call parsing (`<|tool_call|>` blocks) and the
  repeated-tool-call loop guard (previously uncommitted WIP) are now part of
  the release, gated to Gemma-4-family models via the model catalog — for any
  other model the content is treated as opaque text and never regex-interpreted

### Fixed
- HelpScreen escape-dismiss regression (Textual 8 modal trap)
- TUI crash on Ctrl+U / Ctrl+W / Ctrl+K edit keybindings (Textual 8 changed
  `Document.replace` to `replace_range`; found and fixed during live
  verification)
- Chat input kept a stray newline after Enter, hiding typed text on an
  invisible second line (found and fixed during live verification); Shift+Enter
  now inserts a newline as documented

## [0.4.1] — 2026-08

### Fixed
- TUI ignored keyboard input at startup on Textual 8 (RichLog stole focus from
  the chat input); the input is now focused explicitly on mount
- Blank assistant replies from thinking models (Qwen3, DeepSeek, Glimmer, etc.)
  that emit output in `reasoning_content`; r105 now falls back to
  reasoning_content when content is empty and renders it in the TUI


## [0.4.0] — 2026-08

### Added
- Config validation with clear error messages; invalid values now raise on save
- Export dependency guard: /export shows a friendly install hint when optional export deps are missing
- Unit test for the export dependency guard
- pip cache for setup-python in CI

### Changed
- Document export dependencies moved to optional extra `r105[export]`; core deps trimmed
- Release workflow: binary builds consolidated into a matrix with pinned runners (linux, macos x64, macos arm64, windows)
- CI actions bumped off Node 20 (checkout v5, setup-python v6, codecov v6)

### Fixed
- CI lint failures (ruff SIM102/RUF003/I001)
- Release workflow: formula push no longer fails on detached HEAD ("fatal: You are not currently on a branch.")
- update-formulas no longer runs when the PyPI publish failed

### Removed
- Stale `build/` output that was tracked in the repository

## [0.3.1] — 2026-06

### Added
- Windows x64 binary build
- macOS ARM binary build

### Changed
- Binary naming fixed (platform-suffixed release assets)
- Stale publish workflow removed

## [0.3.0] — 2026-06

### Added
- NsjailSandbox backend with seccomp-bpf syscall allowlist
- SandboxProfile system for per-tool isolation configuration
- MCP SSE transport (MCPSSEClient) alongside existing stdio transport
- Animated streaming indicator in status bar during SSE receive
- Auto-save on TUI exit (`__autosave__` session)
- Fuzzy history search in Ctrl+R browser (difflib-based live filtering)
- Diff-aware write_file with dry_run mode and unified-diff generation
- DiffView TUI widget with approve/reject buttons for file changes
- Homebrew formula template in docs/homebrew.rb

### Changed
- Backend priority: nsjail > bwrap > rlimit > none
- BwrapSandbox uses minimal /dev bind (null, urandom, zero, fd) instead of full /dev
- BwrapSandbox conditionally grants network/filesystem per SandboxProfile
- MCPClient refactored into abstract MCPClientBase + MCPStdioClient + MCPSSEClient
- MCPServerConfig supports `transport` (stdio|sse) and `url` fields
- HistoryScreen with fuzzy search Input, Ctrl+S quick-save
- write_file returns 'created' for new files, diff-aware messages for edits
- StatusBarWidget with animated streaming indicator replacing static emojis
- Version bumped to 0.3.0

### Fixed
- Version mismatch between pyproject.toml and __init__.py
- _sync_request passing json kwarg to GET/DELETE (broke health, profiles)
- Sync send() not including tool_calls in history
- execute_python returning empty string for signal-killed processes
- test_timeout assertion always passing due to 'elapsed > 0' escape hatch

## [0.2.0] — 2025-06

### Added
- Sandbox abstraction with `BwrapSandbox`, `RLimitSandbox`, and `NoopSandbox` backends
- Plugin system (`r105/plugins.py`): custom tools from Python files in `~/.config/r105/plugins/`
- MCP (Model Context Protocol) support (`r105/mcp_client.py`): JSON-RPC over stdio
- Session management: `/session save|load|list|delete`, `--session` CLI flag
- Conversation export: `/export markdown|json|html`

## [0.1.0] — 2025-05

### Added
- Initial release: TUI frontend for llama-router
- Interactive chat with streaming SSE support
- Built-in tools: `execute_python`, `write_file`, `read_file`, `list_files`, `web_search`, `web_fetch`, `get_time`, `calculate`, `system_info`
- Slash-command system with fuzzy command palette
- Theme system with 4 built-in themes (r105, dracula, solarized-dark, high-contrast)
- Skills system with markdown skill files and parameter substitution
- RAG integration with ingest, search, list, delete commands
- File explorer sidebar
- Token usage estimation and auto-compaction at 80% context
