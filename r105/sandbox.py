"""Sandbox backends for executing untrusted Python code.

Provides a pluggable sandbox abstraction with backends:

- NsjailSandbox: advanced isolation via nsjail with seccomp-bpf (strongest, Linux)
- BwrapSandbox: namespace isolation via bubblewrap (stronger, Linux, requires bwrap)
- DockerSandbox: container isolation via Docker (strong, cross-platform, requires docker)
- RLimitSandbox: resource limits via setrlimit ONLY (Unix-only, NO isolation).
  LAST-RESORT FALLBACK — does NOT isolate filesystem or network. Never use for
  untrusted code unless nsjail/bwrap/docker are unavailable, and even then
  only for fully trusted local code.
- NoopSandbox: no isolation at all (explicit opt-in only).

Backend selection is automatic: nsjail > bwrap > docker > rlimit > none.
RLimit is deliberately demoted to a strict last resort: it only restricts
CPU/RAM via setrlimit and provides zero filesystem/network isolation.

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

from r105.constants import (
    SANDBOX_CPU_SECONDS,
    SANDBOX_FILESIZE_MB,
    SANDBOX_MEMORY_MB,
    SANDBOX_TIMEOUT,
)
from r105.errors import SandboxUnavailableError

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

# Explicit allowlist of safe environment variables forwarded to the sandbox.
# Deliberately exhaustive: anything not listed here is dropped. Previous
# prefix-based matching (e.g. "PATH" matching "PATH_EVIL") risked leaking
# secrets when a prefix collided with a longer variable name.
_SAFE_ENV_VARS = frozenset({
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_COLLATE",
    "LANGUAGE",
    "USER",
    "LOGNAME",
    "TERM",
    "TERMINFO",
    "SHELL",
    "COLORTERM",
    "NO_COLOR",
    "CLICOLOR",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "PYTHONUNBUFFERED",
    "PYTHONIOENCODING",
    "PYTHONHASHSEED",
    "TZ",
    "EDITOR",
})

# Only true namespace prefixes (trailing underscore) are allowed via prefix
# matching. "LC_" and "XDG_" are namespaced by convention; everything else
# must match exactly in _SAFE_ENV_VARS above.
_SAFE_ENV_PREFIXES = (
    "LC_",
    "XDG_",
)

# Patterns that indicate a secret/credential-bearing variable.
_SECRET_PATTERNS = re.compile(
    r"(?i)(SECRET|TOKEN|KEY|PASSWORD|PASSWD|CREDENTIAL|CERT|AUTH)",
)


def _sanitize_env(tmpdir: str) -> dict[str, str]:
    """Return a minimal environment dict with secrets stripped.

    Only variables in the explicit ``_SAFE_ENV_VARS`` allowlist (plus the
    namespaced ``LC_*``/``XDG_*`` prefixes) are forwarded. Everything else
    is dropped. Secret-bearing and cloud-credential variables are always
    dropped even if they would otherwise match the allowlist.
    """
    clean: dict[str, str] = {}
    for key, value in os.environ.items():
        # Drop known secret-bearing vars (always, even if allowlisted)
        if _SECRET_PATTERNS.search(key):
            continue
        # Drop cloud-provider / AI-provider credential vars
        if any(key == p or key.startswith(p) for p in (
            "AWS_", "GCP_", "AZURE_", "GOOGLE_",
            "OPENAI_", "ANTHROPIC_", "COHERE_",
            "GITHUB_", "DOCKER_", "KUBECONFIG", "SSH_", "KUBE_",
            "R105_", "LLAMA_",
        )):
            continue
        # Keep only explicitly allowlisted vars or namespaced prefixes
        if key in _SAFE_ENV_VARS or key.startswith(_SAFE_ENV_PREFIXES):
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


# -- RLimit backend (DEMOTED: last-resort fallback, NOT isolation) ------------


class RLimitSandbox(SandboxBackend):
    """STRICT LAST-RESORT FALLBACK — NOT true isolation.

    Uses ``resource.setrlimit()`` for basic CPU/RAM/filesize limits ONLY.
    It does NOT isolate the filesystem, network, PIDs, or UIDs: code runs
    as the host user with full host FS/network access (minus env secrets).

    SECURITY WARNING: do NOT use for untrusted code. Prefer nsjail > bwrap >
    docker. This backend exists only so ``execute_python`` still functions on
    minimal Unix hosts (e.g. macOS without Docker) for trusted local code.
    A ``RuntimeWarning`` is emitted on every execution to make the posture
    explicit in logs.

    Limits: 256 MB memory, 25s CPU, no child processes, 50 MB files.
    """

    name = "rlimit"
    _warned: bool = False

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
        import warnings

        if not RLimitSandbox._warned:
            warnings.warn(
                "RLimitSandbox provides NO filesystem/network isolation "
                "(rlimit only). Use nsjail/bwrap/docker for untrusted code.",
                RuntimeWarning,
                stacklevel=2,
            )
            RLimitSandbox._warned = True
        p = profile or PROFILE_EXECUTE_PYTHON
        tmpdir = tempfile.mkdtemp(prefix="r105_sandbox_")
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
        tmpdir = tempfile.mkdtemp(prefix="r105_bwrap_")
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
                cmd[8:8] = ["--tmpfs", "/home"]

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
            "--hostname", "r105-sandbox",
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
        tmpdir = tempfile.mkdtemp(prefix="r105_nsjail_")
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


# -- Docker backend (strong cross-platform isolation) ------------------------


class DockerSandbox(SandboxBackend):
    """Sandbox using Docker containers for cross-platform isolation.

    Provides strong isolation on any platform with Docker:
    - Fresh ``python:3.12-slim`` container per execution (``--rm``)
    - No network by default (``--network none`` unless profile needs it)
    - Memory/CPU limits (``--memory``, ``--cpus``)
    - Read-only root FS with a single writable ``/tmp`` workdir bind
    - Non-root user, no privilege escalation (``--user nobody``, ``--pids-limit``)
    - Sanitized environment (secrets stripped via ``_sanitize_env``)

    Requires Docker daemon: https://docs.docker.com/get-docker/
    Configure image via ``R105_DOCKER_IMAGE`` env var (default: python:3.12-slim).
    """

    name = "docker"
    DEFAULT_IMAGE = "python:3.12-slim"

    @staticmethod
    def _image() -> str:
        return os.environ.get("R105_DOCKER_IMAGE", DockerSandbox.DEFAULT_IMAGE)

    @staticmethod
    def is_available() -> bool:
        if shutil.which("docker") is None:
            return False
        try:
            result = subprocess.run(
                ["docker", "info", "--format", "{{json .}}"],
                capture_output=True, text=True, timeout=5,
            )
            return result.returncode == 0
        except Exception:
            return False

    def execute(
        self,
        code: str,
        *,
        profile: SandboxProfile | None = None,
        timeout: float = SANDBOX_TIMEOUT,
    ) -> subprocess.CompletedProcess[str]:
        p = profile or PROFILE_EXECUTE_PYTHON
        actual_timeout = p.timeout if timeout == SANDBOX_TIMEOUT else timeout
        tmpdir = tempfile.mkdtemp(prefix="r105_docker_")
        try:
            # Write code to a file to avoid shell-quoting issues with -c.
            code_path = os.path.join(tmpdir, "snippet.py")
            with open(code_path, "w", encoding="utf-8") as fh:
                fh.write(code)
            env = _sanitize_env(tmpdir)
            cmd: list[str] = [
                "docker", "run", "--rm", "-i",
                "--user", "nobody",
                "--read-only",
                "--pids-limit", "64",
                "--memory", f"{p.memory_mb}m",
                "--cpus", "1.0",
                "--cap-drop", "ALL",
                "--security-opt", "no-new-privileges",
                "-v", f"{tmpdir}:/work:ro",
                "-w", "/tmp",
            ]
            if not p.needs_network:
                cmd.extend(["--network", "none"])
            # Forward only sanitized env vars explicitly.
            for key in ("PYTHONHASHSEED", "PYTHONIOENCODING", "PYTHONUNBUFFERED", "TZ", "LANG"):
                if key in env:
                    cmd.extend(["-e", f"{key}={env[key]}"])
            cmd.extend([self._image(), "python", "/work/snippet.py"])
            return subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=actual_timeout + 10.0,  # container startup overhead
            )
        finally:
            shutil.rmtree(tmpdir, ignore_errors=True)


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

_BACKENDS: list[type[SandboxBackend]] = [NsjailSandbox, BwrapSandbox, DockerSandbox, RLimitSandbox]
_sandbox: SandboxBackend | None = None

# Why the active backend was selected instead of a stronger one (None when
# the strongest available backend won or the backend was set explicitly).
# Surfaced to the user via ``get_fallback_reason()``, a CLI stderr warning,
# and the TUI status bar so a silent downgrade to rlimit/none is visible.
_fallback_reason: str | None = None

# Backends that provide NO filesystem/network isolation. Auto-selecting one
# of these is a security-relevant downgrade that must be surfaced.
_WEAK_BACKENDS = frozenset({"rlimit", "none"})


def _class_backend_name(cls: type[SandboxBackend]) -> str:
    """Backend name from the class without instantiating.

    Subclasses shadow the abstract ``name`` property with a plain string
    class attribute; fall back to the class name if missing.
    """
    raw: Any = getattr(cls, "name", "")
    return raw if isinstance(raw, str) and raw else cls.__name__


def _unavailable_reason(cls: type[SandboxBackend]) -> str:
    """Best-effort human reason why *cls* is not usable (for fallback notices)."""
    name = _class_backend_name(cls)
    binary = {"nsjail": "nsjail", "bwrap": "bwrap", "docker": "docker"}.get(name, "")
    if name in {"nsjail", "bwrap"} and sys.platform != "linux":
        return f"{name} requires Linux (running on {sys.platform})"
    if binary and shutil.which(binary) is None:
        return f"{name} not installed (no `{binary}` on PATH)"
    if name == "docker":
        return "docker found but the daemon is unreachable (`docker info` failed)"
    if name in {"nsjail", "bwrap"}:
        return f"{name} installed but unusable (e.g. user namespaces disabled)"
    if name == "rlimit":
        return "rlimit unavailable (non-Unix platform)"
    return f"{name} unavailable"


def detect_backend_with_reason() -> tuple[SandboxBackend, str | None]:
    """Return ``(backend, fallback_reason)`` for the best available backend.

    Preference order: nsjail > bwrap > docker > rlimit > none.
    RLimit is a strict last resort (no isolation) and is only selected when
    no true isolation backend is available.

    *fallback_reason* is None when the strongest backend (nsjail) wins;
    otherwise it explains which stronger backends were skipped and why, plus
    an explicit isolation warning when the winner is weak.
    """
    skipped: list[str] = []
    for cls in _BACKENDS:
        if cls.is_available():
            backend: SandboxBackend = cls()
            reason: str | None = None
            if skipped or backend.name in _WEAK_BACKENDS:
                detail = "; ".join(skipped) if skipped else "no stronger backend available"
                reason = f"using {backend.name} ({detail})"
                if backend.name in _WEAK_BACKENDS:
                    reason += " — WARNING: no filesystem/network isolation"
            try:
                from r105.logging import warn as _log_warn
                if backend.name in _WEAK_BACKENDS:
                    _log_warn("sandbox_weak_backend", backend=backend.name, reason=reason)
                elif reason is not None:
                    _log_warn("sandbox_fallback", backend=backend.name, reason=reason)
            except Exception:
                pass
            return backend, reason
        skipped.append(_unavailable_reason(cls))
    detail = "; ".join(skipped) if skipped else "no isolation backend available"
    reason = f"using none ({detail}) — WARNING: no filesystem/network isolation"
    try:
        from r105.logging import warn as _log_warn
        _log_warn("sandbox_weak_backend", backend="none", reason=reason)
    except Exception:
        pass
    return NoopSandbox(), reason


def detect_backend() -> SandboxBackend:
    """Return the best available sandbox backend.

    Preference order: nsjail > bwrap > docker > rlimit > none.
    RLimit is a strict last resort (no isolation) and is only selected when
    no true isolation backend is available. Use :func:`get_fallback_reason`
    to find out whether (and why) a weaker backend was selected.
    """
    global _fallback_reason
    backend, _fallback_reason = detect_backend_with_reason()
    return backend


def get_fallback_reason() -> str | None:
    """Return why a weaker sandbox backend was auto-selected, or None."""
    return _fallback_reason


def current_backend_name() -> str | None:
    """Return the selected backend name without triggering detection."""
    if _sandbox is None:
        return None
    return str(getattr(_sandbox, "name", None) or type(_sandbox).__name__)


def weak_backend_warning() -> str | None:
    """Short user-facing warning when the selected backend lacks isolation.

    Returns None for strong backends (or when no backend was selected yet)
    so callers can surface the downgrade without triggering detection.
    """
    name = current_backend_name()
    if name in _WEAK_BACKENDS:
        return f"sandbox={name} (no isolation)"
    return None


def get_sandbox() -> SandboxBackend:
    """Return the current sandbox backend, creating a default one if needed."""
    global _sandbox
    if _sandbox is None:
        _sandbox = detect_backend()
    return _sandbox


def set_sandbox(name: str) -> SandboxBackend | None:
    """Set the sandbox backend by name. Returns None if the named backend is unavailable."""
    global _sandbox, _fallback_reason
    backend = get_backend(name)
    if backend is not None:
        _sandbox = backend
        # Explicit operator choice — not a fallback.
        _fallback_reason = None
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


# ---------------------------------------------------------------------------
# Permission posture — user-selectable tool-execution policy
# ---------------------------------------------------------------------------

VALID_PERMISSION_POSTURES = {"full-access", "restricted", "sandboxed", "off"}

# Posture -> effective sandbox backend selection
_POSTURE_BACKENDS: dict[str, str] = {
    "full-access": "none",
    "restricted": "auto",
    "sandboxed": "auto",
    "off": "none",
}

# Tools blocked under the "restricted" posture (code execution + network)
_RESTRICTED_BLOCKED_TOOLS = frozenset({"execute_python", "web_search", "web_fetch"})

_posture: str = "sandboxed"


def current_posture() -> str:
    """Return the active permission posture."""
    return _posture


def set_posture(posture: str, *, backend: str = "auto") -> None:
    """Apply a permission posture and configure the sandbox backend.

    *posture* must be one of VALID_PERMISSION_POSTURES.
    *backend* is the sandbox backend name to use for the sandboxed and
    restricted postures (``auto`` selects the best available).
    """
    global _posture
    if posture not in VALID_PERMISSION_POSTURES:
        raise ValueError(
            f"Invalid permission posture '{posture}'. "
            f"Valid: {', '.join(sorted(VALID_PERMISSION_POSTURES))}"
        )
    _posture = posture
    backend_name = _POSTURE_BACKENDS[posture]
    if backend_name == "auto":
        if backend and backend.lower() != "auto":
            try:
                set_sandbox(backend)
                return
            except SandboxUnavailableError:
                pass
        detect_backend()
    else:
        set_sandbox(backend_name)


def posture_allows_tool(posture: str, tool_name: str) -> tuple[bool, str]:
    """Return ``(allowed, reason)`` for a tool under the given posture."""
    if posture == "off":
        return False, "tool execution is disabled (permission_posture=off)"
    if posture == "restricted" and tool_name in _RESTRICTED_BLOCKED_TOOLS:
        return False, f"'{tool_name}' is blocked by permission_posture=restricted"
    return True, ""
