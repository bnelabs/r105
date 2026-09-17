# r105 — Beyond the prompt

r105 is a local-first AI harness for terminal work. It connects to local or cloud OpenAI-compatible backends, keeps your sessions on your machine, and gives the model bounded tools for files, calculations, web research, and native Rust execution.

The current release is 2.5.1. The application is written in Rust: installed builds are single native executables and do not require Python, Node.js, or a runtime virtual environment.

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust">
  <img src="https://img.shields.io/github/v/release/bnelabs/r105?color=cba6f7" alt="Release">
  <img src="https://img.shields.io/badge/AI%20harness-local%20first-blue" alt="AI harness">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="License">
</p>

## Supported systems

| Operating system | Architectures | Distribution |
| --- | --- | --- |
| Linux with glibc 2.35 or newer | x86_64, aarch64 | tar.gz; Ubuntu/Debian deb, Arch pkg.tar.zst, Fedora/RHEL rpm for x86_64 |
| Alpine Linux with musl | x86_64 | native apk package |
| macOS | x86_64, arm64 | tar.gz; arm64 supports Apple silicon; local `.app` bundle on macOS |
| Windows | x86_64, arm64 | zip; arm64 supports Windows on ARM |
| FreeBSD 14 | amd64 | native pkg package |

The Linux GNU binary is built against an Ubuntu 22.04 baseline. Ubuntu 22.04 or newer, current Arch Linux, and current Fedora or RHEL compatible systems are supported. Alpine uses a separate native musl build because a glibc binary is not a portable Alpine package.

The release archive names make the target explicit:

- r105-linux-x86_64.tar.gz
- r105-linux-aarch64.tar.gz
- r105-macos-x86_64.tar.gz
- r105-macos-arm64.tar.gz
- r105-windows-x86_64.zip
- r105-windows-arm64.zip

Every release includes SHA256SUMS. The macOS application bundle is a convenience build for local use; public distribution still requires platform signing and notarization.

## Installation

### Homebrew

The Homebrew tap installs the native binary:

```sh
brew install bnelabs/tap/r105
```

### Release archives

Download the archive for your operating system and architecture from GitHub Releases, verify its checksum, then place the executable on your PATH.

```sh
VERSION=2.5.1
curl -LO https://github.com/bnelabs/r105/releases/download/v$VERSION/r105-macos-arm64.tar.gz
curl -LO https://github.com/bnelabs/r105/releases/download/v$VERSION/SHA256SUMS
grep 'r105-macos-arm64.tar.gz$' SHA256SUMS | shasum -a 256 -c -
tar -xzf r105-macos-arm64.tar.gz
install -m 755 r105 /usr/local/bin/r105
```

### Native Linux packages

Use the package that matches the distribution:

```sh
VERSION=2.5.1

# Ubuntu / Debian
sudo apt install "./r105_${VERSION}_amd64.deb"

# Arch Linux
sudo pacman -U "r105-${VERSION}-1-x86_64.pkg.tar.zst"

# Fedora / RHEL compatible systems
sudo dnf install "./r105-${VERSION}-1.x86_64.rpm"

# FreeBSD
sudo pkg add "./r105-${VERSION}-freebsd-amd64.pkg"

# Alpine Linux
sudo apk add --allow-untrusted "./r105-${VERSION}-r0.apk"
```

The Alpine package is unsigned for public release convenience; verify SHA256SUMS before installing it. Distributions with signed package repositories can rebuild the recipe and sign it with their own key.

### Install from source

Install the Rust toolchain from rustup or your operating system package manager:

```sh
git clone https://github.com/bnelabs/r105.git
cd r105
cargo install --path . --locked
```

### Docker

The image is built from the Rust source and contains the native r105 executable:

```sh
docker build -t r105 .
docker run -it --rm \
  -v ~/.config/r105:/root/.config/r105 \
  -v ~/r105-workspace:/root/r105-workspace \
  r105
```

For a local llama-router stack:

```sh
docker compose up -d llama-router
docker compose run --rm r105
```

## Quick start

Choose the surface that matches how you want to work:

