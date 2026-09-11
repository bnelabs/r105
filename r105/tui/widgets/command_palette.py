"""Interactive command palette — shows slash commands with arrow-key navigation."""

from __future__ import annotations

from textual.geometry import Region, Spacing
from textual.widgets import OptionList

# Structured command definitions: (category, command, usage, description)
COMMAND_DEFS: list[tuple[str, str, str, str]] = [
    # Chat
    ("Chat", "/state", "", "Show active settings (profile, quality, tokens)"),
    ("Chat", "/tokens", "", "Show context usage, source, and confidence"),
    ("Chat", "/model", "[name]", "Show current model, list available, or switch models"),
    ("Chat", "/history", "", "Show last 12 messages in compact form"),
    ("Chat", "/clear", "", "Clear all conversation history"),
    ("Chat", "/compact", "", "Summarize conversation history and continue"),
    ("Chat", "/profile", "<name>", "Force a router task profile, or omit for auto"),
    ("Chat", "/quality", "fast|balanced|best", "Set quality hint metadata"),
    ("Chat", "/json", "[on|off]", "Toggle JSON object response mode"),
    ("Chat", "/max", "<tokens>", "Override max_tokens, or omit for auto"),
    ("Chat", "/cache-prompt", "[on|off]", "Enable llama.cpp prompt-prefix caching"),
    ("Chat", "/config", "reload|show", "Reload or inspect config.json"),
    ("Chat", "/autocompact", "[on|off]", "Toggle auto-compaction at 80% context"),
    ("Chat", "/reasoning", "auto|off|low|medium|high", "Set reasoning effort (sent to model-capable backends)"),
    ("Chat", "/permissions", "full-access|restricted|sandboxed|off", "Set permission posture for tool execution"),
    ("System", "/connect", "<provider> [base-url]", "Connect to a local or cloud OpenAI-compatible provider"),
    ("System", "/provider", "<provider> [base-url]", "Alias for /connect"),
    # Skills
    ("Skills", "/skills", "", "List available skill files"),
    ("Skills", "/skill use", "<name> [key=val...]", "Add a skill with optional params"),
    ("Skills", "/skill drop", "<name>", "Remove one active skill"),
    ("Skills", "/skill clear", "", "Remove all active skills"),
    ("Skills", "/skill show", "<name>", "Print a skill file"),
    # Workspace
    ("Workspace", "/workspace", "", "Show workspace directory and generated files"),
    ("Workspace", "/preview", "<filename>", "Preview a workspace file"),
    # Sessions
    ("Sessions", "/session save", "<name>", "Save conversation to a session file"),
    ("Sessions", "/session load", "<name>", "Load and restore a saved session"),
    ("Sessions", "/session list", "", "List saved sessions"),
    ("Sessions", "/session delete", "<name>", "Delete a saved session"),
    ("Sessions", "/export", "text|markdown|json|html", "Export conversation to a file"),
    # Plugins
    ("Plugins", "/plugin list", "", "List loaded custom tool plugins"),
    ("Plugins", "/plugin reload", "", "Reload plugins from disk"),
    # MCP
    ("MCP", "/mcp list", "", "List connected MCP servers"),
    ("MCP", "/mcp tools", "<server>", "List tools from an MCP server"),
    ("MCP", "/mcp reconnect", "<server>", "Reconnect and rediscover an MCP server"),
    # System
    ("System", "/theme", "<name>", "Switch theme (r105, dracula, solarized-dark, high-contrast)"),
    ("System", "/health", "", "Check llama-router health"),
    ("System", "/profiles", "", "List available router profiles"),
    ("System", "/help", "", "Show full command reference"),
    ("System", "/exit", "", "Quit r105"),
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


class CommandPalette(OptionList):
    """An interactive suggestion palette for slash commands.

    Shows fuzzy-matched commands with category headers and selection highlighting.
    Arrow keys (handled via ChatInput) navigate the list.
    Enter selects, Escape dismisses.

    ``OptionList`` is used instead of a text widget so the palette exposes its
    full command list as virtual content. This gives Textual a real scroll
    range when the list is taller than the docked palette.
    """

    def __init__(self, **kwargs) -> None:
        super().__init__(**kwargs)
        self._selected_index: int = 0
        self._items: list[tuple[str, str, str, str]] = []  # (category, cmd, usage, desc)
        self._filter_text: str = ""
        self._command_option_indexes: list[int] = []

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
            self._set_selected((self._selected_index + 1) % len(self._items))

    def select_prev(self) -> None:
        """Move selection up one item (wraps)."""
        if self._items:
            self._set_selected((self._selected_index - 1) % len(self._items))

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
        """Rebuild the palette options with category headers and footer."""
        if not self._items:
            message = ""
            if self._filter_text and self._filter_text != "/":
                message = f"[dim]no commands matching '{self._filter_text}'[/dim]"
            self._command_option_indexes = []
            self.set_options([message] if message else [])
            if message:
                self.disable_option_at_index(0)
            self.call_after_refresh(self._scroll_to_start)
            return

        options: list[str] = []
        disabled_indexes: list[int] = []
        command_option_indexes: list[int] = []
        last_category: str | None = None

        for i, (category, cmd, usage, desc) in enumerate(self._items):
            # Add category header when entering a new category
            if category != last_category:
                if options:
                    options.append("")  # blank line between categories
                    disabled_indexes.append(len(options) - 1)
                options.append(f"[bold #89b4fa]── {category} ──[/bold #89b4fa]")
                disabled_indexes.append(len(options) - 1)
                last_category = category

            command_option_indexes.append(len(options))
            options.append(
                self._render_command(
                    cmd,
                    usage,
                    desc,
                    selected=i == self._selected_index,
                )
            )

        # Add hint footer
        options.append("")
        disabled_indexes.append(len(options) - 1)
        options.append(
            "[dim #585b70]↑↓ navigate  ↵ select  esc dismiss  tab autocomplete[/dim #585b70]"
        )
        disabled_indexes.append(len(options) - 1)

        self._command_option_indexes = command_option_indexes
        self.set_options(options)
        for option_index in disabled_indexes:
            self.disable_option_at_index(option_index)
        self.highlighted = self._command_option_indexes[self._selected_index]
        self.call_after_refresh(self.scroll_to_highlight)

    @staticmethod
    def _render_command(
        cmd: str, usage: str, desc: str, *, selected: bool
    ) -> str:
        """Render one selectable command option."""
        usage_str = f" {usage}" if usage else ""
        if selected:
            return (
                f"[bold #cba6f7]▶ {cmd}{usage_str}[/bold #cba6f7]  "
                f"[dim #6c7086]{desc}[/dim #6c7086]"
            )
        return f"  [bold]{cmd}{usage_str}[/bold]  [dim]{desc}[/dim]"

    def _set_selected(self, selected_index: int) -> None:
        """Update the selected command and reveal it in the viewport."""
        if not self._items:
            return
        previous_index = self._selected_index
        self._selected_index = selected_index

        if self._command_option_indexes:
            previous_option = self._command_option_indexes[previous_index]
            selected_option = self._command_option_indexes[selected_index]
            previous = self._items[previous_index]
            current = self._items[selected_index]
            self.replace_option_prompt_at_index(
                previous_option,
                self._render_command(*previous[1:], selected=False),
            )
            self.replace_option_prompt_at_index(
                selected_option,
                self._render_command(*current[1:], selected=True),
            )
            self.highlighted = selected_option
            self.call_after_refresh(self.scroll_to_highlight)

    def scroll_to_highlight(self, top: bool = False) -> None:
        """Keep the highlighted command one row clear of the bottom border.

        ``OptionList.scroll_to_highlight`` considers an option whose bottom
        edge touches the viewport edge visible. On some terminal renderers,
        including Windows consoles, that row is drawn against the border and
        appears clipped. Reserve one row below the highlighted option so the
        selection remains readable when navigating down.
        """
        highlighted = self.highlighted
        if highlighted is None or not self.is_mounted:
            return

        self._update_lines()
        try:
            y = self._index_to_line[highlighted]
            height = self._heights[highlighted]
        except KeyError:
            return

        self.scroll_to_region(
            Region(0, y, self.scrollable_content_region.width, height),
            spacing=Spacing(bottom=1),
            force=True,
            animate=False,
            top=top,
            immediate=True,
            x_axis=False,
        )

    def _selected_line_index(self) -> int:
        """Return the rendered line containing the selected command."""
        if not self._items or not self._command_option_indexes:
            return 0
        option_index = self._command_option_indexes[self._selected_index]
        try:
            return self._index_to_line.get(option_index, option_index)
        except (AttributeError, RuntimeError):
            return option_index

    def _scroll_to_start(self) -> None:
        """Reset stale scroll when filtering leaves no visible commands."""
        try:
            self.scroll_home(animate=False, immediate=True, x_axis=False)
        except (AttributeError, RuntimeError):
            return

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
