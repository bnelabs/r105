"""Multi-line chat input with history, fuzzy autocomplete, and slash palette.

Features:
  - Enter to submit, Shift+Enter for newline
  - Up/Down arrow key history navigation (normal mode)
  - Up/Down arrow key palette navigation (slash mode: when text starts with /)
  - Enter selects from palette (slash mode), submits message (normal mode)
  - Escape dismisses palette and clears slash input
  - Fuzzy slash-command matching on Tab
  - Slash detection via TextArea.Changed event (no polling)
"""

from __future__ import annotations

from textual.binding import Binding
from textual.message import Message
from textual.widgets import TextArea

from rova.tui.widgets.command_palette import COMMAND_DEFS


class ChatInput(TextArea):
    """Multi-line text input for the chat interface.

    Two modes:
      Normal  — Enter submits, Up/Down navigate history
      Slash   — text starts with /, palette is visible:
                Up/Down navigate palette, Enter selects, Escape dismisses

    Posts:
      ChatSubmitted  — when user presses Enter with non-empty text (normal mode)
      SlashChanged   — when text changes while starting with /
      SlashNavigate  — when user presses Up/Down in slash mode
      SlashSelect    — when user presses Enter in slash mode
      SlashDismiss   — when user presses Escape in slash mode
    """

    BINDINGS = [
        Binding("enter", "submit", "Submit", show=False),
        Binding("escape", "dismiss_slash", "Dismiss", show=False),
    ]

    class ChatSubmitted(Message):
        """Emitted on Enter with the trimmed text (normal mode)."""

        def __init__(self, value: str) -> None:
            super().__init__()
            self.value = value

    class SlashChanged(Message):
        """Emitted when text changes while starting with /."""

        def __init__(self, value: str) -> None:
            super().__init__()
            self.value = value

    class SlashNavigate(Message):
        """Emitted when user presses Up/Down in slash mode."""

        def __init__(self, direction: int) -> None:
            super().__init__()
            self.direction = direction  # -1 for up, 1 for down

    class SlashSelect(Message):
        """Emitted when user presses Enter in slash mode."""

        pass

    class SlashDismiss(Message):
        """Emitted when user presses Escape in slash mode."""

        pass

    def __init__(self, **kwargs) -> None:
        super().__init__(
            text="",
            show_line_numbers=False,
            tab_behavior="focus",
            **kwargs,
        )
        self._history: list[str] = []
        self._history_index: int = -1
        self._scratch: str = ""  # saved current text when navigating history

    # -- Properties -------------------------------------------------------

    @property
    def in_slash_mode(self) -> bool:
        """True when the current text starts with /."""
        return self.text.startswith("/")

    # -- Submit -----------------------------------------------------------

    def action_submit(self) -> None:
        """Enter key: select from palette (slash mode) or submit (normal mode)."""
        value = self.text
        if self.in_slash_mode:
            self.post_message(self.SlashSelect())
            return

        # Normal mode: submit the message
        if value.strip():
            self._history.append(value)
            if len(self._history) > 200:
                self._history.pop(0)
            self._history_index = -1
            self._scratch = ""
            self.post_message(self.ChatSubmitted(value))
        self.clear()

    # -- Slash dismiss ----------------------------------------------------

    def action_dismiss_slash(self) -> None:
        """Escape key: dismiss palette and clear slash input."""
        if self.in_slash_mode:
            self.post_message(self.SlashDismiss())
            self.clear()

    # -- Slash detection (event-driven, no polling) ------------------------

    def on_text_area_changed(self, event: TextArea.Changed) -> None:
        """React to text changes — post SlashChanged when text starts with /."""
        text = self.text
        if text.startswith("/"):
            self.post_message(self.SlashChanged(text))
        else:
            self.post_message(self.SlashChanged(""))

    # -- Arrow-key handling -----------------------------------------------

    def action_cursor_up(self, select: bool = False) -> None:
        """Up arrow: navigate palette (slash mode) or history (normal mode)."""
        if self.in_slash_mode:
            self.post_message(self.SlashNavigate(-1))
            return

        # Normal mode: history navigation
        row, _col = self.cursor_location
        if row == 0 and self._history:
            self._navigate_history(-1)
        else:
            super().action_cursor_up()

    def action_cursor_down(self, select: bool = False) -> None:
        """Down arrow: navigate palette (slash mode) or history (normal mode)."""
        if self.in_slash_mode:
            self.post_message(self.SlashNavigate(1))
            return

        # Normal mode: history navigation
        row, _col = self.cursor_location
        last_row = self.document.line_count - 1
        if row >= last_row and self._history:
            self._navigate_history(1)
        else:
            super().action_cursor_down()

    def _navigate_history(self, direction: int) -> None:
        """Cycle through command history."""
        if self._history_index == -1:
            self._scratch = self.text
            if direction == -1:
                self._history_index = len(self._history) - 1
            else:
                return  # nothing to go "down" to from scratch

        new_index = self._history_index + direction
        if 0 <= new_index < len(self._history):
            self._history_index = new_index
            self.text = self._history[self._history_index]
        elif new_index >= len(self._history):
            # Past the end: restore scratch
            self._history_index = -1
            self.text = self._scratch

    # -- Tab autocomplete (fuzzy) -----------------------------------------

    def action_focus_next(self) -> None:
        """Tab: fuzzy-autocomplete slash commands, else move focus."""
        if self.text.startswith("/"):
            # If arguments are present, suppress focus shift (user is mid-command)
            if " " in self.text.strip():
                return
            match = _fuzzy_best_match(self.text)
            if match:
                self.text = match
                self.cursor_location = (self.document.line_count - 1, len(match))
                self.post_message(self.SlashChanged(match))
                return
        self.screen.action_focus_next()  # type: ignore[attr-defined]


