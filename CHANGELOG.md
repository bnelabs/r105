# Changelog

All notable changes to r105 are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Addressable transcript blocks: every message is addressable as `#n`.
  `/filter <block> <pattern>` narrows a block's output (substring,
  `--regex`, `--case`, `--invert`, `--context N`) with a dim trailer
  showing hidden lines and the clear command; `/block [n]` lists or
  describes blocks; `/rerun [n]` resubmits an earlier prompt or `!`
  line (`/` commands prefill instead of replaying); `/copy out [n]`
  copies a block verbatim; `/expand` accepts `#n` addresses.
- Shell-command ghost layer: `!` lines now complete the command name
  from shell builtins and PATH executables (30s cache) between the
  history-frequency and path layers.
- Context-aware shell arguments: `!` lines complete subcommands and
  flags for common commands (git, cargo, npm, docker, kubectl, gh,
  ssh, make, go, pip, brew, tmux, terraform, node, yarn, pnpm, python,
  scp, helm, systemctl) plus live values — git branches/tags/remotes
  from `.git`, npm scripts, make targets, ssh hosts, kubectl resource
  types, files — all from local files, no subprocess. Nested verbs
  complete a level deeper (`gh issue list`, `docker container ls`,
  `git stash pop`).
- Shell alias expansion: rc-file aliases and gitconfig aliases expand
  before spec lookup, so `g st` completes as git and `git co` offers
  checkout's branches; aliases also appear in command-position menus.
- Shell Tab menu: Tab on a `!`/`/sh` line applies the single match at
  once or opens a navigable menu when ambiguous (history first, then
  context rows tagged by kind, then files); ↑↓ move, Tab fills, Esc
  closes, click selects and fills.
- Post-failure shell corrections: a failed `!` line offers up to three
  ranked fixes when local rules match (mistyped command, git
  subcommand/branch/flag, missing upstream, `chmod +x`, mistyped
  path); `→` applies the best into the empty composer, alternates
  list in the transcript notice and status, any edit drops them.
- Partial ghost accept: `→` at end of input takes the whole ghost,
  Ctrl+`→` takes one word.
- Session pane (Ctrl+B, remappable `sidebar` action): a left column
  listing saved sessions and recent workspaces — open, start fresh,
  delete, filter, and switch workspaces by keyboard or mouse.
  Switching autosaves the live transcript first, so nothing is lost;
  recents persist in `recent_workspaces.json`.
- Composer history walk like a terminal: ↑/↓ preview earlier user
  turns, first ↑ stashes the live draft, ↓ past the newest restores it,
  Esc restores, any edit adopts. The composer title shows the walk.
- Empty-composer coaching hint (`prompt · ! shell · / commands ·
  # route · ↑ history`) — display only, never part of the input.

### Changed
- `/help` is grouped (Essentials, Modes & guardrails, Ask & answer,
  Files & context, Transcript, Providers & plugins) with a shorter keys
  block and a `/help <command>` tip, instead of one 50-row wall.
- `/state` renders labeled lines (mode, model, backend, permissions,
  approvals, thinking, quality) instead of a raw `key=value` dump, and
  includes the approval policy summary.
- The transcript pane lost its `transcript` title; the conversation
  needs no label.
- Ghost completion yields to open menus (palette, `@` files, argument
  values) instead of fighting them for Tab.

### Fixed
- Stray "limbo" text on screen: TUI runs now log to `<config>/r105.log`
  instead of stderr, so tracing warnings can no longer scribble rows
  ratatui never repaints. Headless subcommands keep stderr logging.

## [2.1.0] — 2026-09-15

### Added
- Shell-history completion cascade replacing the sidecar model: `!` and
  `/sh` ghosts resolve from shell-history frequency (cwd-weighted,
  persisted as `shell_history.json`, capped by `completion_history_max`)
  then path top-hit. The debounce/dim/Tab/Esc shell is unchanged; the
  tick is synchronous, so generations and in-flight tracking are gone.
  `/completion [status|clear]` replaces the sidecar commands.

