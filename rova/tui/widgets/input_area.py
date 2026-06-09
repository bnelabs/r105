"""Input area for chat messages with slash-command support."""

from __future__ import annotations

from textual.widgets import Input
from textual.message import Message


class ChatInput(Input):
    """Single-line input that emits Submitted on Enter."""

    class Submitted(Message):
        """Emitted when the user submits (Enter)."""

        def __init__(self, value: str) -> None:
            super().__init__()
            self.value = value

    def __init__(self, **kwargs) -> None:
        super().__init__(
            placeholder="Type a message or / for commands…",
            **kwargs,
        )

    def on_input_submitted(self, event: Input.Submitted) -> None:
        event.stop()
        self.post_message(self.Submitted(event.value))
