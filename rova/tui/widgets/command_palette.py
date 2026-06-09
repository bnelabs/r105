"""Interactive command palette — shows slash commands with arrow-key navigation."""

from __future__ import annotations

from textual.widgets import Static

# Structured command definitions: (category, command, usage, description)
COMMAND_DEFS: list[tuple[str, str, str, str]] = [
    # Chat
    ("Chat", "/state", "", "Show active settings (profile, rag, quality, tokens)"),
    ("Chat", "/tokens", "", "Show estimated context usage"),
    ("Chat", "/model", "", "Show active model and context window capacity"),
    ("Chat", "/history", "", "Show last 12 messages in compact form"),
    ("Chat", "/clear", "", "Clear all conversation history"),
    ("Chat", "/compact", "", "Summarize conversation history and continue"),
    ("Chat", "/profile", "<name>", "Force a router task profile, or omit for auto"),
    ("Chat", "/quality", "fast|balanced|best", "Set quality hint metadata"),
    ("Chat", "/json", "[on|off]", "Toggle JSON object response mode"),
    ("Chat", "/max", "<tokens>", "Override max_tokens, or omit for auto"),
    ("Chat", "/autocompact", "[on|off]", "Toggle auto-compaction at 80% context"),
    # RAG
    ("RAG", "/rag", "on|off", "Toggle RAG metadata for chat requests"),
    ("RAG", "/rag ingest", "<path-or-url>...", "Ingest local files/directories or URLs"),
    ("RAG", "/rag search", "<query>", "Search the active RAG index"),
    ("RAG", "/rag list", "", "List all indexed documents"),
    ("RAG", "/rag delete", "<id>", "Remove a document from the RAG index"),
    ("RAG", "/rag update", "<path>", "Re-index specific paths"),
    # Skills
    ("Skills", "/skills", "", "List available skill files"),
    ("Skills", "/skill use", "<name> [key=val...]", "Add a skill with optional params"),
    ("Skills", "/skill drop", "<name>", "Remove one active skill"),
    ("Skills", "/skill clear", "", "Remove all active skills"),
    ("Skills", "/skill show", "<name>", "Print a skill file"),
    # Workspace
    ("Workspace", "/workspace", "", "Show workspace directory and generated files"),
    ("Workspace", "/preview", "<filename>", "Preview a workspace file"),
    # System
    ("System", "/theme", "<name>", "Switch theme (rova, dracula, solarized-dark, high-contrast)"),
    ("System", "/health", "", "Check llama-router health"),
    ("System", "/profiles", "", "List available router profiles"),
    ("System", "/help", "", "Show full command reference"),
    ("System", "/exit", "", "Quit Rova"),
]


def _fuzzy_score(candidate: str, query: str) -> int:
    """Score a candidate against a query using character contiguity.

    Characters must appear in order. Contiguous runs score higher.
    Exact prefix match gets a large bonus.
    """
    c = candidate.lower()
    q = query.lower()
    if not q:
        return 0
    if c.startswith(q):
        return 1000 + len(q) * 10
    if q in c:
        return 500 + len(q) * 5

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
        return 0
    return longest_contig * 10 + qi * 2


class CommandPalette(Static):
    """An interactive suggestion palette for slash commands.

    Shows fuzzy-matched commands with category headers and selection highlighting.
    Arrow keys (handled via ChatInput) navigate the list.
    Enter selects, Escape dismisses.
    """

    def __init__(self, **kwargs) -> None:
        super().__init__(**kwargs)
        self._selected_index: int = 0
        self._items: list[tuple[str, str, str, str]] = []  # (category, cmd, usage, desc)
        self._filter_text: str = ""

    # -- Public API -------------------------------------------------------

    @property
    def selected_index(self) -> int:
        return self._selected_index

    @property
    def item_count(self) -> int:
        return len(self._items)

    def show_commands(self, filter_text: str) -> None:
        """Filter commands and show the palette."""
        self._filter_text = filter_text
        self._items = self._filter_commands(filter_text)
        self._selected_index = 0
        self._refresh_content()
        self.add_class("-visible")

    def hide(self) -> None:
        """Hide the palette."""
        self.remove_class("-visible")

    @property
    def is_visible(self) -> bool:
        return self.has_class("-visible")

    def select_next(self) -> None:
        """Move selection down one item (wraps)."""
        if self._items:
            self._selected_index = (self._selected_index + 1) % len(self._items)
            self._refresh_content()

    def select_prev(self) -> None:
        """Move selection up one item (wraps)."""
        if self._items:
            self._selected_index = (self._selected_index - 1) % len(self._items)
            self._refresh_content()

    def get_selected(self) -> tuple[str, str, str, str] | None:
        """Return the (category, cmd, usage, desc) tuple for the highlighted item."""
        if self._items and 0 <= self._selected_index < len(self._items):
            return self._items[self._selected_index]
        return None

    def get_selected_command(self) -> str | None:
        """Return the command string of the highlighted item."""
        selected = self.get_selected()
        if selected:
            cmd = selected[1]
            usage = selected[2]
            return f"{cmd} {usage}".strip()
        return None

    # -- Internal ---------------------------------------------------------

    def _refresh_content(self) -> None:
        """Rebuild the palette content with selection highlight and category headers."""
        if not self._items:
            if self._filter_text and self._filter_text != "/":
                self.update(
                    f"[dim]no commands matching '{self._filter_text}'[/dim]"
                )
            else:
                self.update("")
            return

        lines: list[str] = []
        last_category: str | None = None

        for i, (category, cmd, usage, desc) in enumerate(self._items):
            # Add category header when entering a new category
            if category != last_category:
                if lines:
                    lines.append("")  # blank line between categories
                lines.append(f"[bold #89b4fa]── {category} ──[/bold #89b4fa]")
                last_category = category

            usage_str = f" {usage}" if usage else ""
            if i == self._selected_index:
                # Highlighted: mauve arrow + bold command
                lines.append(
                    f"[bold #cba6f7]▶ {cmd}{usage_str}[/bold #cba6f7]  "
                    f"[dim #6c7086]{desc}[/dim #6c7086]"
                )
            else:
                lines.append(
                    f"  [bold]{cmd}{usage_str}[/bold]  [dim]{desc}[/dim]"
                )

        # Add hint footer
        lines.append("")
        lines.append(
            "[dim #585b70]↑↓ navigate  ↵ select  esc dismiss  tab autocomplete[/dim #585b70]"
        )

        self.update("\n".join(lines))

    def _filter_commands(
        self, filter_text: str
    ) -> list[tuple[str, str, str, str]]:
        """Return commands matching the filter, best first, grouped by category."""
        query = filter_text.strip()
        # Show all commands when just "/" is typed
        if not query or query == "/":
            return list(COMMAND_DEFS)

        # Fuzzy-match against command name, description, and usage
        scored: list[tuple[int, str, str, str, str]] = []
        for category, cmd, usage, desc in COMMAND_DEFS:
            score = _fuzzy_score(cmd, query)
            if score <= 0:
                score = _fuzzy_score(desc, query)
            if score <= 0 and usage:
                score = _fuzzy_score(usage, query)
            if score > 0:
                scored.append((score, category, cmd, usage, desc))

        scored.sort(key=lambda x: x[0], reverse=True)
        return [(cat, cmd, usage, desc) for _, cat, cmd, usage, desc in scored]
