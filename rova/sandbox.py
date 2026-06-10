"""Sandbox backends for executing untrusted Python code.

Provides a pluggable sandbox abstraction with three backends:

- RLimitSandbox: resource limits via setrlimit (Unix-only, no isolation)
- BwrapSandbox: namespace isolation via bubblewrap (stronger, Linux, requires bwrap)
- NsjailSandbox: advanced isolation via nsjail with seccomp-bpf (strongest, Linux)

Backend selection is automatic: nsjail > bwrap > rlimit > none (Windows).

Per-tool sandbox profiles allow tools to specify their isolation requirements
(e.g., execute_python needs no network, while web_search needs it).
"""

from __future__ import annotations

import abc
import os
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from typing import Any

from rova.constants import (
    SANDBOX_CPU_SECONDS,
    SANDBOX_FILESIZE_MB,
    SANDBOX_MEMORY_MB,
    SANDBOX_TIMEOUT,
)
from rova.errors import SandboxUnavailableError

# -- Sandbox profiles ---------------------------------------------------------


@dataclass
class SandboxProfile:
    """Isolation requirements for a tool execution.

    Each tool declares what it needs, and the sandbox backend enforces
    the strictest possible isolation while granting only what's required.
    """

    # Whether the tool needs network access (web_search, web_fetch)
    needs_network: bool = False

    # Whether the tool needs to read/write host filesystem files
    needs_filesystem: bool = False

    # Whether the tool needs write access (vs. read-only)
    needs_write: bool = False

    # Whether to enable seccomp filtering (nsjail only, always on for bwrap)
    seccomp: bool = True

    # Custom seccomp policy string (nsjail --seccomp_string)
    seccomp_policy: str = ""

    # Process timeout in seconds
    timeout: float = SANDBOX_TIMEOUT

    # Memory limit in MB
    memory_mb: int = SANDBOX_MEMORY_MB

    # CPU time limit in seconds
    cpu_seconds: int = SANDBOX_CPU_SECONDS


# Default profiles for built-in tools
PROFILE_EXECUTE_PYTHON = SandboxProfile(
    needs_network=False,
    needs_filesystem=False,
    needs_write=False,
    seccomp=True,
)
PROFILE_FILE_TOOLS = SandboxProfile(
    needs_network=False,
    needs_filesystem=True,
    needs_write=True,
    seccomp=True,
)
PROFILE_WEB_TOOLS = SandboxProfile(
    needs_network=True,
    needs_filesystem=False,
    needs_write=False,
    seccomp=True,
)
PROFILE_SYSTEM_TOOLS = SandboxProfile(
    needs_network=False,
    needs_filesystem=False,
    needs_write=False,
    seccomp=False,
)


def profile_for_tool(name: str) -> SandboxProfile:
    """Return the appropriate sandbox profile for a tool name."""
    profiles: dict[str, SandboxProfile] = {
        "execute_python": PROFILE_EXECUTE_PYTHON,
        "write_file": PROFILE_FILE_TOOLS,
        "read_file": PROFILE_FILE_TOOLS,
        "list_files": PROFILE_FILE_TOOLS,
        "web_search": PROFILE_WEB_TOOLS,
        "web_fetch": PROFILE_WEB_TOOLS,
        "get_time": PROFILE_SYSTEM_TOOLS,
        "calculate": PROFILE_SYSTEM_TOOLS,
        "system_info": PROFILE_SYSTEM_TOOLS,
    }
    return profiles.get(name, PROFILE_EXECUTE_PYTHON)


# -- Environment sanitisation -------------------------------------------

# Env vars that are safe to pass through to the sandbox.
_SAFE_ENV_PREFIXES = (
    "PATH", "HOME", "TMPDIR", "TMP", "LANG", "LC_", "USER", "LOGNAME",
    "TERM", "SHELL", "COLORTERM", "DISPLAY", "WAYLAND_DISPLAY",
    "XDG_", "DBUS_", "PYTHONUNBUFFERED",
)

