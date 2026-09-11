"""Structured logging for r105 — JSON log output for debugging async operations.

Logs are written to ``~/.local/state/r105/log.jsonl`` in newline-delimited JSON
format, making them easy to tail, grep, and ingest into log aggregators.

Usage::

    from r105.logging import log
    log.info("send_completed", wall_seconds=2.3, tokens=420)
    log.error("api_error", status_code=500, method="POST")

Safe to call from async contexts — writes are thread-safe via the logging
module's internal locks.
"""

from __future__ import annotations

import json
import logging
import os
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

LOGGER_NAME = "r105"
_LOG_DIR = Path.home() / ".local" / "state" / "r105"
_LOG_FILE = _LOG_DIR / "log.jsonl"
_LEVEL = os.environ.get("R105_LOG_LEVEL", "INFO").upper()
_TRACE_ID: ContextVar[str | None] = ContextVar("r105_trace_id", default=None)


def current_trace_id() -> str | None:
    """Return the correlation id attached to the current task, if any."""
    return _TRACE_ID.get()


@contextmanager
def trace_context(trace_id: str | None) -> Iterator[None]:
    """Attach *trace_id* to log records emitted in this execution context."""
    token = _TRACE_ID.set(trace_id)
    try:
        yield
    finally:
        _TRACE_ID.reset(token)


def _with_trace_id(extra: dict[str, Any]) -> dict[str, Any]:
    """Add the active correlation id without overwriting an explicit value."""
    fields = dict(extra)
    trace_id = fields.get("trace_id") or current_trace_id()
    if trace_id:
        fields["trace_id"] = trace_id
    return fields


class JSONFormatter(logging.Formatter):
    """Format log records as single-line JSON objects."""

    def format(self, record: logging.LogRecord) -> str:
        entry: dict[str, Any] = {
            "ts": datetime.now(UTC).isoformat(),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
        }
        # Include extra fields passed via extra={}
        for key, value in getattr(record, "extra_fields", {}).items():
            entry[key] = value
        if record.exc_info and record.exc_info[0]:
            entry["exception"] = f"{record.exc_info[0].__name__}: {record.exc_info[1]}"
        return json.dumps(entry, sort_keys=True, default=str)


def _setup() -> logging.Logger:
    """Configure the r105 logger with JSON file output and console fallback."""
    logger = logging.getLogger(LOGGER_NAME)
    logger.setLevel(_LEVEL)

    # File handler — JSON lines format
    try:
        _LOG_DIR.mkdir(parents=True, exist_ok=True)
        fh = logging.FileHandler(str(_LOG_FILE), encoding="utf-8")
        fh.setFormatter(JSONFormatter())
        logger.addHandler(fh)
    except OSError:
        pass  # fall back to stderr

    return logger


# Module-level logger singleton
_logger: logging.Logger | None = None


def _get_logger() -> logging.Logger:
    global _logger
    if _logger is None:
        _logger = _setup()
    return _logger


# -- Convenience functions --------------------------------------------------


def info(message: str, **extra: Any) -> None:
    _get_logger().info(message, extra={"extra_fields": _with_trace_id(extra)})


def warn(message: str, **extra: Any) -> None:
    _get_logger().warning(message, extra={"extra_fields": _with_trace_id(extra)})


def error(message: str, **extra: Any) -> None:
    _get_logger().error(message, extra={"extra_fields": _with_trace_id(extra)})


def debug(message: str, **extra: Any) -> None:
    _get_logger().debug(message, extra={"extra_fields": _with_trace_id(extra)})