```sh
r105                 # interactive workspace in the terminal you already use
r105 window          # independent native window with its own terminal surface
r105 terminal        # standalone shell session with block tracking
```

`r105` is the normal interactive workspace inside the current terminal. It
provides persistent tabs, split panes, session management, completion,
approvals, and exports. `r105 window` opens a separate desktop window with its
own PTY and inline AI panel. `r105 terminal` is shell-only when you need a
standalone PTY without the workspace UI. `r105 run` is an explicit alias for
the default `r105` command.

The default connection is a local llama-router at http://127.0.0.1:8010. You can also send one prompt and exit:

```sh
r105 send "explain quicksort in three sentences"
```

Check the connection or diagnose the local installation:

```sh
r105 health
r105 doctor
r105 config-schema
```

## Connecting a provider

The TUI uses a guided connection flow. Type /connect, choose a predefined provider, enter a key when required, wait for the live model list, and choose a model. API keys remain in memory and are never written to config.json.

The same flow is available in scripted form:

| Command | Backend | Base URL | Key |
| --- | --- | --- | --- |
| /connect opencode | direct | https://opencode.ai/zen/v1 | OPENCODE_API_KEY |
| /connect opencode-go | direct | https://opencode.ai/zen/go/v1 | OPENCODE_API_KEY |
| /connect llamacpp | direct | menu asks; local default http://127.0.0.1:8080/v1 | none |
| /connect ollama | direct | menu asks; local default http://127.0.0.1:11434/v1 | none |
| /connect lmstudio | direct | menu asks; local default http://127.0.0.1:1234/v1 | none |
| /connect vllm | direct | menu asks; local default http://127.0.0.1:8000/v1 | optional OPENAI_API_KEY |
| /connect router | router | menu asks; local default http://127.0.0.1:8010 | none |
| /connect openai | direct | https://api.openai.com/v1 | OPENAI_API_KEY |
| /connect groq | direct | https://api.groq.com/openai/v1 | GROQ_API_KEY |
| /connect openrouter | direct | https://openrouter.ai/api/v1 | OPENROUTER_API_KEY |
| /connect deepseek | direct | https://api.deepseek.com/v1 | DEEPSEEK_API_KEY |
| /connect together | direct | https://api.together.xyz/v1 | TOGETHER_API_KEY |
| /connect url https://host/v1 | direct | custom | optional OPENAI_API_KEY |

OpenCode and other cloud providers ask for the key before the model request when the corresponding environment variable is absent. A key supplied in the TUI is only held by that process. Provider metadata, URL, backend, and selected model can be saved for the next launch without saving credentials.

Local and self-hosted providers (`llama-router`, `llama.cpp`, Ollama, LM Studio, and vLLM) open an endpoint dialog from the provider menu. Empty input keeps the local default; replace `127.0.0.1` with a LAN hostname or IP for a server on your network. r105 probes the endpoint before showing its model picker and saving the connection.

### llama.cpp

Start an OpenAI-compatible llama-server:

```sh
llama-server -m /path/to/model.gguf --host 127.0.0.1 --port 8080
r105
```

In the TUI, open `/connect` and choose `llama.cpp`. The menu asks for the base URL and uses `http://127.0.0.1:8080/v1` when submitted empty. For a server on your LAN, enter an address such as `http://192.168.1.50:8080/v1`; r105 checks `/v1/models`, then lets you choose a model and saves the working endpoint. The provider supports model listing, SSE streaming, tool calls, prompt caching with /cache-prompt on, and model switching.

### Environment variables

| Variable | Purpose |
| --- | --- |
| R105_URL | Startup base URL |
| R105_MODEL | Startup model override |
| OPENAI_BASE_URL | OpenAI-compatible fallback URL |
| OPENAI_API_KEY | OpenAI, vLLM, or custom endpoint key |
| OPENCODE_API_KEY | OpenCode Zen and OpenCode Go key |
| GROQ_API_KEY | Groq key |
| OPENROUTER_API_KEY | OpenRouter key |
| DEEPSEEK_API_KEY | DeepSeek key |
| TOGETHER_API_KEY | Together AI key |
| R105_CONFIG_DIR | Alternate configuration directory |
| R105_STRICT_CONFIG | Fail on unknown config keys when set to 1 |