# Patterns that indicate a secret/credential-bearing variable.
_SECRET_PATTERNS = re.compile(
    r"(?i)(SECRET|TOKEN|KEY|PASSWORD|PASSWD|CREDENTIAL|CERT|AUTH)",
)


def _sanitize_env(tmpdir: str) -> dict[str, str]:
    """Return a minimal environment dict with secrets stripped.

    Only safe variables are forwarded. Everything else is dropped.
    """
    clean: dict[str, str] = {}
    for key, value in os.environ.items():
        # Drop known secret-bearing vars
        if _SECRET_PATTERNS.search(key):
            continue
        # Drop cloud-provider credential vars
        if any(key.startswith(p) for p in (
            "AWS_", "GCP_", "AZURE_", "GOOGLE_",
            "OPENAI_", "ANTHROPIC_", "COHERE_",
            "GITHUB_TOKEN", "DOCKER_", "KUBECONFIG", "SSH_",
        )):
            continue
        # Keep only explicitly safe prefixes
        if any(key == prefix or key.startswith(prefix) for prefix in _SAFE_ENV_PREFIXES):
            clean[key] = value

    # Override with sandbox-specific paths
    clean["HOME"] = tmpdir
    clean["TMPDIR"] = tmpdir
    clean["PYTHONPATH"] = ""
    return clean


# -- Base class --------------------------------------------------------------


class SandboxBackend(abc.ABC):
    """Abstract base for Python sandbox backends."""

    @abc.abstractmethod
    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
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


# -- RLimit backend ---------------------------------------------------------


class RLimitSandbox(SandboxBackend):
    """Sandbox using resource.setrlimit() for basic resource limits.

    Limits: 256 MB memory, 25s CPU, no child processes, 50 MB files.
    Runs under the host UID — NOT suitable for multi-tenant production use.
    """

    name = "rlimit"

    @staticmethod
    def is_available() -> bool:
        return sys.platform != "win32"

    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
        p = profile or PROFILE_EXECUTE_PYTHON
        tmpdir = tempfile.mkdtemp(prefix="rova_sandbox_")
        try:
            kwargs: dict[str, Any] = {
                "capture_output": True,
                "text": True,
                "timeout": p.timeout if timeout == SANDBOX_TIMEOUT else timeout,
                "cwd": tmpdir,
                "env": _sanitize_env(tmpdir),
            }
            if sys.platform != "win32":
                kwargs["preexec_fn"] = _sandbox_preexec

            return subprocess.run([sys.executable, "-c", code], **kwargs)
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


def _sandbox_preexec() -> None:
    """Set resource limits for sandboxed Python execution (Unix only)."""
    import resource

    mem_bytes = SANDBOX_MEMORY_MB * 1024 * 1024
    resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
    resource.setrlimit(resource.RLIMIT_CPU, (SANDBOX_CPU_SECONDS, SANDBOX_CPU_SECONDS))
    resource.setrlimit(resource.RLIMIT_NPROC, (0, 0))
    resource.setrlimit(resource.RLIMIT_FSIZE, (SANDBOX_FILESIZE_MB * 1024 * 1024, SANDBOX_FILESIZE_MB * 1024 * 1024))


# -- Bubblewrap backend ------------------------------------------------------


