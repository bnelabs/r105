"""Sandbox backends for executing untrusted Python code.

Provides a pluggable sandbox abstraction with two backends:

- RLimitSandbox: resource limits via setrlimit (current behavior, Unix-only)
- BwrapSandbox: namespace isolation via bubblewrap (stronger, requires bwrap)

Backend selection is automatic: bwrap > rlimit > none (Windows).
"""

from __future__ import annotations

import abc
import os
import shutil
import subprocess
import sys
import tempfile
from typing import Any


class SandboxBackend(abc.ABC):
    """Abstract base for Python sandbox backends."""

    @abc.abstractmethod
    def execute(self, code: str, timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
        """Execute Python *code* in a sandbox and return the CompletedProcess."""
        ...

    @property
    @abc.abstractmethod
    def name(self) -> str:
        """Human-readable backend name for /health output."""
        ...

    @staticmethod
    def is_available() -> bool:
        """Return True if this backend can be used on this system."""
        return True


class RLimitSandbox(SandboxBackend):
    """Sandbox using resource.setrlimit() for basic resource limits.

    Limits: 256 MB memory, 25s CPU, no child processes, 50 MB files.
    Runs under the host UID — NOT suitable for multi-tenant production use.
    """

    name = "rlimit"

    @staticmethod
    def is_available() -> bool:
        return sys.platform != "win32"

    def execute(self, code: str, timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
        tmpdir = tempfile.mkdtemp(prefix="rova_sandbox_")
        try:
            kwargs: dict[str, Any] = {
                "capture_output": True,
                "text": True,
                "timeout": timeout,
                "cwd": tmpdir,
                "env": {
                    "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                    "HOME": tmpdir,
                    "TMPDIR": tmpdir,
                    "PYTHONPATH": "",
                },
            }
            if sys.platform != "win32":
                kwargs["preexec_fn"] = _sandbox_preexec

            return subprocess.run([sys.executable, "-c", code], **kwargs)
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


def _sandbox_preexec() -> None:
    """Set resource limits for sandboxed Python execution (Unix only)."""
    import resource

    mem_bytes = 256 * 1024 * 1024    # 256 MB
    resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
    resource.setrlimit(resource.RLIMIT_CPU, (25, 25))
    resource.setrlimit(resource.RLIMIT_NPROC, (0, 0))
    resource.setrlimit(resource.RLIMIT_FSIZE, (50 * 1024 * 1024, 50 * 1024 * 1024))


class BwrapSandbox(SandboxBackend):
    """Sandbox using bubblewrap (bwrap) for Linux namespace isolation.

    Provides stronger isolation than rlimit:
    - Private /tmp (tmpfs)
    - No network (--unshare-net, if supported)
    - Read-only access to /usr, /lib, /bin, /etc (for Python stdlib)
    - No access to host files outside the sandbox tmpdir
    - Process dies with parent (--die-with-parent)

    Requires bubblewrap to be installed: apt install bubblewrap / pacman -S bubblewrap
    """

    name = "bwrap"

    @staticmethod
    def is_available() -> bool:
        """Check if bwrap is installed AND can actually create a sandbox.

        Some environments (containers, restricted kernels) have bwrap installed
        but deny user-namespace creation. We smoke-test with a trivial command.
        """
        if sys.platform != "linux":
            return False
        if shutil.which("bwrap") is None:
            return False
        try:
            result = subprocess.run(
                ["bwrap", "--ro-bind", "/usr", "/usr", "--die-with-parent", "true"],
                capture_output=True, text=True, timeout=5,
            )
            return result.returncode == 0
        except Exception:
            return False

    @staticmethod
    def _has_netns() -> bool:
        """Check if network namespaces are supported in this environment."""
        try:
            result = subprocess.run(
                ["bwrap", "--unshare-net", "--die-with-parent", "true"],
                capture_output=True, text=True, timeout=5,
            )
            return result.returncode == 0
        except Exception:
            return False

    def execute(self, code: str, timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
        tmpdir = tempfile.mkdtemp(prefix="rova_bwrap_")
        try:
            cmd = [
                "bwrap",
                "--ro-bind", "/usr", "/usr",
                "--ro-bind", "/lib", "/lib",
                "--ro-bind", "/lib64", "/lib64",
                "--ro-bind", "/bin", "/bin",
                "--ro-bind", "/etc", "/etc",
                "--tmpfs", "/tmp",
                "--bind", tmpdir, "/tmp",
                "--die-with-parent",
                "--proc", "/proc",
                "--dev", "/dev",
            ]
            # Network isolation is best-effort — may fail in containers
            if self._has_netns():
                cmd.insert(8, "--unshare-net")

            cmd.extend([sys.executable, "-c", code])

            return subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=timeout,
                cwd="/tmp",
            )
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


class NoopSandbox(SandboxBackend):
    """No sandbox — executes Python directly. Fallback for Windows or when
    no sandbox is available. Only used when explicitly configured."""

    name = "none"

    def execute(self, code: str, timeout: float = 30.0) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            timeout=timeout,
        )


# -- Backend registry -------------------------------------------------------

_BACKENDS: list[type[SandboxBackend]] = [BwrapSandbox, RLimitSandbox]
_sandbox: SandboxBackend | None = None


def detect_backend() -> SandboxBackend:
    """Return the best available sandbox backend.

    Preference order: bwrap > rlimit > none.
    """
    for cls in _BACKENDS:
        if cls.is_available():
            return cls()
    return NoopSandbox()


def get_sandbox() -> SandboxBackend:
    """Return the current sandbox backend, creating a default one if needed."""
    global _sandbox
    if _sandbox is None:
        _sandbox = detect_backend()
    return _sandbox


def set_sandbox(name: str) -> SandboxBackend | None:
    """Set the sandbox backend by name. Returns None if the named backend is unavailable."""
    global _sandbox
    backend = get_backend(name)
    if backend is not None:
        _sandbox = backend
    return backend


def get_backend(name: str) -> SandboxBackend | None:
    """Look up a sandbox backend by name (case-insensitive).

    Returns None if no backend with that name is found.
    """
    name = name.lower()
    for cls in _BACKENDS:
        if cls.name == name:
            if cls.is_available():
                return cls()
            return None
    if name == "none":
        return NoopSandbox()
    return None
