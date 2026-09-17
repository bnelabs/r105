# Configuration

r105 keeps configuration, sessions, tab state, and plugins under one local
configuration directory. The default is `~/.config/r105`; set `R105_CONFIG_DIR`
to use another directory. On systems that provide `XDG_CONFIG_HOME`, r105 uses
`$XDG_CONFIG_HOME/r105` when `R105_CONFIG_DIR` is not set.

| Path | Purpose |
| --- | --- |
| `config.json` | Provider, model, UI, completion, permission, and sandbox settings |
| `sessions/` | Named and autosave TUI sessions plus native-window checkpoints |
| `tabs.json` | TUI tab order, pane sessions, focus, zoom, and split-tree layout |
| `plugins/` | Native plugin manifests and executable tool definitions |
| `skills/` | Markdown skill prompts (the path is configurable) |
| `r105.log` | Interactive `chat`/`terminal` diagnostics; headless commands use stderr |

## Example

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
  "approval_exec": "ask",
  "approval_write": "ask",
  "approval_read": "allow",
  "approval_network": "allow",
  "approval_mcp": "ask",
  "approval_plugin": "ask",
  "timeout_seconds": 120
}
```

Unknown keys are ignored with a warning for compatibility. Set
`R105_STRICT_CONFIG=1` (or `true`, `yes`, or `on`) to fail on unknown keys.
Generate the authoritative JSON Schema with:

```sh
r105 config-schema
r105 config-schema --output /tmp/r105-config-schema.json
```

## Settings

| Key | Values / shape | Purpose |
| --- | --- | --- |
| `theme` | `r105`, `dracula`, `solarized-dark`, `high-contrast` | TUI theme |
| `workspace` | path or `null` | Default workspace for files and commands |
| `skills_dir` | path | Markdown skill directory |
| `plugins_dir` | path | Native plugin manifest directory |
| `backend` | `direct`, `router`, or `null` | Transport mode |
| `provider` | provider id or `null` | Preset used by `/connect` and startup |
| `url` | HTTP(S) URL or `null` | OpenAI-compatible base URL |
| `model` | string or `null` | Saved model selection |
| `profile` | string or `null` | Router profile |
| `quality` | `fast`, `balanced`, `best`, or `null` | Router quality hint |
| `timeout_seconds` | positive integer | Backend, tool, and one-shot PTY timeout |
| `max_tokens` | CLI-only | Completion limit for the current run |
| `auto_compact` | boolean | Compact older context automatically |
| `cache_prompt` | boolean | Enable local prompt-prefix caching when supported |
| `context_tokens` | positive integer or `null` | Fallback context window |
| `model_contexts` | object of positive integers | Exact or longest-contained model context overrides |
| `model_families` | object of strings or `null` | Optional provider family metadata |
| `reasoning_effort` | `auto`, `off`, `none`, `disabled`, `low`, `medium`, `high`, `max`, `xhigh` | Reasoning hint |
| `show_thinking` | boolean | Show reasoning sections in the TUI |
| `thinking_default_expanded` | boolean | Initial reasoning expansion state |
| `attention_bell` | boolean | Ring the terminal bell when work settles |
| `mouse` | boolean | Enable TUI mouse capture |
| `completion_enabled` | boolean | Enable local completion |
| `completion_debounce_ms` | 0–5000 | Completion debounce |
| `completion_history_max` | 1–5000 | Shell-history completion entries |
| `ai_suggest` | boolean | Allow idle model shell suggestions after local completion |
| `permission_posture` | `full-access`, `restricted`, `sandboxed`, `off` | Global execution posture |
| `sandbox_backend` | `auto`, `nsjail`, `bwrap`, `docker`, `rlimit`, `none` | Rust/shell execution boundary |
| `approval_exec` | `allow`, `ask`, `deny` | Execution approval default |
| `approval_write` | `allow`, `ask`, `deny` | File-write approval default |
| `approval_read` | `allow`, `ask`, `deny` | File-read approval default |
| `approval_network` | `allow`, `ask`, `deny` | Network approval default |
| `approval_mcp` | `allow`, `ask`, `deny` | MCP approval default |
| `approval_plugin` | `allow`, `ask`, `deny` | Native-plugin approval default |
| `command_allowlist` | string array | Commands that may bypass execution prompts |
| `command_denylist` | string array | Commands that are always denied |
| `allow_plugin_overrides` | boolean | Permit plugin hook result/argument rewrites |
| `docker_image` | string or `null` | Image used by the Docker sandbox backend |
| `keybindings` | string map | Remappable TUI Ctrl actions |
| `mcp_servers` | JSON array | Stdio or HTTP MCP server definitions |

`max_tokens`, `workspace`, `model`, `url`, provider, backend, profile, and
quality can also be supplied as CLI flags. CLI values apply only to the
current process. `R105_MODEL` overrides the startup model, `R105_URL` provides
the startup URL, and `OPENAI_BASE_URL` is the final OpenAI-compatible URL
fallback. Provider-specific API keys are read from the environment or entered
for the current TUI connection flow; they are never written to `config.json`
or session files.

## Migration and durability

Configuration and named-session writes use temporary files and atomic
replacement. The loader accepts older config/session shapes and applies
defaults for fields that were added later. Tab state is normalized on load:
missing layouts, legacy single-pane entries, invalid pane indexes, and stale
focus values are repaired before the next save.
