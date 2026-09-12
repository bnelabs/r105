#!/usr/bin/env python3
"""Optional stdlib-only Python compatibility bridge for r105.

The Rust application starts this program only when the user configures it.
Communication is newline-delimited JSON on stdin/stdout; Python program output
is captured and returned in the response, so it cannot corrupt the protocol.
This file is deliberately separate from the Rust release artifacts.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from typing import Any

PROTOCOL_VERSION = 1
MAX_REQUEST_BYTES = 160_000
MAX_CODE_BYTES = 100 * 1024
MAX_OUTPUT_BYTES = 100_000


def _respond(value: dict[str, Any]) -> None:
    sys.stdout.write(json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def _decode(value: bytes) -> str:
    return value.decode("utf-8", errors="replace")


def _read_limited(pipe: Any, result: dict[str, Any], key: str) -> None:
    data = bytearray()
    total = 0
    try:
        while True:
            chunk = pipe.read(8192)
            if not chunk:
                break
            total += len(chunk)
            if len(data) < MAX_OUTPUT_BYTES:
                data.extend(chunk[: MAX_OUTPUT_BYTES - len(data)])
    finally:
        result[key] = bytes(data)
        result[f"{key}_truncated"] = total > MAX_OUTPUT_BYTES


def _limits() -> None:
    """Apply best-effort resource limits inside the child interpreter."""
    try:
        import resource

        cpu = 30
        memory = 512 * 1024 * 1024
        output = 10 * 1024 * 1024
        resource.setrlimit(resource.RLIMIT_CPU, (cpu, cpu))
        resource.setrlimit(resource.RLIMIT_AS, (memory, memory))
        resource.setrlimit(resource.RLIMIT_FSIZE, (output, output))
    except (ImportError, OSError, ValueError):
        # Windows and restricted containers may not provide resource(3).
        pass


def _child_source(code: str, workspace: str, allow_network: bool) -> str:
    workspace_literal = repr(workspace)
    network_guard = "" if allow_network else """
def _r105_deny_network(event, args):
    if event.startswith("socket."):
        raise PermissionError("network access is disabled for this Python tool")
import sys as _r105_sys
_r105_sys.addaudithook(_r105_deny_network)
"""
    return f"""import sys as _r105_sys
import os as _r105_os
_r105_sys.path.insert(0, {workspace_literal})
{network_guard}
exec(compile({code!r}, '<r105-python>', 'exec'), {{'__name__': '__main__', '__file__': '<r105-python>'}})
"""


def _execute(request: dict[str, Any]) -> dict[str, Any]:
    code = request.get("code")
    workspace_value = request.get("workspace")
    if not isinstance(code, str) or not code:
        return {"ok": False, "error": "code is required"}
    if len(code.encode("utf-8")) > MAX_CODE_BYTES:
        return {"ok": False, "error": "code is too large"}
    if not isinstance(workspace_value, str) or not workspace_value:
        return {"ok": False, "error": "workspace is required"}

    workspace = Path(workspace_value).expanduser().resolve()
    if not workspace.is_dir():
        return {"ok": False, "error": "workspace is not a directory"}
    allow_network = bool(request.get("allow_network", False))
    try:
        timeout_seconds = float(request.get("timeout_seconds", 30))
    except (TypeError, ValueError):
        timeout_seconds = 30.0
    timeout_seconds = min(max(timeout_seconds, 1.0), 300.0)

    environment = {
        key: os.environ[key]
        for key in ("PATH", "LANG", "LC_ALL", "TZ")
        if key in os.environ
    }
    environment.update(
        {
            "HOME": str(workspace),
            "PYTHONNOUSERSITE": "1",
            "PYTHONUNBUFFERED": "1",
            "R105_PYTHON_BRIDGE_CHILD": "1",
            "R105_ALLOW_NETWORK": "1" if allow_network else "0",
        }
    )

    source = _child_source(code, str(workspace), allow_network)
    command = [sys.executable, "-I", "-S", "-c", source]
    try:
        process = subprocess.Popen(
            command,
            cwd=str(workspace),
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            preexec_fn=_limits if os.name != "nt" else None,
        )
    except OSError as error:
        return {"ok": False, "error": f"could not start Python: {error}"}

    captured: dict[str, Any] = {}
    stdout_thread = threading.Thread(
        target=_read_limited, args=(process.stdout, captured, "stdout"), daemon=True
    )
    stderr_thread = threading.Thread(
        target=_read_limited, args=(process.stderr, captured, "stderr"), daemon=True
    )
    stdout_thread.start()
    stderr_thread.start()
    timed_out = False
    try:
        process.wait(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        process.kill()
        process.wait()
    stdout_thread.join(timeout=2)
    stderr_thread.join(timeout=2)
    stdout = _decode(captured.get("stdout", b""))
    stderr = _decode(captured.get("stderr", b""))
    truncated = bool(captured.get("stdout_truncated") or captured.get("stderr_truncated"))
    if truncated:
        stderr = (stderr + "\n" if stderr else "") + "[output truncated by r105 Python bridge]"
    return {
        "ok": True,
        "exit_code": None if timed_out else process.returncode,
        "stdout": stdout,
        "stderr": stderr,
        "timed_out": timed_out,
        "truncated": truncated,
    }


def main() -> int:
    for raw in sys.stdin:
        if len(raw.encode("utf-8", errors="replace")) > MAX_REQUEST_BYTES:
            _respond({"ok": False, "error": "request is too large"})
            continue
        try:
            request = json.loads(raw)
        except json.JSONDecodeError as error:
            _respond({"ok": False, "error": f"invalid JSON: {error.msg}"})
            continue
        if not isinstance(request, dict):
            _respond({"ok": False, "error": "request must be a JSON object"})
            continue
        if request.get("protocol") != PROTOCOL_VERSION:
            _respond({"ok": False, "error": f"unsupported protocol: {request.get('protocol')!r}"})
            continue
        if request.get("action") != "execute":
            _respond({"ok": False, "error": "unsupported action"})
            continue
        _respond(_execute(request))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