### Changed
- Removed the `llama-server` sidecar path and its config keys
  (`completion_endpoint`, `completion_model_path`,
  `completion_timeout_ms`); stale keys warn as unknown. No weights to
  fetch, no inference engine to install.

## [2.0.0] — 2026-09-15

### Added
- Enforced working modes: plan allows reads and web research but refuses
  writes and execution, ask answers without tools (`todo_write` excepted
  so plans can be recorded). The mode persists in the session file, syncs
  to the Tab label on load, and ships a prompt preamble so the model does
  not spam denied calls.
- Per-action approvals: per-category `approval_exec/write/read/network/
  mcp/plugin` levels (`allow|ask|deny`, ask by default for exec, write,
  MCP, and plugins) plus `command_allowlist`/`command_denylist` regexes.
  `ask` calls pause on an inline card (`y` once, `a` always this run,
  `n` deny); enforcement re-runs inside `execute`, so no path bypasses it.
- Visible task list: the model-maintained `todo_write` tool renders a
  collapsible TASKS section with footer `tasks done/total` counts.
- Ghost-text completion: `!` and `/sh` lines debounce into a local
  `llama-server` sidecar running Qwen2.5-Coder-0.5B (Q8_0 GGUF,
  ~350ms warm on M1, ~578MB RSS). Tab accepts, Esc dismisses, all
  failures fall back silent; `/completion [status|start|stop]` manages it.

### Removed
- The Python compatibility layer: `execute_python` tool,
  `src/python_bridge.rs`, the `bridge/` reference script, `/approve`
  and `/bridge` commands, the `r105 bridge` subcommand, and the
  `python_bridge_command` / `auto_approve_execute_python` config keys.
  Stale keys warn as unknown keys. `execute_rust` remains the sandboxed
  code-execution tool.

## [1.4.0] — 2026-09-15

### Added
- Per-section transcript expansion: collapsed tool sections render as one
  line with a stable `[n]` gutter id, and `/expand [n|all|none]` opens or
  closes them. Failed tool calls always stay expanded.
- Checkpoints and `/rewind [n]`: `/clear` and `/compact` save a checkpoint
  first (kept to the last 10), and `/rewind` restores an earlier one after
  backing up the current work, with a boundary notice marking the restore
  point.
- Safer compaction: pair-aware context splits, rejection of empty
  summaries, and `/compact to retry` after a failed compaction.
- Plugin `before_tool`/`after_tool` hooks: declaring plugins can deny a
  tool call with a visible reason or rewrite its arguments and result.
  Hook failures fail the tool visibly instead of silently.
- Session parents and `/session tree`: sessions record their parent,
  `/session fork <name> [turns]` snaps to an exchange boundary with
  tool-pair repair, and `/session tree` shows the fork hierarchy.
- `#` classify prefix: a `#` line is classified as a shell one-liner
  (prefilled after `!` for review), an agent prompt (sent as-is), or
  ambiguous (hint in the status line). Nothing executes unseen.
