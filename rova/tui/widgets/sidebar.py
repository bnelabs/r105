"""Sidebar showing state, workspace files, and skills."""

from __future__ import annotations

from pathlib import Path

from textual.app import ComposeResult
from textual.containers import VerticalScroll
from textual.widgets import Static

from rova.state import ChatState, token_usage
from rova.skills import list_skills


class Sidebar(VerticalScroll):
    """Right sidebar with state info, workspace listing, and skills."""

    def compose(self) -> ComposeResult:
        yield Static("", id="sidebar-state")
        yield Static("", id="sidebar-workspace")
        yield Static("", id="sidebar-skills")

    def refresh_state(self, state: ChatState, workspace_dir: Path) -> None:
        usage = token_usage(state)

        state_text = (
            f"[bold]State[/bold]\n"
            f"profile: {state.profile or 'auto'}\n"
            f"rag:     {state.rag if state.rag is not None else 'auto'}\n"
            f"quality: {state.quality or 'auto'}\n"
            f"max_tok: {state.max_tokens or 'auto'}\n"
            f"json:    {state.json_mode}\n"
            f"ctx:     {usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)\n"
            f"turns:   {len(state.history) // 2}"
        )

        workspace_text = self._workspace_text(workspace_dir)
        skills_text = self._skills_text(state)

        try:
            self.query_one("#sidebar-state", Static).update(state_text)
            self.query_one("#sidebar-workspace", Static).update(workspace_text)
            self.query_one("#sidebar-skills", Static).update(skills_text)
        except Exception:
            pass

    def _workspace_text(self, workspace_dir: Path) -> str:
        if not workspace_dir.exists():
            return "[bold]Workspace[/bold]\n(dir not found)"
        files = sorted(
            f for f in workspace_dir.iterdir() if f.name != ".gitkeep"
        )
        if not files:
            return "[bold]Workspace[/bold]\n(empty)"
        lines = ["[bold]Workspace[/bold]"]
        for f in files[:20]:
            size = f.stat().st_size
            lines.append(f"  {f.name} ({_fmt_size(size)})")
        if len(files) > 20:
            lines.append(f"  ... +{len(files) - 20} more")
        return "\n".join(lines)

    def _skills_text(self, state: ChatState) -> str:
        all_skills = list_skills(state.skills_dir)
        active = set(state.active_skills)
        if not all_skills:
            return "[bold]Skills[/bold]\n(none)"
        lines = ["[bold]Skills[/bold]"]
        for name in all_skills:
            marker = "●" if name in active else "○"
            lines.append(f"  {marker} {name}")
        return "\n".join(lines)


def _fmt_size(size: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if size < 1024:
            return f"{size}{unit}"
        size //= 1024
    return f"{size}TB"
