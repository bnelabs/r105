"""Environment diagnostics — the ``r105 doctor`` command.

Runs a series of checks (config validity, sandbox availability, backend
reachability, workspace writability) and reports pass/fail per check.
Pure logic takes injected probes so tests never touch the network.
"""

from __future__ import annotations

import os
import platform
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class DoctorCheck:
    """One diagnostic result."""

    name: str
    ok: bool
    detail: str = ""


@dataclass
class DoctorReport:
    """Full diagnostic report."""

    checks: list[DoctorCheck] = field(default_factory=list)

    @property
    def passed(self) -> bool:
        return all(c.ok for c in self.checks)

    def render(self) -> str:
        lines = []
        for check in self.checks:
            mark = "ok" if check.ok else "FAIL"
            line = f"[{mark}] {check.name}"
            if check.detail:
                line += f" — {check.detail}"
            lines.append(line)
        lines.append("all checks passed" if self.passed else "issues found")
        return "\n".join(lines)


def run_doctor(
    *,
    version: str,
    config: dict[str, Any] | None,
    config_error: str | None,
    config_path: str,
    sandbox_backends: list[tuple[str, bool, str]],
    selected_backend: str,
    fallback_reason: str | None,
    backend_health: dict[str, Any] | None,
    backend_error: str | None,
    backend_url: str,
    workspace_dir: Path,
    workspace_writable: bool,
    skills_dir: Path,
    skills_count: int,
    api_keys: list[str],
) -> DoctorReport:
    """Assemble a report from pre-gathered probe values (no I/O here)."""
    report = DoctorReport()
    report.checks.append(DoctorCheck(
        "python", sys.version_info >= (3, 12),
        f"{platform.python_version()} (requires >= 3.12)",
    ))
    report.checks.append(DoctorCheck("r105", True, f"version {version}"))
    if config_error is not None:
        report.checks.append(DoctorCheck("config", False, f"{config_path}: {config_error}"))
    elif config is None:
        report.checks.append(DoctorCheck("config", False, f"{config_path}: unreadable"))
    else:
        report.checks.append(DoctorCheck("config", True, config_path))
    for backend_name, available, reason in sandbox_backends:
        marker = "selected" if backend_name == selected_backend else "available"
        if available:
            report.checks.append(DoctorCheck(
                f"sandbox:{backend_name}", True,
                marker if backend_name == selected_backend else reason or "usable",
            ))
        else:
            report.checks.append(DoctorCheck(
                f"sandbox:{backend_name}", True, f"unavailable ({reason})",
            ))
    if fallback_reason:
        report.checks.append(DoctorCheck("sandbox:fallback", False, fallback_reason))
    else:
        report.checks.append(DoctorCheck(
            "sandbox:fallback", True, f"no downgrade (using {selected_backend})",
        ))
    if backend_error is not None:
        report.checks.append(DoctorCheck(
            "backend", False, f"{backend_url}: {backend_error}",
        ))
    elif backend_health is not None:
        ok = bool(backend_health.get("ok"))
        report.checks.append(DoctorCheck(
            "backend", ok,
            f"{backend_url} models={backend_health.get('models_available', '?')}"
            if ok else f"{backend_url}: {backend_health.get('error', 'unhealthy')}",
        ))
    else:
        report.checks.append(DoctorCheck("backend", True, f"{backend_url} (unchecked)"))
    report.checks.append(DoctorCheck(
        "workspace", workspace_writable,
        f"{workspace_dir} writable" if workspace_writable else f"{workspace_dir} NOT writable",
    ))
    report.checks.append(DoctorCheck(
        "skills", True, f"{skills_count} skill(s) in {skills_dir}",
    ))
    if api_keys:
        report.checks.append(DoctorCheck(
            "api keys", True, f"set: {', '.join(api_keys)}",
        ))
    else:
        report.checks.append(DoctorCheck(
            "api keys", True, "none set (local backends only)",
        ))
    return report


def collect_probes(
    *,
    client: Any,
    backend_url: str,
    workspace_dir: Path,
    skills_dir: Path,
    check_backend: bool = True,
) -> dict[str, Any]:
    """Gather live probe values for :func:`run_doctor` (does I/O)."""
    from r105 import __version__
    from r105.config import CONFIG_DIR
    from r105.sandbox import (
        _BACKENDS,
        _class_backend_name,
        _unavailable_reason,
        current_backend_name,
        get_fallback_reason,
    )
    from r105.skills import list_skills

    try:
        from r105.config import ensure_config
        config: dict[str, Any] | None = ensure_config()
        config_error: str | None = None
    except Exception as exc:
        config, config_error = None, str(exc)

    sandbox_backends: list[tuple[str, bool, str]] = []
    for cls in _BACKENDS:
        name = _class_backend_name(cls)
        try:
            available = cls.is_available()
        except Exception:
            available = False
        reason = "" if available else (_unavailable_reason(cls) or "unavailable")
        sandbox_backends.append((name, available, reason))

    try:
        fallback_reason = get_fallback_reason()
    except Exception:
        fallback_reason = None
    try:
        selected = current_backend_name() or "unknown"
    except Exception:
        selected = "unknown"

    backend_health: dict[str, Any] | None = None
    backend_error: str | None = None
    if check_backend and client is not None:
        try:
            backend_health = client.health()
        except Exception as exc:
            backend_error = str(exc)

    try:
        probe = workspace_dir / ".r105_write_probe"
        probe.write_text("ok")
        probe.unlink()
        writable = True
    except OSError:
        writable = False

    try:
        skills_count = len(list_skills(skills_dir))
    except Exception:
        skills_count = 0

    api_keys = [k for k in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY")
                if os.environ.get(k)]

    return {
        "version": __version__,
        "config": config,
        "config_error": config_error,
        "config_path": str(Path(CONFIG_DIR) / "config.json"),
        "sandbox_backends": sandbox_backends,
        "selected_backend": selected,
        "fallback_reason": fallback_reason,
        "backend_health": backend_health,
        "backend_error": backend_error,
        "backend_url": backend_url,
        "workspace_dir": workspace_dir,
        "workspace_writable": writable,
        "skills_dir": skills_dir,
        "skills_count": skills_count,
        "api_keys": api_keys,
    }