## TUI workflow

The TUI keeps the current task visible, moves setup into focused
overlays, and renders borderless: a tab bar, the conversation, and the
composer.

```
 r105  1 night-run  2 parallel  +     build · llamacpp · model
> list the rust files

● Sure — here they are.

  ◌ thought · 2 lines · click or /expand to inspect

──────────────────────── Shell · Enter runs
> git checkout main▌
 Ready  ↑1.2k ↓380  ⎇main
 ~/r105-workspace · nsjail  context ██░░ 34% 12k/32k
```

Conversation turns stay answer-first: `>` marks the prompt, `●` marks the
assistant reply, and reasoning/tool output stays in compact expandable rows
(`click` or `/expand`) instead of printing internal traces into the conversation.

- Tabs are session-backed: Ctrl+Shift+T opens one on a fresh session, Ctrl+Shift+W closes it (or the focused pane in a split), Ctrl+Tab and Ctrl+Shift+Tab cycle, Alt+1..9 selects, Alt+Shift+←/→ reorders, and clicking the bar works too. The active chip exposes `×` for direct closure. Switching autosaves the live session first; the bar persists across restarts in `tabs.json`.
- Panes use a persistent nested layout inside a tab: Ctrl+Shift+D opens a fresh session to the right, Ctrl+Shift+E stacks one below, Ctrl+Shift+←/→ (or Ctrl+Alt+←/→) moves focus, clicking a pane focuses it, Ctrl+Shift+Enter temporarily maximizes the focused pane, and Ctrl+Shift+W closes it (the last pane closes the tab). Split view uses compact pane headers and tree-owned dividers; the active pane and its adjacent divider get the accent marker while background panes stay quiet. Every pane is a live session: a request keeps streaming while you work in a sibling, the header shows the session name plus `…` while running and `•` when a background pane finished, and up to four panes fit. Tab switches stash the whole layout — each pane autosaves its own session and the exact split, zoom state, and focus restore together.
- Ctrl+R searches shell history in reverse: the current draft seeds the query, typing narrows (case-insensitive), ↑↓/wheel move the highlight, the composer previews the match live, Enter accepts it without running, Esc restores the draft. Clicking a row selects it; clicking the highlighted row accepts.
- Type / or press Ctrl+P to open the action palette (`*` marks saved workflows from Markdown files). Click a row to pick it; click again to fill it in.
- Press Ctrl+B for the session pane: a left column listing saved sessions (`●` marks the loaded one) and recent workspaces with the live one pinned first. ↑↓ move, Enter opens (`+ New session` starts fresh), `d` deletes a saved file, typing filters, click selects (click again opens), wheel scrolls, Esc returns to the composer. Switching sessions or starting fresh autosaves the live session first, so the pane never discards work; `/session` and `/workspace` stay for everything scripted.
- After a space, `/command <Tab>` completes argument values (models, themes, skill and session names, …); unknown commands suggest the closest match.
- `#` turns plain words into a shell command draft for review (e.g. `# list large files`) — Enter runs it, nothing executes unseen.
- Up and Down keep the selected command inside the visible palette window, including when the list is taller than the terminal.
- /connect and /models use scrollable provider and model pickers.
- Tab cycles through build, plan, and ask modes. Modes are enforced:
  plan allows reads and web research but refuses writes and execution,
  ask answers without tools; the session file remembers the mode.
- Type a shell command and pause: the cascade suggests the rest
  dimmed. Shell-history frequency comes
  first (same-directory runs win), context-aware arguments second
  (`git checkout` offers branches, `npm run` offers scripts, `make`
  offers targets, `ssh` offers hosts, `kubectl get` offers resource
  types, flags complete after `-`, nested verbs complete deeper like
  `gh issue list`), command names (builtins plus PATH executables)
  third, path top-hit last. Shell aliases (`~/.bashrc`, `~/.zshrc`)
  and git aliases expand before lookup, so `g st` completes as git.
  No weights, no server, microseconds. Tab or → accepts
  (continuations chain), Ctrl+→ takes one word, Esc dismisses until
  the next edit. The composer colors shell input as you type
  (commands cyan, flags yellow, strings green, operators magenta),
  keeps a visible cursor cell, and titles itself `Shell · Enter runs`
  when the line will execute.
