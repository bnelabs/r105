"""File explorer widget — tree view of the workspace directory.

Uses Textual's built-in DirectoryTree for async lazy-loading,
so expanding large directories won't freeze the UI.
"""

from __future__ import annotations

from pathlib import Path

from textual.message import Message
from textual.widgets import DirectoryTree


class FileExplorer(DirectoryTree):
    """A directory tree widget showing the workspace file structure.

    Uses Textual's DirectoryTree which lazy-loads directories asynchronously,
    handles filesystem permissions, and provides standard UI hooks.

    Click a file to preview it in the chat view.
    """

    class FileSelected(Message):
        """Posted when a file is selected (clicked)."""

        def __init__(self, path: Path) -> None:
            super().__init__()
            self.path = path

    def __init__(self, workspace_dir: Path, **kwargs) -> None:
        super().__init__(workspace_dir, **kwargs)

    def refresh_tree(self) -> None:
        """Rebuild the tree (e.g., after a file is created)."""
        self.reload()

    def on_directory_tree_file_selected(
        self, event: DirectoryTree.FileSelected
    ) -> None:
        """Post a message when a file node is selected."""
        self.post_message(self.FileSelected(event.path))