- Palette recency and live state: recently used commands rank higher
  (typo'd input never pollutes history) and running sessions show
  `· now`/`· active` badges.
- Tinted status line (muted/success/error) and folding of repeated
  consecutive notices (`message (×N)`).

## [1.3.0] — 2026-09-15

### Added
- Markdown-backed custom slash commands: `name.md` files in
  `~/.config/r105/commands/` (global) or `<workspace>/.r105/commands/`
  (project) become `/name` commands with `$1`/`$@`/`${N:-default}`
  argument substitution, palette listing, `/help`, and `/commands`
  management. Built-ins win name collisions and shadowing is reported.
- First-argument value completion (`/theme <Tab>`, `/skill use <Tab>`,
  `/session load <Tab>`, …) with a Tab-accept popup.
- Did-you-mean suggestions for unknown slash commands.
- `/sh <request>` drafts one shell command from plain words via the
  model and prefills the composer with `!command` for review; nothing
  runs without an explicit Enter.

## [1.2.0] — 2026-09-14

### Added
- The `/models` picker shows each model's provider load state, and the
  connect flow warns when a pick needs a first-use load.
- A request with no first token after 15 seconds notes once that the
  server may be loading the model, covering prompts and compactions.

## [1.1.0] — 2026-09-14

### Added
- `@file` references with fuzzy workspace completion (Tab accepts); attached
  files and directory listings join the conversation as context.
- `!` shell prefix runs commands inside the sandbox boundary and attaches
  their output to the conversation.
- New slash commands: `/editor` (compose in `$EDITOR`), `/settings` (one
  overlay for theme, permissions, reasoning, and toggles), `/thinking`
  (show or hide thinking blocks), `/attention` (completion bell toggle),
  `/undo` (drop the last exchange and restore its prompt), and `/redo`.
- Steering: Enter while a request is active cancels it and jumps the queue;
  Alt+Enter while busy queues a follow-up instead.
- Terminal bell when a request fully settles, with `attention_bell` config.
- Footer session token telemetry (`↑in ↓out`), git branch, and input hints;
  the header shows the sandbox backend and skill count.
- `keybindings` config remaps the five Ctrl shortcuts (`cancel`, `details`,
  `tasks`, `history`, `redraw`) with `ctrl+<letter>` specs.
- `/session fork <name>`, `/copy [n]` for the nth fenced code block, and
  opt-in mouse wheel scrolling via the `mouse` config key.

### Changed
- Reasoning-only replies are wrapped in `<thinking>` blocks so the transcript
  can hide them or collapse them to two lines.
- Tool errors auto-expand even when tool details are collapsed.
- The tool-round limit reports the skipped calls instead of a bare Done.
- Failed prompts are preserved in the composer with `/retry` recovery.
- Tool names appear in the status bar and transcript; the model picker marks
  the active model and the theme picker previews live.
- The event loop uses an async `EventStream` instead of blocking polls.

### Fixed
- Session search no longer risks a panic on Unicode text and matches
  case-insensitively beyond ASCII.
- Session files with unknown message roles are rejected instead of poisoning
  the backend payload.
- llama.cpp detection requires the provider id instead of matching `:8080`
  in any URL; router inference parses the real URL port.
- `safe_path` no longer creates parent directories as a side effect.
- Calculator power towers and sign chains respect the nesting depth guard.
- `execute_rust` maps workspace paths for the Docker `/workspace` mount.
- Malformed MCP `config.json` warns instead of silently dropping servers.
- Native plugins are gated by the permission posture and run confined to
  the workspace with a sanitized environment.

## [1.0.2] — 2026-09-14

### Added
- Endpoint prompts in the provider menu for local and self-hosted
  `llama-router`, llama.cpp, Ollama, LM Studio, and vLLM connectors.
- LAN hostname/IP support for local inference providers, with each provider's
  loopback URL retained as the empty-input default.

### Fixed
- Selecting llama.cpp from the TUI no longer immediately attempts only
  `127.0.0.1:8080`; it now lets the user enter and validate the server URL
  before model discovery.

## [1.0.1] — 2026-09-12

### Added
- Optional external Python compatibility bridge for legacy `execute_python`
  workflows, with an approval gate, versioned JSON protocol, and a stdlib-only
  reference implementation kept outside the native binary.
- Restored the legacy `/state`, `/history`, `/quality`, `/json`, `/max`,
  `/config`, `/autocompact`, `/reasoning`, `/permissions`, `/approve`,
  `/preview`, and `/bridge` command surface in the native TUI.

### Fixed
- The native TUI now honors `--yes` for the Python approval gate while keeping
  the configured permission posture and sandbox boundary visible.

## [1.0.0] — 2026-09-12

### Added
- Rust native implementation of the r105 AI harness with a Ratatui TUI,
  Tokio async runtime, OpenAI compatible direct/router backends, streaming SSE,
  model discovery, sessions, exports, skills, plugins, MCP, tools, and doctor
  diagnostics
- Guided /connect flow for OpenCode Zen, OpenCode Go, llama.cpp, llama-router,
  Ollama, LM Studio, vLLM, OpenAI, Groq, OpenRouter, DeepSeek, Together, and
  custom endpoints
- Native execute_rust tool with sandbox selection, cancellation, bounded
  arithmetic, workspace containment, DNS/IP SSRF controls, and redirect checks
- Native executable plugin protocol and live MCP tools/list discovery with
  /mcp reconnect
- Native release artifacts for Linux x86_64/aarch64, macOS x86_64/arm64,
  Windows x86_64/arm64, FreeBSD amd64, Ubuntu/Debian, Arch, Fedora, and Alpine

### Changed
- Replaced the Textual/Python runtime with one Rust executable and a smaller
  dependency surface
- Redesigned the TUI around a persistent composer, explicit build/plan/ask
  modes, focused provider/model pickers, visible queue and context state,
  cancellable work, and a scroll-safe command palette
- Replaced Python optional exporters with dependency-free Markdown, text, JSON,
  HTML, and PDF exporters
- Preserved the existing config and session JSON shape where practical, with
  versioned atomic session writes and __autosave__ on exit
- Homebrew and Scoop distributions now consume platform binaries instead of a
  Python virtual environment and dependency resource tree

### Fixed
- Alpine musl packaging uses the Rust 1.88 toolchain required by the native
  crate instead of Alpine's older repository compiler
- Cross compiled Linux aarch64 and Windows ARM artifacts are no longer run on
  incompatible x86 release runners during smoke testing

### Removed
- Embedded Python runtime, Python plugin loading, and execute_python
- PyPI, PyInstaller, and Python-only package build steps

## [0.8.3] — 2026-09-11

### Added
- Guided `/connect` setup in the TUI with predefined local and cloud providers,
  session-only API-key entry, live model discovery, and model selection
- OpenCode Zen and OpenCode Go connection presets

### Changed
- Provider metadata is saved without credentials so the selected connection can
  be restored on the next launch when its environment key is available

## [0.8.2] — 2026-09-11

### Fixed
- Slash-command palette navigation now keeps the selected row clear of the
  bottom border, including on Windows terminals
- Alpine package smoke validation now checks the native musl binary without
  requiring dependency resolution in an empty offline root

### Changed
- Release documentation now lists the published Alpine Linux x86_64 package
  alongside the complete binary and native package compatibility matrix

## [0.8.1] — 2026-09-11

### Fixed
- CI type checking for provider switching and live client reconfiguration
- Alpine package signing during temporary repository index generation
- Release workflow reliability for versioned release notes and authenticated
  Homebrew formula synchronization

## [0.8.0] — 2026-09-11

### Added
- `/connect` and `/provider` commands for switching between llama.cpp,
  llama-router, Ollama, LM Studio, vLLM, and supported cloud or custom
  OpenAI-compatible providers from the live TUI
- llama.cpp direct-provider preset at `http://127.0.0.1:8080/v1`, including
  support for base URLs that already contain the `/v1` API prefix

### Fixed
- Slash-command palette navigation now scrolls the selected command into view
  when the command list extends below the docked palette

## [0.7.0] — 2026-09-11

### Added
- Typed `Client` facade with stable `chat()`, `stream_chat()`, and
  `list_models()` methods, while preserving concrete backend compatibility
- Correlation IDs on `ChatState`, backend request headers, structured logs, and
  local tool executions
- Atomic, fsynced session writes; the TUI status bar now reports backend health,
  sandbox backend, and workspace writability
- Independent slash-command parser/registry, formal `Tool` protocol, and
  dedicated tool-security and sandbox-profile modules
- Config-schema consistency validation in startup and CI, plus the
  `benchmarks/token_estimation.py` tiktoken comparison utility
- MCP transport code is imported lazily, so ordinary tool startup does not
  load the stdio/SSE manager until MCP is configured or called
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
- `/config reload` for applying supported config changes during a live session
- `r105 doctor`: environment diagnostics (Python/config/sandbox/backend/
  workspace/skills/API keys) with per-check pass/fail and exit status
- `execute_python` confirmation gate: one-time per-session approval via
  `/approve execute_python`, `--yes` CLI flag, or
  `auto_approve_execute_python` config key (sandbox still applies)
- Web tool rate limiting: 30 searches / 60 fetches per rolling minute
  (cache hits don't consume budget)
- `Ctrl+X` / `cancel_tools` cancels a waiting local-tool batch
- The status bar renders context usage as a visual progress bar
- Release artifacts now include `.deb`, `.pkg.tar.zst`, `.rpm`, FreeBSD
  `.pkg`, Alpine `.apk`, Unix `.tar.gz`, and Windows x86_64/ARM64 `.zip`
  packages
- Release metadata validation checks the tag, project version, runtime version,
  and changelog section before publishing
- SSE streaming robustness: `event:` tracking with `event: error` surfacing
  as `RouterAPIError`, malformed-frame counting with structured logging,
  and exponential-backoff retries for transient pre-stream failures
  (connect errors, timeouts, HTTP 5xx)
- Sandbox fallback transparency: `detect_backend_with_reason()` /
  `get_fallback_reason()` explain downgrades; a startup stderr warning and a
  TUI status-bar `⚠ sandbox=<backend>` segment surface weak backends
- Plugin validation: `register()` signature checks and tool-definition schema
  validation report specific errors through `/plugin reload`
- Ctrl+T tool inspector: modal screen listing every tool call with arguments
  and result previews, with fuzzy filtering
- `convert` tool: unit conversion across length, mass, time, data, speed,
  volume, and temperature; `calculate` gains math functions and pi/e/tau
- Session diffing: `/session load` previews unsaved messages and changed
  settings before restoring
- Collapsible long tool results with diff coloring and code highlighting

### Fixed
- Web SSRF bypasses: redirects are checked before every hop, ambient proxies
  are disabled for web tools, and the custom transport connects only to the
  exact public IP returned by its validated DNS resolution
- `execute_python` dispatch now honors explicitly enabled plugin overrides while
  preserving the built-in sandbox profile path
- `calculate` now bounds AST depth, node count, numeric intermediates, powers,
  and factorial arguments to prevent resource-exhaustion expressions
- Removed the stale module-level `TOOL_DEFINITIONS` snapshot; definitions are
  read from the live registry on every request
- Removed obsolete `#rag-sources` selectors from all bundled themes
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
- MCP initialization now reports the actual r105 runtime version

### Changed
- Standalone release assets are archived with platform-specific extensions:
  `.tar.gz` for Unix targets and `.zip` for Windows
- The Linux release binary is built on Ubuntu 22.04 as the common glibc
  baseline for Ubuntu, Arch, and Fedora packages
- Slash-command handlers share `_apply_bool_toggle` / `_parse_choice`
  helpers; `ChatScreen.on_unmount` cancels the in-flight worker
- Structure (no behavior change): `client.py` split into `payload.py`
  (wire format), `sse.py` (streaming core), and `r105/backends/`
  (`base`/`direct`/`router`); `ToolRegistry` moved to `registry.py`;
  `web_search`/`web_fetch` implementations moved to `tools_web.py`;
  tool-loop mechanics extracted to `tool_loop.py`; command handlers take
  a single `CommandContext` instead of five positional arguments

### Removed
- RAG subsystem: `/rag` commands, `r105 ingest`/`search` CLI subcommands,
  `--rag` flag, `ChatState.rag`, router `/rag/*` endpoints, and RAG docs.
  The router backend now covers profiles + metadata only
- Dead code: duplicate SSRF/HTML/DDG helpers folded into `r105/tools_web.py`
  (which nothing imported), `_SAFE_ENV_PREFIXES_LEGACY` alias; web tools
  send a versioned `r105/{__version__}` User-Agent

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

[Unreleased]: https://github.com/bnelabs/r105/compare/v1.0.1...HEAD
[1.0.1]: https://github.com/bnelabs/r105/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/bnelabs/r105/compare/v0.8.3...v1.0.0
[0.8.3]: https://github.com/bnelabs/r105/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/bnelabs/r105/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/bnelabs/r105/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/bnelabs/r105/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/bnelabs/r105/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/bnelabs/r105/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/bnelabs/r105/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/bnelabs/r105/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/bnelabs/r105/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/bnelabs/r105/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/bnelabs/r105/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/bnelabs/r105/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/bnelabs/r105/releases/tag/v0.1.0