- Enter on a shell-looking line runs it: `git status` executes
  without any marker. Prose and questions still go to the model (a
  trailing `?` is the sure way to ask), `!` forces execution for lines
  the detector reads as prose, `# <goal>` drafts a command, and
  `cd <dir>` retargets the workspace like a terminal.
- When nothing local extends a shell line and the composer sits idle,
  the active model may propose the rest (`ai_suggest` in config, or
  `/completion ai off`, disables it). Idle-only, debounced, one flight
  per input; a dismissal sticks.
- Tab on a shell line opens a completion menu when ambiguous (history,
  subcommands, flags, branches, scripts, files — each tagged with its
  kind and a one-line description when curated; live pod, container,
  namespace, context, and unit values join from a background cache
  refreshed off the keystroke path). ↑↓ move, Tab fills, Enter runs,
  Esc closes, click selects (click again fills). One match applies at
  once without opening.
- A failed `!` line offers fixes when local rules match (mistyped
  command, git subcommand/branch/flag, missing upstream, `chmod +x`,
  mistyped path): `Did you mean ...?` — → applies the best into the
  empty composer, alternates list in the session view, any edit drops it.
- Up and Down at an empty-argument composer walk earlier user turns like
  a shell: first Up stashes the live draft, Down past the newest
  restores it, Esc restores, any edit adopts. The composer title shows
  the walk while it is active.
- Every message is a grouped block (`#n` in its header,
  ✓/✗ on tool results): click a block header to collapse it,
  Alt+Up/Down jumps between blocks, `/filter <n> <pattern>` narrows
  long output (`--regex`, `--case`, `--invert`, `--context N`, clear
  with the shown command), `/block [n]` lists or describes blocks,
  `/rerun [n]` resubmits an earlier prompt or `!` line, and
  `/copy out [n]` copies a block (filtered view when a filter is set).
- Destructive tools pause on an approval card (`y` once, `a` always this
  run, `n` deny) unless the per-category policy (`approval_exec`,
  `approval_write`, …) or `command_allowlist`/`command_denylist` says
  otherwise.
- The model can keep a visible task list (`todo_write` tool, TASKS
  section, `tasks done/total` in the footer) that collapses like any
  other section.
- Type @ to fuzzy-complete a workspace file; Tab accepts, Enter sends with the file attached as context.
- Start a line with ! to run a shell command in the sandbox; its output joins the conversation context.
- Start a line with # to describe what you want to do: the model drafts one shell command into the composer for review. Nothing executes unseen.
- The composer edits like a small IDE input: click to place the cursor, brackets and quotes close themselves, Ctrl+Left/Right (or Alt+F/B) jumps by word, Ctrl+A/E jumps to line ends, Ctrl+W/U/K deletes a word to the line start/end.
- Enter while a request is active steers it (the new prompt jumps the queue); Alt+Enter while busy queues a follow-up instead.
- Esc or Ctrl+X cancels the current request or local tool batch.
- Ctrl+O expands tool details; Ctrl+T shows active work. The six Ctrl shortcuts are remappable via `keybindings` in config.json.
- Mouse is on by default: wheel scrolls (Shift+wheel scrolls faster), click the composer to place the cursor, click a block header to collapse it. /mouse toggles capture; Shift+drag still selects natively. Ctrl+Home/End jumps to the top/latest, and End returns when scrolled up.
- PageUp and PageDown scroll a full page; Alt+Up/Down jumps between blocks.
- Saved workflows are Markdown files (`/workflows` lists them, `/commands reload` refreshes); run one as `/name [args]` or find it with Ctrl+P.
- Alt+Enter or Shift+Enter inserts a newline; Enter sends the prompt.
- Exiting with /exit or Ctrl+C saves an __autosave__ session when it is nonempty.

## Slash commands