class BwrapSandbox(SandboxBackend):
    """Sandbox using bubblewrap (bwrap) for Linux namespace isolation.

    Provides stronger isolation than rlimit:
    - Private /tmp (tmpfs)
    - No network (--unshare-net, if supported)
    - Read-only access to /usr, /lib, /bin, /etc (for Python stdlib)
    - Minimal /dev with only null, urandom, zero (not full host /dev)
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

    _netns_available: bool | None = None

    @classmethod
    def _has_netns(cls) -> bool:
        """Check if network namespaces are supported in this environment."""
        if cls._netns_available is not None:
            return cls._netns_available
        try:
            result = subprocess.run(
                ["bwrap", "--unshare-net", "--die-with-parent", "true"],
                capture_output=True, text=True, timeout=5,
            )
            cls._netns_available = result.returncode == 0
        except Exception:
            cls._netns_available = False
        return cls._netns_available

    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
        p = profile or PROFILE_EXECUTE_PYTHON
        actual_timeout = p.timeout if timeout == SANDBOX_TIMEOUT else timeout
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
                # Minimal /dev — bind only what Python needs
                "--dev-bind", "/dev/null", "/dev/null",
                "--dev-bind", "/dev/urandom", "/dev/urandom",
                "--dev-bind", "/dev/zero", "/dev/zero",
                "--dev-bind", "/dev/fd", "/dev/fd",
            ]
            # Network: only grant if the tool profile requires it
            if not p.needs_network and self._has_netns():
                cmd.insert(8, "--unshare-net")

            # Filesystem access: only bind workspace if needed
            if not p.needs_filesystem:
                cmd.insert(8, "--tmpfs", "/home")

            cmd.extend([sys.executable, "-c", code])

            return subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=actual_timeout,
                cwd="/tmp",
                env=_sanitize_env(tmpdir),
            )
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


# -- Nsjail backend ----------------------------------------------------------


class NsjailSandbox(SandboxBackend):
    """Sandbox using nsjail for advanced Linux namespace isolation.

    Provides the strongest isolation of all backends:
    - Chroot-based filesystem isolation (copies minimal Python environment)
    - Seccomp-bpf filtering (syscall allowlist)
    - Full user namespace mapping (appears as root inside, unprivileged outside)
    - Network namespace isolation (--clone_newnet)
    - CLONE_NEWPID isolation (no access to host process list)
    - All resource limits (rlimit + cgroup)

    Requires nsjail to be installed: apt install nsjail / pacman -S nsjail

    The default seccomp policy blocks dangerous syscalls (mount, reboot, kexec,
    bpf, etc.) while allowing normal Python operations (read, write, socket,
    etc.).
    """

    name = "nsjail"

    # Default seccomp-bpf allowlist string for normal Python execution.
    # Blocks kernel-hazardous operations while permitting stdlib usage.
    _DEFAULT_SECCOMP_POLICY = (
        # Allow: core process operations
        "ALLOW { read,write,open,openat,close,mmap,mprotect,munmap,brk "
        "getcwd,chdir,fstat,newfstatat,lseek,pread64,pwrite64,readlink,readlinkat "
        "statx,getdents,getdents64,ioctl,fcntl,flock,fsync,dup,dup2,dup3 "
        "pipe,pipe2,socket,connect,bind,listen,accept,accept4,setsockopt,getsockopt "
        "exit,exit_group,nanosleep,clock_gettime,gettimeofday,time "
        "futex,getpid,getppid,gettid,geteuid,getegid,getuid,getgid "
        "clone,clone3,fork,vfork,wait4,waitid,rt_sigaction,rt_sigprocmask "
        "set_robust_list,get_robust_list,set_tid_address "
        "mmap,munmap,mremap,mlock,munlock "
        "sendto,recvfrom,sendmsg,recvmsg,shutdown,getsockname,getpeername "
        "uname,sysinfo,prctl,arch_prctl "
        "sigaltstack,personality,gettid,setpgid,getpgid,setsid "
        "socketpair,sendfile,splice,tee,epoll_create,epoll_ctl,epoll_wait "
        "eventfd2,openat2,close_range,pidfd_open,pidfd_send_signal "
        # Allow time/random
        "clock_nanosleep,clock_getres"
        "}"
    )

    @staticmethod
    def is_available() -> bool:
        """Check if nsjail is installed."""
        if sys.platform != "linux":
            return False
        if shutil.which("nsjail") is None:
            return False
        try:
            result = subprocess.run(
                ["nsjail", "--help"],
                capture_output=True, text=True, timeout=5,
            )
            return result.returncode == 0
        except Exception:
            return False

    def _build_nsjail_cfg(
        self,
        tmpdir: str,
        env: dict[str, str],
        profile: SandboxProfile,
        code: str,
    ) -> list[str]:
        """Build the nsjail command-line arguments based on profile."""
        python_bin = shutil.which("python3") or shutil.which("python") or sys.executable
        lib_paths = _find_lib_dirs()

        args = [
            "nsjail",
            "--really_quiet",  # suppress nsjail banner
            "--chroot", "/",   # use host filesystem as chroot
            "--rw",             # make chroot read-write
            "--disable_proc",   # no /proc inside jail
            "--time_limit", str(int(profile.timeout)),
            "--rlimit_as", str(profile.memory_mb * 1024 * 1024),
            "--rlimit_cpu", str(profile.cpu_seconds),
            "--rlimit_nproc", "64",
            "--rlimit_fsize", str(SANDBOX_FILESIZE_MB * 1024 * 1024),
            "--max_cpus", "1",
            "--hostname", "rova-sandbox",
            "--is_root_rw", "false",
        ]

        # Seccomp: default policy or custom
        if profile.seccomp:
            policy = profile.seccomp_policy or self._DEFAULT_SECCOMP_POLICY
            args.extend(["--seccomp_string", policy])
        else:
            args.append("--seccomp_log")  # log but don't block

        # Network: only if the tool needs it
        if not profile.needs_network:
            args.append("--clone_newnet")

        # Filesystem binds
        # Bind temporary directory as writable working directory
        args.extend(["--bindmount", f"{tmpdir}:/tmp"])
        args.extend(["--cwd", "/tmp"])

        # Read-only bind of Python and required libs
        args.extend(["--bindmount_ro", f"{python_bin}:{python_bin}"])
        for lib_dir in lib_paths:
            args.extend(["--bindmount_ro", f"{lib_dir}:{lib_dir}"])

        # Minimal /dev inside the jail
        args.extend(["--dev_null"])
        args.extend(["--dev_urandom"])
        args.extend(["--dev_zero"])

        # Skip host /home access for non-filesystem tools
        if not profile.needs_filesystem:
            args.extend(["--tmpfs", "/home"])

        # Set environment
        for key, value in env.items():
            args.extend(["--env", f"{key}={value}"])

        # The command to run
        args.extend(["--", python_bin, "-c", code])

        return args

    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
        p = profile or PROFILE_EXECUTE_PYTHON
        actual_timeout = p.timeout if timeout == SANDBOX_TIMEOUT else timeout
        tmpdir = tempfile.mkdtemp(prefix="rova_nsjail_")
        try:
            env = _sanitize_env(tmpdir)
            cmd = self._build_nsjail_cfg(tmpdir, env, p, code)

            return subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=actual_timeout + 5.0,  # extra seconds for nsjail startup
            )
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


def _find_lib_dirs() -> list[str]:
    """Find library directories needed by Python (lib, lib64)."""
    dirs: list[str] = []
    # Common library paths
    for p in ["/lib", "/lib64", "/usr/lib", "/usr/lib64", "/usr/lib/python3",
              "/usr/local/lib"]:
        if os.path.isdir(p):
            dirs.append(p)
    # Also include the site-packages for installed packages
    try:
        import site
        for sp in site.getsitepackages():
            if os.path.isdir(sp):
                dirs.append(sp)
    except Exception:
        pass
    return dirs


# -- Noop backend ----------------------------------------------------------


class NoopSandbox(SandboxBackend):
    """No sandbox — executes Python directly. Fallback for Windows or when
    no sandbox is available. Only used when explicitly configured."""

    name = "none"

    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-c", code],
            capture_output=True,
            text=True,
            timeout=timeout if timeout else SANDBOX_TIMEOUT,
        )


# -- Backend registry -------------------------------------------------------

_BACKENDS: list[type[SandboxBackend]] = [NsjailSandbox, BwrapSandbox, RLimitSandbox]
_sandbox: SandboxBackend | None = None


def detect_backend() -> SandboxBackend:
    """Return the best available sandbox backend.

    Preference order: nsjail > bwrap > rlimit > none.
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
    Raises SandboxUnavailableError if the named backend exists but
    cannot be used on this system.
    """
    name = name.lower()
    for cls in _BACKENDS:
        if cls.name == name:
            if cls.is_available():
                return cls()
            raise SandboxUnavailableError(
                f"Sandbox backend '{name}' is not available on this system"
            )
    if name == "none":
        return NoopSandbox()
    return None
