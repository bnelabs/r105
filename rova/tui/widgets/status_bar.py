"""Bottom status bar showing profile, RAG, quality, and context usage."""

from __future__ import annotations

from textual.widgets import Static

from rova.state import ChatState, TokenUsage


class StatusBarWidget(Static):
    """Footer bar showing current settings and context usage."""

    def __init__(self, **kwargs) -> None:
        super().__init__("", **kwargs)

    def update_status(self, state: ChatState, usage: TokenUsage) -> None:
        profile = state.profile or "auto"
        rag = "rag" if state.rag else "no-rag"
        skills = f"+{len(state.active_skills)} skill" if state.active_skills else "plain"
        ctx_line = f"ctx={usage.used_tokens}/{usage.context_tokens} ({usage.percent:.1f}%)"
        self.update(
            f"rova {profile}/{rag}/{skills}  │  {ctx_line}  │  Type / for commands, /exit to quit"
        )
