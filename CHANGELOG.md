# Changelog

All notable changes to Rova are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- Plugin system (`rova/plugins.py`): custom tools from Python files in `~/.config/rova/plugins/`
- MCP (Model Context Protocol) support (`rova/mcp_client.py`): JSON-RPC over stdio
- Session management: `/session save|load|list|delete`, `--session` CLI flag
- Conversation export: `/export markdown|json|html`

## [0.1.0] — 2025-05

### Added
- Initial release: TUI frontend for llama-router
- Interactive chat with streaming SSE support
- Built-in tools: `execute_python`, `write_file`, `read_file`, `list_files`, `web_search`, `web_fetch`, `get_time`, `calculate`, `system_info`
- Slash-command system with fuzzy command palette
- Theme system with 4 built-in themes (rova, dracula, solarized-dark, high-contrast)
- Skills system with markdown skill files and parameter substitution
- RAG integration with ingest, search, list, delete commands
- File explorer sidebar
- Token usage estimation and auto-compaction at 80% context