# -- Fuzzy matching ----------------------------------------------------------

def _fuzzy_score(candidate: str, query: str) -> int:
    """Score a candidate against a query using character contiguity.

    Returns a score where higher = better match:
      - characters must appear in order in the candidate
      - contiguous runs are heavily weighted
      - exact prefix match gets a large bonus
    """
    c = candidate.lower()
    q = query.lower()
    if not q:
        return 0
    if c.startswith(q):
        return 1000 + len(q) * 10  # strong prefix bonus
    if q in c:
        return 500 + len(q) * 5  # substring bonus

    # Character-by-character ordered matching
    qi = 0
    last_match = -1
    longest_contig = 0
    current_contig = 0
    for i, ch in enumerate(c):
        if qi < len(q) and ch == q[qi]:
            qi += 1
            if last_match >= 0 and i == last_match + 1:
                current_contig += 1
            else:
                current_contig = 1
            longest_contig = max(longest_contig, current_contig)
            last_match = i
    if qi < len(q):
        return 0  # not all chars matched
    return longest_contig * 10 + qi * 2  # contiguity-weighted


def _fuzzy_best_match(partial: str) -> str | None:
    """Return the best fuzzy-matching command for autocomplete."""
    prefix = partial.lstrip("/").lower()
    scored: list[tuple[int, str]] = []
    for _cat, cmd, _usage, _desc in COMMAND_DEFS:
        cmd_name = cmd.lstrip("/").lower()
        if prefix and prefix not in cmd_name:
            # Also try fuzzy
            score = _fuzzy_score(cmd, partial)
            if score <= 0:
                # Fallback: check if search term appears anywhere
                if prefix not in cmd_name and prefix not in cmd:
                    continue
                score = 1
        else:
            score = _fuzzy_score(cmd, partial)

        if score > 0:
            scored.append((score, cmd))

    if not scored:
        return None

    scored.sort(key=lambda x: x[0], reverse=True)
    best = scored[0][1]

    # If only one match, add trailing space
    if len(scored) == 1:
        return best + " "
    return best
