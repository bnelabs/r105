# Optional Python bridge

`r105` is a native Rust executable and does not require Python. This directory
contains a stdlib-only reference bridge for installations that still need the
legacy `execute_python` tool.

Configure it for a TUI session with:

```sh
chmod +x bridge/r105_python_bridge.py
export R105_PYTHON_BRIDGE="python3 /path/to/r105/bridge/r105_python_bridge.py"
r105 bridge
```

Approve execution in the current session with `/approve execute_python`, or
use the compatibility `auto_approve_execute_python` setting. The Rust host
starts the configured command without a shell and sends one newline-delimited
JSON request for each execution. The protocol version is `1`; a request uses
`protocol`, `action`, `code`, `workspace`, `allow_network`, and
`timeout_seconds`, and the response returns `ok`, `exit_code`, `stdout`, and
`stderr`.

The reference bridge runs user code in a separate interpreter with a sanitized
environment, bounded output, a timeout, and best-effort Unix resource limits.
The selected r105 sandbox and permission posture still apply. Treat a custom
bridge executable as trusted local software.