| Command | Action |
| --- | --- |
| /help | Show commands and keybindings |
| /state | Show active settings and connection |
| /connect or /provider | Guided provider, credential, and model setup |
| /models | Refresh and choose models |
| /model [name] | Show or select the active model |
| /health | Check backend connectivity |
| /profiles | List llama-router profiles |
| /profile [name or auto] | Set or clear the router profile |
| /build, /plan, /ask | Switch working mode (enforced, persisted) |
| /completion [status\|clear\|ai on\|ai off] | Suggestion status; clear history; model ghost toggle |
| /history | Show a session preview |
| /skills | List Markdown skills |
| /skill use <name> [key=value] | Activate a skill |
| /skill show <name> | Display a skill |
| /skill drop <name> | Deactivate a skill |
| /skill clear | Deactivate all active skills |
| /compact | Summarize older conversation context |
| /tokens | Show context usage |
| /quality [fast, balanced, or best] | Set the router quality hint |
| /json [on or off] | Toggle JSON response mode |
| /max [tokens] | Set or clear the completion token limit |
| /cache-prompt [on or off] | Toggle llama.cpp prompt prefix caching |
| /config show or reload | Inspect or reload configuration |
| /clear | Clear the visible session |
| /workspace [path] | Show or change the workspace |
| /session save, load, list, search, delete, diff, fork, tree | Manage local sessions |
| /export markdown, text, json, html, pdf [path] | Export the session |
| /mcp list, tools, reconnect [server] | Inspect or rediscover MCP tools |
| /plugin list, reload | Inspect native executable plugins |
| /theme [name] | Show or switch the theme |
| /autocompact [on or off] | Toggle automatic context compaction |
| /reasoning [effort] | Set the reasoning effort hint |
| /thinking [on or off] | Show or hide the model's thinking blocks |
| /attention [on or off] | Toggle the bell when a request finishes |
| /mouse [on or off] | Toggle mouse capture |
| /commands [reload] | List saved workflows |
| /workflows [reload] | List saved reusable workflows |
| /settings | Change theme, permissions, reasoning, and toggles |
| /editor | Compose the prompt in $EDITOR |
| /permissions [posture] | Set the local tool permission posture |
| /preview <filename> | Preview a workspace file |
| /map | Show a compact workspace map |
| /diff | Show the Git workspace diff |
| /copy [n] | Copy the last response or its nth code block |
| /copy out [n] | Copy a block (filtered view when filtered) |
| /filter <block> <pattern> | Narrow one block's output (regex/case/invert/context flags) |
| /block [n] | List blocks or describe one |
| /rerun [n] | Resubmit an earlier prompt or `!` shell line |
| /tasks | Show active and queued work |
| /retry | Retry the last failed prompt |
| /undo | Remove the last exchange and restore its prompt |
| /redo | Re-apply the last undone exchange |
| /expand [n\|#block\|all\|none] | Expand or collapse sections |
| /rewind [n] | Restore an earlier checkpoint (current work is backed up first) |
| /exit | Save autosave and quit |

## Built in tools

The model can use these native tools:

| Tool | Purpose |
| --- | --- |
| execute_rust | Compile and run Rust in the configured sandbox |
| write_file | Write a workspace-relative file |
| edit_file | Replace one anchored span in a workspace file |
| apply_patch | Apply a structured Add/Update/Delete patch with @@ hunks |
| read_file | Read a workspace-relative file |
| list_files | List a workspace directory |
| get_time | Return the local system clock |
| calculate | Evaluate bounded arithmetic |
| convert | Convert common units |
| system_info | Return basic platform information |
| web_search | Search public web pages through DuckDuckGo |
| web_fetch | Fetch public HTTP(S) pages |
| todo_write | Replace the visible task list (all modes, never needs approval) |

Tool schemas are sent with each request. Tool calls are executed in parallel where possible, and individual failures are returned as tool results so one failure does not discard the rest of the batch.

## Security and permissions

r105 is local-first, but model-generated actions still cross explicit boundaries:

- Workspace file tools reject absolute paths, traversal, and symlink escapes.
- Web tools accept only HTTP(S), reject embedded credentials, block private and metadata addresses, pin the validated DNS address for the request, and validate every redirect before following it.
- Arithmetic limits bound expression size, recursion, powers, intermediates, and factorial arguments.
- execute_rust uses the selected sandbox backend, clears inherited environment variables, uses the workspace as its working directory, bounds runtime and output, and removes temporary artifacts.
- auto selects nsjail, bwrap, Docker, or the timeout based fallback. r105 doctor reports the selected backend.
- full-access, sandboxed, restricted, and off permission postures are represented in config. Native Rust execution is disabled under off.
- Rust plugins are operator installed executables and are namespaced as plugin_<plugin>_<tool>. They run with a sanitized environment and a 30 second response limit.

The fallback executor supplies timeout, output, workspace, and environment controls. Install nsjail, bubblewrap, or Docker when stronger OS isolation is required.

## Skills

Skills are Markdown prompt files in ~/.config/r105/skills/ by default.

```sh
mkdir -p ~/.config/r105/skills
cat > ~/.config/r105/skills/reviewer.md <<'EOF'
Review the requested change for correctness, security, and missing tests.
Return findings with file, line, impact, and a concrete fix.
EOF
```

Use it from the TUI:

```text
/skills
/skill use reviewer
/skill show reviewer
/skill drop reviewer
```

A skill can use literal {key} placeholders. Activate it with values such as /skill use reviewer scope=security; r105 substitutes those values into the Markdown system message. Skill names are restricted to one local Markdown filename.

## Custom commands

Custom commands are Markdown prompt files that become real slash commands — no plugin code needed.
A global directory (`~/.config/r105/commands/`) and a project directory (`<workspace>/.r105/commands/`)
are scanned for `name.md`; each file becomes `/name`.

```sh
mkdir -p ~/.config/r105/commands
cat > ~/.config/r105/commands/review.md <<'EOF'
---
description: Review the staged change
argument-hint: <scope>
---

Review the $ARGUMENTS for correctness, security, and missing tests.
Return findings with file, line, impact, and a concrete fix.
EOF
```

Typing `/review scope=security` expands the template and submits it as the prompt
(`$1`, `$@`/`$ARGUMENTS`, `${1:-default}`, and `${@:2}` slices are supported).
Custom commands appear in the `/` palette marked with `*`, accept `/help /review`,
and reload with `/commands reload` (also reloaded by `/config reload` and workspace
switches). Built-in commands always win a name collision; shadowed files are listed
by `/commands` instead of running silently.

## Native plugins

Plugins are executable programs described by JSON manifests in ~/.config/r105/plugins/. The manifest declares the executable and its tool schemas:

```json
{
  "name": "example",
  "command": "r105-plugin-example",
  "version": "1",
  "tools": [
    {
      "name": "hello",
      "description": "Return a greeting",
      "parameters": {
        "name": { "type": "string" }
      },
      "required": ["name"]
    }
  ]
}
```

The executable receives one JSON request on stdin and returns one JSON object on stdout:

```json
{"method":"call","tool":"hello","arguments":{"name":"Ada"}}
```

Return {"content":"..."} or {"result":"..."}. Plugin tools are exposed to the model as plugin_example_hello.

### Tool hooks

A manifest can declare `"hooks": ["before_tool", "after_tool"]` to observe, gate, or rewrite tool execution. Only plugins that declare hooks are ever spawned for them, and declared hooks see every tool call (there are no per-tool subscriptions):

```json
{
  "name": "example",
  "command": "r105-plugin-example",
  "version": "1",
  "tools": [],
  "hooks": ["before_tool", "after_tool"]
}
```

Before a tool runs (after argument repair), each `before_tool` plugin receives:

```json
{"method":"before_tool","tool":"write_file","arguments":{"path":"demo.txt"}}
```

Reply `{"deny": "reason"}` to abort the call with a visible error, or `{"arguments": {...}}` to replace the arguments (rewrites chain across plugins). After the tool runs, each `after_tool` plugin receives `{"method":"after_tool","tool":"...","arguments":{...},"result":...}` and may reply `{"result": ...}` to replace the result. Hook calls have a 5 second limit, and hook transport failures fail the tool visibly (`tool error: … hook …`) instead of silently.

## MCP

MCP servers are configured in ~/.config/r105/config.json:

```json
{
  "mcp_servers": [
    {
      "name": "filesystem",
      "transport": "stdio",
      "command": "mcp-server-filesystem",
      "args": ["/home/user/project"]
    },
    {
      "name": "remote",
      "transport": "http",
      "url": "http://127.0.0.1:3001/mcp"
    }
  ]
}
```

Use /mcp reconnect to initialize the configured servers and call tools/list. Discovered schemas are cached in memory and become available to the next prompt as mcp_<server>_<tool>. /mcp tools displays the schemas currently available. Stdio and request/response HTTP transports are supported.

## Configuration

The default file is ~/.config/r105/config.json:

See [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for the complete key
reference, persistence paths, precedence, and migration behavior.

```json
{
  "theme": "r105",
  "workspace": "/home/user/r105-workspace",
  "skills_dir": "/home/user/.config/r105/skills",
  "plugins_dir": "/home/user/.config/r105/plugins",
  "backend": "direct",
  "provider": "llamacpp",
  "url": "http://127.0.0.1:8080/v1",
  "model": "local-model",
  "auto_compact": true,
  "cache_prompt": true,
  "ai_suggest": true,
  "permission_posture": "sandboxed",
  "sandbox_backend": "auto",
  "timeout_seconds": 120
}
```

Context windows are detected automatically: a local llama.cpp server's
loaded window (`/props`) and per-model provider metadata (`/v1/models`)
feed the footer budget, `/tokens`, and auto-compaction. Override a model
or a family with `model_contexts` (`{"qwen3": 131072, "*": 32768}` — an
exact key wins, otherwise the longest key contained in the model id);
the global `context_tokens` stays as the fallback. Detected values are
never persisted.

Unknown keys are ignored with a warning for compatibility. Set R105_STRICT_CONFIG=1 to fail fast. r105 config-schema prints the JSON Schema used by the native loader.

## Sessions, tabs, and exports

Named sessions are JSON files in `~/.config/r105/sessions/`. The loader reads
legacy r105 0.8.x session shapes, accepts null or structured message content,
writes versioned files with atomic replacement, and preserves model, context,
active skills, skill parameters, mode, and trace metadata. The TUI stores its
tab bar in `~/.config/r105/tabs.json`; each tab records pane sessions, focus,
zoom, and the nested right/down split tree. Invalid or legacy tab files are
repaired to a safe single-pane layout.

The native window creates a separate `window-<id>` session unless an explicit
`--session <name>` is supplied. Its AI history is checkpointed while the
window is open and can be resumed with `r105 --session <name> window`.

```text
/session save night-run
/session list
/session search authentication
/session diff night-run
/session load night-run
```

Exports have no optional runtime dependencies:

```text
/export markdown workspace.md
/export text workspace.txt
/export json workspace.json
/export html workspace.html
/export pdf workspace.pdf
```

## CLI reference

```text
r105 [OPTIONS] [COMMAND]

Commands:
  run             Start r105 in the current terminal (the default)
  send           Send one prompt and exit
  health         Check the selected backend
  doctor         Diagnose config, sandbox, backend, and workspace
  profiles       Print llama-router profiles
  config-schema  Print or write the config JSON Schema
  sandbox        Run a command inside the sandbox boundary (dry-run tester)
  terminal       Open the PTY shell with block tracking; `-- <cmd>` runs once
  window         Open the native r105 terminal and AI window
```

## Terminal command

`r105 terminal` opens a persistent shell in a real PTY with a block
list. Each submitted line snapshots a block (command, working
directory, elapsed time); `Ctrl+B` toggles the block list, `Ctrl+Q`
quits. `r105 terminal -- <cmd>` runs one command in a fresh PTY and
prints its block (command, cwd, exit code, output tail).

Interactive blocks use OSC 7/133/633 shell integration for cwd, command
boundaries, and exit codes when the shell is recognized (bash, zsh, or
fish); other shells fall back to an honest completed status. The one-shot
path records exact exits independently. The interactive workspace, `send`,
and `sandbox` remain available as separate commands.

## Native window

`r105 window` opens r105 in its own OS window: a GPU-rendered terminal
hosting the same PTY shell core. Typing runs in the shell, resize reflows the
grid, and `Cmd/Ctrl+Q` or the close button quits. This surface is intentionally
one live PTY and one AI history; persistent tabs and nested split panes belong
to the interactive `r105` workspace in the current terminal.
The top bar is consistent on macOS, Linux, Windows, and other desktop targets:
it shows the installed version and opens Help or About. On macOS, the system
application menu also provides About r105 with “Built by BNE Labs”, the version,
and the same Help content.
`r105 window --smoke 60` renders 60 frames and exits with a frame
report for automated checks (`--smoke-chrome` also seeds the
composer, AI panel, and approval bar for headless coverage).
Use `r105 window --smoke 120 --smoke-snapshot /tmp/r105-frame.ppm` to save
the final frame for visual inspection.

`Ctrl+J` opens the AI composer (`Enter` sends, `Alt+Enter` inserts a newline,
`Esc` returns to the shell, and `Ctrl+C` cancels a run). Answers stream into
the AI panel (`Ctrl+K`; arrows and `PgUp`/`PgDn` navigate); tool calls use the
same approve-then-execute loop as the TUI, with `y`/`a`/`n` verdicts inline.
`Esc` in the terminal cancels a live AI run, otherwise it goes to the shell.
Drag selects terminal text. `Cmd+C/V` on macOS or `Ctrl+Shift+C/V` on
Linux/Windows copies and pastes; plain `Ctrl+C` remains the shell interrupt.
Scrollback, IME commits, and smoke-frame PPM capture are supported. AI
responses are plain text; markdown rendering remains deliberately out of
scope in this surface.

Common options:

| Option | Description |
| --- | --- |
| --url <URL> | OpenAI-compatible base URL |
| --workspace <PATH> | Workspace directory |
| --skills-dir <PATH> | Markdown skill directory |
| --plugins-dir <PATH> | Native plugin manifest directory |
| --model <NAME> | Model override |
| --backend <direct|router> | Connection mode |
| --provider <ID> | Provider preset |
| --profile <NAME> | Router profile |
| --quality <fast|balanced|best> | Router quality hint |
| --max-tokens <N> | Completion limit |
| --json | Request JSON object responses |
| --session <NAME> | Load a saved session |
| --timeout <SECONDS> | Backend and tool timeout |
| --yes | Select full access without prompting |

## Project layout

The Rust code is organized by responsibility:

```
src/
├── app.rs       diagnostics
├── approve.rs   approval policy, grants, and decision resolution
├── assistant.rs headless window AI orchestration and lifecycle tests
├── backend.rs   OpenAI-compatible HTTP and SSE client
├── command.rs   slash parser, registry, fuzzy visibility
├── config.rs    config schema and atomic JSON writes
├── custom.rs    Markdown workflow discovery and expansion
├── edit.rs      anchored edits and structured patch application
├── export.rs    dependency-free exporters
├── instructions.rs project instruction chain (AGENTS files)
├── mcp.rs       native MCP discovery and calls
├── model.rs     messages, state, usage
├── plugin.rs    executable plugin protocol
├── provider.rs  provider catalog and connection resolution
├── sandbox.rs   subprocess boundary
├── security.rs  path and web security checks
├── session.rs   versioned atomic persistence
├── suggest.rs   deterministic shell-history and argument completion
├── terminal.rs  PTY sessions, vt100 screen, and shell blocks
├── sse.rs       streaming parser and tool call accumulator
├── tool.rs      native tools and bounded expression parser
├── window.rs    native GPU terminal and inline AI chrome
└── ui/          Ratatui TUI (panes, tabs, commands, completion, render)
```

## Documentation

- Architecture: ARCHITECTURE.md
- Contribution workflow: CONTRIBUTING.md
- Security policy: SECURITY.md
- Skills: docs/SKILLS.md
- Tools: docs/TOOLS.md
- Export formats: docs/EXPORT.md
- Configuration: docs/CONFIGURATION.md
- Change specifications: docs/specs/README.md
- Package recipes: packaging/README.md
- Changelog: CHANGELOG.md
- Release notes: docs/release-notes/

## License

MIT. See LICENSE.
