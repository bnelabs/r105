"""DiffView widget — shows file diffs with approve/reject controls."""

from __future__ import annotations

from pathlib import Path

from textual import on
from textual.app import ComposeResult
from textual.containers import Horizontal, VerticalScroll
from textual.widgets import Button, Static


class DiffView(Static):
    """A widget that displays a file diff with approve/reject buttons.

    Emits DiffView.Approved or DiffView.Rejected when the user responds.
    """

    class Approved(Static.Message):
        """Posted when the user approves the diff."""

    class Rejected(Static.Message):
        """Posted when the user rejects the diff."""

    def __init__(
        self,
        file_path: Path,
        diff_text: str,
        *,
        is_new_file: bool = False,
    ) -> None:
        super().__init__()
        self.file_path = file_path
        self.diff_text = diff_text
        self.is_new_file = is_new_file

    def on_mount(self) -> None:
        self._render_diff()

    def compose(self) -> ComposeResult:
        yield Static(id="diff-content")
        with Horizontal(id="diff-actions"):
            yield Button("✅ Approve", variant="primary", id="diff-approve")
            yield Button("❌ Reject", variant="error", id="diff-reject")

    def _render_diff(self) -> None:
        content = self.query_one("#diff-content", Static)
        lines: list[str] = []

        if self.is_new_file:
            lines.append(f"[bold]New file: {self.file_path.name}[/bold]")
            lines.append(f"[dim]Location: {self.file_path}[/dim]")
            lines.append("")
            lines.append(self.diff_text)
        else:
            lines.append(f"[bold]Changes to: {self.file_path.name}[/bold]")
            lines.append(f"[dim]Location: {self.file_path}[/dim]")
            lines.append("")
            for line in self.diff_text.splitlines():
                if line.startswith("+++") or line.startswith("---"):
                    continue  # skip diff headers for cleaner view
                if line.startswith("@@"):
                    lines.append(f"[dim]{line}[/dim]")
                elif line.startswith("+"):
                    lines.append(f"[bold #a6e3a1]{line}[/bold #a6e3a1]")
                elif line.startswith("-"):
                    lines.append(f"[bold #f38ba8]{line}[/bold #f38ba8]")
                else:
                    lines.append(line)

        lines.append("")
        lines.append("[dim]Review the changes above, then approve or reject.[/dim]")
        content.update("\n".join(lines))

    @on(Button.Pressed, "#diff-approve")
    def _on_approve(self) -> None:
        self.emit_compose_message(DiffView.Approved(self))

    @on(Button.Pressed, "#diff-reject")
    def _on_reject(self) -> None:
        self.emit_compose_message(DiffView.Rejected(self))
