"""Structured logging for Rova — JSON log output for debugging async operations.

Logs are written to ``~/.local/state/rova/log.jsonl`` in newline-delimited JSON
format, making them easy to tail, grep, and ingest into log aggregators.

Usage::

    from rova.logging import log
    log.info("send_completed", wall_seconds=2.3, tokens=420)
    log.error("api_error", status_code=500, method="POST")

Safe to call from async contexts — writes are thread-safe via the logging
module's internal locks.
"""

from __future__ import annotations

import json
import logging
import os
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LOGGER_NAME = "rova"
_LOG_DIR = Path.home() / ".local" / "state" / "rova"
_LOG_FILE = _LOG_DIR / "log.jsonl"
_LEVEL = os.environ.get("ROVA_LOG_LEVEL", "INFO").upper()


class JSONFormatter(logging.Formatter):
    """Format log records as single-line JSON objects."""

    def format(self, record: logging.LogRecord) -> str:
        entry: dict[str, Any] = {
            "ts": datetime.now(timezone.utc).isoformat(),
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
    """Configure the rova logger with JSON file output and console fallback."""
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
    _get_logger().info(message, extra={"extra_fields": extra})


def warn(message: str, **extra: Any) -> None:
    _get_logger().warning(message, extra={"extra_fields": extra})


def error(message: str, **extra: Any) -> None:
    _get_logger().error(message, extra={"extra_fields": extra})


def debug(message: str, **extra: Any) -> None:
    _get_logger().debug(message, extra={"extra_fields": extra})
