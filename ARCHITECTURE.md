# r105 architecture

r105 is a Rust terminal AI harness. The binary owns the terminal UI, backend protocol, tool loop, persistence, security checks, native plugins, and MCP transports.

```
┌───────────────┐       HTTP/SSE       ┌─────────────────────────┐
│ Ratatui TUI   │ ◄──────────────────► │ OpenAI compatible API   │
│ transcript    │                      │ llama-router / llama.cpp│
│ composer      │                      │ Ollama / cloud provider │
└──────┬────────┘                      └─────────────────────────┘
       │
       ├── native tool loop
       │    ├── workspace file tools
       │    ├── bounded arithmetic and conversion
       │    ├── SSRF checked web tools
       │    └── sandboxed Rust execution
       │
       ├── native executable plugins
       └── MCP stdio or HTTP servers
```

## Module boundaries

| Module | Responsibility |
| --- | --- |
| main.rs | CLI parsing, startup configuration, command dispatch |
| app.rs | doctor diagnostics |
| config.rs | paths, schema validation, atomic JSON writes |
| provider.rs | provider presets, environment credentials, connection selection |
| backend.rs | OpenAI compatible payloads, health, models, profiles, streaming |
| sse.rs | split frame parsing and streaming tool call accumulation |
| model.rs | messages, chat state, usage estimates, skill injection |
| ui.rs | Ratatui event loop, overlays, palette, transcript, cancellation |
| command.rs | slash registry, shell word parsing, fuzzy ranking, visible scrolling |
| tool.rs | native tool schemas, dispatch, parallel tool execution |
| security.rs | workspace containment, DNS/IP blocklist, input limits |
| sandbox.rs | nsjail, bubblewrap, Docker, or timeout fallback execution |
| plugin.rs | executable plugin manifests and JSON stdin/stdout protocol |
| mcp.rs | MCP initialize, tools/list, tools/call, and in memory discovery cache |
| session.rs | versioned session compatibility and atomic persistence |
| export.rs | Markdown, text, JSON, HTML, and dependency free PDF output |

The code is deliberately a binary crate at this stage. Keeping the modules private prevents accidental coupling while the Rust API settles. The wire and persistence types are serde based so the old config and session formats remain readable.

## Request flow

A normal prompt follows this path:

```
composer
  -> slash command parser or prompt
  -> ChatState::prompt_messages()
  -> Backend::stream_chat()
  -> SseParser
  -> transcript token events
  -> ChatResult
  -> tool calls, if present
  -> parallel ToolContext workers
  -> tool messages
  -> Backend::stream_continue()
  -> final transcript message
```

The backend sends the same OpenAI compatible request shape for direct and router connections. Router only adds the profile, quality, and routing metadata used by llama-router. The client uses a shared reqwest connection pool and disables automatic redirects for model traffic.

Streaming is cancellation aware. A cancellation token is selected against both the request body and the response byte stream. The UI keeps pending prompts in a queue, and Ctrl+X or Esc cancels the active request and its local tool batch.

## UI model

The UI is designed around the working loop used by modern coding harnesses:

- the transcript remains the main surface;
- the composer is always available;
- slash commands open a fuzzy palette instead of a separate screen;
- provider and model setup use focused, scrollable pickers;
- build, plan, and ask are explicit modes;
- context usage, sandbox selection, queue length, and cancellation state stay visible in the footer;
- tool output is collapsed until Ctrl+O expands details.

The command palette uses one shared ensure_visible calculation for keyboard movement and rendering. This keeps the selected item visible when the list extends below the terminal and avoids the Windows bottom border failure from the previous UI.

## Tool and security flow

Every model tool call is parsed into a native ToolCall. Built in tools validate their input before touching the filesystem, network, or process table.

Workspace paths are resolved against a canonical workspace root. Absolute paths, parent traversal, and symlink escapes fail. Web requests allow only HTTP and HTTPS, reject credentials and metadata hostnames, resolve all addresses, reject any blocked answer, pin the chosen public address for the request, and validate every redirect target before the next request.

The expression parser uses a bounded recursive descent grammar. It caps input size, nesting, numeric literals, powers, intermediate values, and factorial arguments. Tool output is truncated before it enters the transcript or a follow up model request.

Rust source sent to execute_rust is written under the workspace .r105/runs directory, compiled, run, and removed. The process is launched through the selected sandbox wrapper with a cleared environment and a hard timeout. The rlimit fallback provides process timeout and output bounds; nsjail, bubblewrap, or Docker provide stronger OS isolation when installed.

## Providers

provider.rs separates user facing preset selection from transport details. A preset contains its backend kind, default URL, credential environment variable, and whether a key is required. The guided /connect flow:

1. selects a preset;
2. asks for a missing cloud key before making a request;
3. creates an in memory candidate connection;
4. calls /v1/models;
5. lets the user choose a returned model;
6. persists provider metadata, URL, backend, and model without credentials.

The llama.cpp preset points at its OpenAI compatible /v1 endpoint. OpenCode Zen and OpenCode Go use their own base URLs and OPENCODE_API_KEY.

## Persistence

Config writes are serialized to a temporary file, fsynced, renamed into place, and followed by a directory sync where supported. Session writes use the same atomic path.

Session files have a version field and preserve the history, model, context window, active skills, skill parameters, quality/profile settings, and trace id. The loader accepts old messages whose content is null or a structured JSON value and converts them to display text.

The UI writes an __autosave__ session before leaving when the transcript is nonempty. Explicit /session save remains available for named sessions, search, diff, load, and delete operations.

## Extension protocols

Native plugins are described by JSON files in the configured plugins directory. A plugin executable receives one JSON request on stdin and returns one JSON object on stdout. Tool names are namespaced by plugin.

MCP servers are configured in config.json. /mcp reconnect performs the initialize handshake and tools/list discovery for one or all configured servers. Discovered schemas stay in memory and are included with the next prompt. Tool calls create a short lived stdio process or HTTP request, depending on transport.

Both extension paths are explicit and local. The Rust binary never embeds a Python interpreter.

## Release architecture

Cargo produces one native executable per target. Unix archives use tar.gz and Windows archives use zip. Linux distribution packages reuse the x86_64 GNU binary for Ubuntu/Debian, Arch, and Fedora; FreeBSD and Alpine build natively for their different ABIs. macOS arm64 and Windows arm64 are separate release assets for Apple silicon and Windows on ARM.
