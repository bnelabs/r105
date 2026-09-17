# Native tools

r105 sends tool schemas in the OpenAI compatible request and executes returned calls through the Rust tool protocol.

## Built in tools

| Tool | Input | Behavior |
| --- | --- | --- |
| execute_rust | code | Compile and run a Rust program in the configured sandbox |
| write_file | path, content | Write a UTF-8 file under the workspace |
| edit_file | path, old_text, new_text, replace_all | Replace one anchored span; fails on missing/ambiguous anchors |
| apply_patch | patch | Apply a structured patch (Add|Update|Delete File with @@ hunks) |
| read_file | path | Read a UTF-8 workspace file |
| list_files | path | List a workspace directory |
| get_time | none | Return the system clock |
| calculate | expression | Evaluate bounded arithmetic |
| convert | value, from_unit, to_unit | Convert common units |
| system_info | none | Return OS, architecture, cwd, and pid |
| web_search | query | Search public web pages through DuckDuckGo |
| web_fetch | url | Fetch one public HTTP(S) page |
| todo_write | items | Replace the visible task list (max 20, one in_progress) |

The native ToolContext carries the workspace, sandbox, cancellation token, network permission, code permission, session mode, approval policy, shared todo list, and plugin directory. Tool calls in one model response run concurrently. A failed call becomes a tool result containing its error, so independent calls can finish.

Every call resolves through the approval policy first (mode gate, permission posture, command denylists/allowlists, then the per-category `approval_*` level): denied calls never start, and `ask` calls pause on an inline card in the TUI (`y` once, `a` always this run, `n` deny) or fail closed outside it.

## Tool protocol

A tool schema has the normal OpenAI function shape:

```json
{
  "type": "function",
  "function": {
    "name": "read_file",
    "description": "Read a UTF-8 workspace file",
    "parameters": {
      "type": "object",
      "properties": {
        "path": { "type": "string" }
      },
      "required": ["path"]
    }
  }
}
```

Tool arguments are accepted as JSON objects. The parser also repairs a JSON string wrapped in a Markdown JSON fence, because some compatible backends emit that form.

## Workspace security

All workspace paths pass through the canonical workspace root. The following are rejected:

- absolute paths;
- parent traversal;
- symlink paths that resolve outside the workspace;
- oversized file writes and reads.

Writes report whether a file was created or updated. Reads and tool results are bounded before they reach the model.

## Patch format

```
*** Begin Patch
*** Add File: notes/new.txt
hello
*** Update File: src/main.rs
@@
 context
-old line
+new line
*** Delete File: old.txt
*** End Patch
```

Update hunks use ` ` context, `-` removals, `+` additions, `@@` separators (ignored). Paths are workspace-relative only. Each hunk must match exactly once. Approval cards preview one line per file before anything writes.

## Instruction chain

Every request prepends project instructions as system messages: global `AGENTS.md`, then workspace `AGENTS.md`, `AGENTS.override.md`, `.r105/AGENTS.md`. Each file caps at 32 KiB; missing files are skipped. The order is stable for prefix-cache hits.

## Web security

web_fetch and web_search use a client with automatic redirects disabled.

For each request r105:

1. checks the scheme and rejects credentials;
2. rejects local and known metadata hostnames;
3. resolves all DNS answers;
4. rejects the host if any answer is loopback, private, link local, multicast, unspecified, carrier grade NAT, or IPv6 unique local;
5. pins the chosen public address in reqwest;
6. validates a Location target before following it;
7. stops after five redirects and truncates the response.

This closes the redirect bypass and the check then resolve gap for the request path. Public hosts with mixed public and private DNS answers fail closed.

## Arithmetic limits

calculate uses a recursive descent parser rather than eval. It limits expression length, nesting depth, numeric literals, powers, finite intermediate values, and factorial arguments. It supports arithmetic, pi, e, tau, sqrt, trigonometric functions, logarithms, exp, abs, floor, ceil, round, and factorial.

## Rust execution

execute_rust writes source under workspace/.r105/runs, compiles with rustc, runs the binary, and removes the temporary files. The Sandbox chooses:

1. nsjail;
2. bubblewrap;
3. Docker;
4. rlimit timeout fallback;
5. none only when explicitly configured.

The environment is cleared before the child starts. Network access is disabled for the Rust tool unless the permission posture allows it. Under the off posture, execute_rust is refused.

The rlimit fallback is a process timeout and output boundary. It is not a substitute for OS namespace isolation; r105 doctor shows which backend is active.

## Plugins

A plugin consists of a JSON manifest and an executable. A manifest in ~/.config/r105/plugins/ looks like this:

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

The executable receives one newline terminated request:

```json
{"method":"call","tool":"hello","arguments":{"name":"Ada"}}
```

It returns one JSON object with content or result. The model sees the tool as plugin_example_hello. Plugin names are namespaced and plugin processes inherit a cleared environment plus PATH.

## MCP

MCP servers use the config.json mcp_servers list. /mcp reconnect performs initialize and tools/list, caches the returned schemas, and exposes them as mcp_server_tool. /mcp tools prints the cached OpenAI function schemas. /mcp list prints configured and discovered counts.

Stdio servers receive JSON RPC lines. HTTP servers receive request/response JSON or an SSE data response. Each tool call has a 30 second response limit.
