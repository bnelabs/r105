"""Skill file management — load and parameterize skill files.

Supports filesystem-watching for hot-reload via polled mtime checks
(no external dependencies required).
"""

from __future__ import annotations

import time
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from rova.state import ChatState


def list_skills(skills_dir: Path) -> list[str]:
    if not skills_dir.exists():
        return []
    return sorted(path.stem for path in skills_dir.glob("*.md") if path.is_file())


def read_skill(skills_dir: Path, name: str, params: dict[str, str] | None = None) -> str:
    """Read a skill file and optionally substitute {param} placeholders."""
    if "/" in name or "\\" in name or name.startswith("."):
        return ""
    path = skills_dir / f"{name}.md"
    if not path.is_file():
        return ""
    text = path.read_text(encoding="utf-8").strip()
    if params and text:
        for key, value in params.items():
            text = text.replace(f"{{{key}}}", value)
    return text


def skill_messages(state: ChatState) -> list[dict[str, str]]:
    """Build system messages from active skills for injection into the conversation.

    This is the canonical implementation. Both client.py and state.py use this
    so skill-loading logic stays in one place.
    """
    messages: list[dict[str, str]] = []
    for name in state.active_skills:
        params = state.skill_params.get(name)
        text = read_skill(state.skills_dir, name, params)
        if text:
            messages.append({"role": "system", "content": f"Active skill: {name}\n{text}"})
    return messages


class SkillWatcher:
    """Polling-based file watcher for the skills directory.

    Checks for new/modified/removed skill files every *poll_interval* seconds.
    No external dependencies — uses mtime polling.
    """

    def __init__(self, skills_dir: Path, poll_interval: float = 3.0) -> None:
        self._skills_dir = skills_dir
        self._poll_interval = poll_interval
        self._known: dict[str, float] = {}  # stem → mtime
        self._last_poll = 0.0

    @property
    def skills_dir(self) -> Path:
        return self._skills_dir

    def check(self) -> bool:
        """Poll the skills directory for changes.

        Returns True if a change was detected (new, modified, or deleted file),
        False otherwise.
        """
        now = time.monotonic()
        if now - self._last_poll < self._poll_interval:
            return False
        self._last_poll = now

        if not self._skills_dir.is_dir():
            changed = bool(self._known)
            self._known.clear()
            return changed

        current: dict[str, float] = {}
        changed = False
        for path in self._skills_dir.glob("*.md"):
            stem = path.stem
            mtime = path.stat().st_mtime
            current[stem] = mtime

            if stem not in self._known:
                changed = True  # new file
            elif abs(self._known[stem] - mtime) > 0.001:
                changed = True  # modified file

        # Check for deleted files
        for stem in self._known:
            if stem not in current:
                changed = True
                break

        self._known = current
        return changed

