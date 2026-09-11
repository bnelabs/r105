# Changelog

All notable changes to r105 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Prompt-prefix caching toggle for llama.cpp-compatible backends via
  `cache_prompt` in config and `/cache-prompt`; disabled by default for
  compatibility with other OpenAI-compatible APIs
- `r105 config-schema` command and `R105_STRICT_CONFIG=1` validation mode
- Plugin host-version/dependency declarations and reload draining for active
  plugin calls
- Versioned session files with legacy migration, full-text `/session search`,
  and model/context prompt settings preserved across session loads
- Visible SSE retry status and actionable backend error messages in the TUI
- Backend usage metadata with token-estimate source/confidence indicators
- `/mcp reconnect <server>` with fresh tool discovery after reconnect
- Configurable core keybindings through the `keybindings` config map
- Dependency-free plain-text conversation export via `/export text`
- `r105 doctor`: environment diagnostics (Python/config/sandbox/backend/
  workspace/skills/API keys) with per-check pass/fail and exit status
- `execute_python` confirmation gate: one-time per-session approval via
  `/approve execute_python`, `--yes` CLI flag, or
  `auto_approve_execute_python` config key (sandbox still applies)
- Web tool rate limiting: 30 searches / 60 fetches per rolling minute
  (cache hits don't consume budget)

### Fixed
- Tool dispatch arity: `ToolRegistry.execute()` adapts to 0/1-arg handlers
  via `call_tool_handler()` (`r105/registry.py`), so `get_time`,
  `calculate`, `convert`, `web_search`, and `web_fetch` no longer raise
  `TypeError` when the LLM calls them; same fix in `PluginRegistry`
- SSRF hardening: DNS now resolves both A and AAAA records, IPv6
  unique-local (`fc00::/7`) and IPv4-mapped literals are blocked, and
  unresolvable hosts fail closed
- Unbounded tool-result cache: LRU cap (128 entries) with recency refresh,
  cleared on `/session load`
- `commands.py`: replaced five `assert`-after-guard checks with explicit
  error returns (asserts vanish under `python -O`)

### Removed
- RAG subsystem: `/rag` commands, `r105 ingest`/`search` CLI subcommands,
  `--rag` flag, `ChatState.rag`, router `/rag/*` endpoints, and RAG docs.
  The router backend now covers profiles + metadata only
- Dead code: duplicate SSRF/HTML/DDG helpers folded into `r105/tools_web.py`
  (which nothing imported), `_SAFE_ENV_PREFIXES_LEGACY` alias; web tools
  send a versioned `r105/{__version__}` User-Agent

### Added
- SSE streaming robustness: `event:` tracking with `event: error` surfacing
  as `RouterAPIError`, malformed-frame counting with structured logging,
  and exponential-backoff retries for transient pre-stream failures
  (connect errors, timeouts, HTTP 5xx)
- Sandbox fallback transparency: `detect_backend_with_reason()` /
  `get_fallback_reason()` explain downgrades, a startup stderr warning and
  a TUI status-bar `⚠ sandbox=<backend>` segment surface weak backends
- Plugin validation: `register()` signature check and tool-definition schema
  validation with specific errors shown by `/plugin reload`
- Ctrl+T tool inspector: modal screen listing every tool call with arguments
  and result previews, with fuzzy filtering
- `convert` tool: unit conversion across length, mass, time, data, speed,
  volume, and temperature; `calculate` gains math functions and pi/e/tau
- Session diffing: `/session load` previews unsaved messages and changed
  settings before restoring
- Collapsible long tool results with diff coloring and code highlighting

### Changed
- Slash-command handlers share `_apply_bool_toggle` / `_parse_choice`
  helpers; `ChatScreen.on_unmount` cancels the in-flight worker
- Structure (no behavior change): `client.py` split into `payload.py`
  (wire format), `sse.py` (streaming core), and `r105/backends/`
  (`base`/`direct`/`router`); `ToolRegistry` moved to `registry.py`;
  `web_search`/`web_fetch` implementations moved to `tools_web.py`;
  tool-loop mechanics extracted to `tool_loop.py`; command handlers take
  a single `CommandContext` instead of five positional arguments

## [0.6.0] — 2026-08

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
