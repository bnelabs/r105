"""Guided provider connection screen for the TUI."""

from __future__ import annotations

import os
from typing import TYPE_CHECKING, Any

import httpx
from textual import work
from textual.app import ComposeResult
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Select, Static

from r105.client import Client, create_client
from r105.config import ensure_config
from r105.connections import (
    CONNECTION_PRESETS,
    ConnectionPreset,
    get_connection_preset,
    provider_options,
    resolve_api_key,
    valid_connection_url,
)
from r105.tui.widgets.chat_view import ChatView

if TYPE_CHECKING:
    from r105.tui.screens.chat import ChatScreen


def extract_model_ids(payload: dict[str, Any]) -> list[str]:
    """Normalize common OpenAI-compatible model-list response shapes."""
    raw_models = payload.get("data") or payload.get("models") or []
    if isinstance(raw_models, dict):
        raw_models = [
            {"id": model_id, **(entry if isinstance(entry, dict) else {})}
            for model_id, entry in raw_models.items()
        ]

    model_ids: list[str] = []
    seen: set[str] = set()
    for entry in raw_models:
        if isinstance(entry, str):
            model_id = entry.strip()
        elif isinstance(entry, dict):
            model_id = str(entry.get("id") or entry.get("name") or entry.get("model") or "").strip()
        else:
            model_id = ""
        if model_id and model_id not in seen:
            model_ids.append(model_id)
            seen.add(model_id)
    return model_ids


class ConnectionScreen(ModalScreen[None]):
    """Select a provider, authenticate it, and choose a live model."""

    BINDINGS = [
        ("escape", "dismiss", "Cancel"),
    ]

    def __init__(self, chat_screen: ChatScreen) -> None:
        super().__init__()
        self.chat_screen = chat_screen
        self._selected: ConnectionPreset | None = None
        self._candidate: Client | None = None
        self._candidate_key: str | None = None
        self._candidate_url: str | None = None
        self._models: list[str] = []
        self._phase = "select"
        self._busy = False
        self._last_provider_id: str | None = None
        self._current_provider_id = self._find_current_provider()

    def compose(self) -> ComposeResult:
        with Vertical(id="connection-dialog"):
            yield Static("[bold]Connect to an AI provider[/bold]", id="connection-title")
            yield Static(
                "Choose a preset. r105 will check the connection, load its model list, "
                "and then let you choose the model.",
                id="connection-intro",
            )
            yield Select(
                provider_options(),
                prompt="Choose a provider",
                value=self._current_provider_id or Select.NULL,
                id="connection-provider",
            )
            yield Static("", id="connection-description")
            yield Static("API key", id="connection-key-label", classes="connection-hidden")
            yield Input(
                placeholder="Paste a key; it is used for this session only",
                password=True,
                id="connection-api-key",
                classes="connection-hidden",
            )
            yield Static("Base URL", id="connection-url-label", classes="connection-hidden")
            yield Input(
                placeholder="https://host.example/v1",
                id="connection-url",
                classes="connection-hidden",
            )
            yield Static("", id="connection-status")
            yield Static("Models", id="connection-model-label", classes="connection-hidden")
            yield Select(
                [],
                prompt="Choose a model",
                id="connection-model",
                classes="connection-hidden",
            )
            with Horizontal(id="connection-buttons"):
                yield Button(
                    "Load models",
                    variant="primary",
                    id="connection-load-models",
                    disabled=self._current_provider_id is None,
                )
                yield Button(
                    "Apply connection",
                    variant="success",
                    id="connection-apply",
                    classes="connection-hidden",
                )
                yield Button("Back", id="connection-back", classes="connection-hidden")
                yield Button("Cancel", id="connection-cancel")

    def on_mount(self) -> None:
        self._update_provider_fields()
        self.query_one("#connection-provider", Select).focus()

    def _find_current_provider(self) -> str | None:
        config = ensure_config()
        configured = get_connection_preset(config.get("provider"))
        if configured is not None:
            return configured.id
        configured_url = config.get("url")
        if configured_url:
            for preset in CONNECTION_PRESETS:
                if preset.base_url == configured_url:
                    return preset.id
            return "custom"
        return None

    def _set_visible(self, selector: str, visible: bool) -> None:
        widget = self.query_one(selector)
        widget.styles.display = "block" if visible else "none"

    def _set_status(self, message: str) -> None:
        self.query_one("#connection-status", Static).update(message)

    def _update_provider_fields(self) -> None:
        select = self.query_one("#connection-provider", Select)
        provider_id = select.value if isinstance(select.value, str) else None
        self._selected = get_connection_preset(provider_id)
        self._phase = "select"
        self._candidate = None
        self._models = []
        self._set_visible("#connection-model-label", False)
        self._set_visible("#connection-model", False)
        self._set_visible("#connection-apply", False)
        self._set_visible("#connection-back", False)
        self._set_visible("#connection-load-models", True)

        if self._selected is None:
            self._last_provider_id = None
            self._set_visible("#connection-key-label", False)
            self._set_visible("#connection-api-key", False)
            self._set_visible("#connection-url-label", False)
            self._set_visible("#connection-url", False)
            self.query_one("#connection-description", Static).update(
                "[dim]Select a provider to begin.[/dim]"
            )
            self.query_one("#connection-load-models", Button).disabled = True
            return

        preset = self._selected
        if self._last_provider_id is not None and self._last_provider_id != preset.id:
            self.query_one("#connection-api-key", Input).value = ""
        self._last_provider_id = preset.id
        description = f"[bold]{preset.label}[/bold]\n{preset.description}"
        if preset.base_url:
            description += f"\n[dim]endpoint: {preset.base_url}[/dim]"
        if preset.docs_url:
            description += f"\n[dim]key page: {preset.docs_url}[/dim]"
        self.query_one("#connection-description", Static).update(description)

        is_custom = preset.id == "custom"
        self._set_visible("#connection-url-label", is_custom)
        self._set_visible("#connection-url", is_custom)
        if is_custom:
            configured_url = ensure_config().get("url")
            if configured_url and not self.query_one("#connection-url", Input).value:
                self.query_one("#connection-url", Input).value = str(configured_url)

        show_key = preset.needs_api_key_field
        self._set_visible("#connection-key-label", show_key)
        self._set_visible("#connection-api-key", show_key)
        key_input = self.query_one("#connection-api-key", Input)
        if preset.api_key_env:
            env_status = "set" if os.environ.get(preset.api_key_env) else "not set"
            key_input.placeholder = (
                f"Optional: leave blank to use {preset.api_key_env} ({env_status})"
            )
        else:
            key_input.placeholder = "Optional API key; leave blank for local servers"
        self.query_one("#connection-load-models", Button).disabled = False
        self._set_status("[dim]Credentials stay in memory and are not saved to config.json.[/dim]")

    def on_select_changed(self, event: Select.Changed) -> None:
        if event.select.id == "connection-provider":
            self._update_provider_fields()

    def _base_url(self) -> str | None:
        if self._selected is None:
            return None
        if self._selected.id == "custom":
            return self.query_one("#connection-url", Input).value.strip()
        return self._selected.base_url

    def _resolve_key(self) -> str | None:
        if self._selected is None:
            return None
        entered = self.query_one("#connection-api-key", Input).value
        current_key = getattr(self.chat_screen, "_connection_api_key", None)
        fallback = current_key if self._selected.id == getattr(
            self.chat_screen, "_connection_provider", None
        ) else None
        return resolve_api_key(self._selected, entered, fallback=fallback)

    @work(exclusive=True)
    async def _load_models(self) -> None:
        if self._busy or self._selected is None:
            return
        self._busy = True
        preset = self._selected
        base_url = self._base_url()
        if not base_url or not valid_connection_url(base_url):
            self._set_status(
                "[red]Enter a valid HTTP(S) base URL without credentials.[/red]"
            )
            self._busy = False
            return

        api_key = self._resolve_key()
        if preset.api_key_required and not api_key:
            self._set_status(
                f"[red]Paste an API key or set {preset.api_key_env} before connecting.[/red]"
            )
            self.query_one("#connection-api-key", Input).focus()
            self._busy = False
            return

        self._set_status(f"[yellow]Connecting to {preset.label} and loading models…[/yellow]")
        self.query_one("#connection-load-models", Button).disabled = True
        self.query_one("#connection-provider", Select).disabled = True
        try:
            candidate = create_client(
                base_url=base_url,
                backend=preset.backend,
                api_key=api_key,
            )
            payload = await candidate.async_list_models(
                client=getattr(self.chat_screen, "_http", None)
            )
            model_ids = extract_model_ids(payload)
        except (httpx.HTTPError, OSError, TimeoutError, ValueError) as exc:
            detail = str(exc).strip() or type(exc).__name__
            self._set_status(f"[red]Could not load models: {detail}[/red]")
            self._enable_provider_controls()
            self._busy = False
            return

        if not model_ids:
            self._set_status(
                "[red]The provider connected but returned no models. "
                "Check the endpoint or credentials.[/red]"
            )
            self._enable_provider_controls()
            self._busy = False
            return

        self._candidate = candidate
        self._candidate_key = api_key
        self._candidate_url = base_url
        self._models = model_ids
        model_select = self.query_one("#connection-model", Select)
        model_select.set_options([(model_id, model_id) for model_id in model_ids])
        current_model = getattr(self.chat_screen.state, "model", "")
        model_select.value = current_model if current_model in model_ids else model_ids[0]
        self._phase = "models"
        self._set_visible("#connection-model-label", True)
        self._set_visible("#connection-model", True)
        self._set_visible("#connection-load-models", False)
        self._set_visible("#connection-apply", True)
        self._set_visible("#connection-back", True)
        self.query_one("#connection-apply", Button).disabled = False
        self._set_status(
            f"[green]Loaded {len(model_ids)} model(s). Choose one and apply the connection.[/green]"
        )
        self._busy = False

    def _enable_provider_controls(self) -> None:
        self.query_one("#connection-load-models", Button).disabled = False
        self.query_one("#connection-provider", Select).disabled = False

    def _reset_model_step(self) -> None:
        self._phase = "select"
        self._candidate = None
        self._models = []
        self._enable_provider_controls()
        self._set_visible("#connection-model-label", False)
        self._set_visible("#connection-model", False)
        self._set_visible("#connection-load-models", True)
        self._set_visible("#connection-apply", False)
        self._set_visible("#connection-back", False)
        self._set_status("[dim]Choose a provider and load its models.[/dim]")

    def _apply_connection(self) -> None:
        if self._selected is None or self._candidate is None or not self._candidate_url:
            return
        model_value = self.query_one("#connection-model", Select).value
        if not isinstance(model_value, str) or not model_value:
            self._set_status("[red]Choose a model before applying the connection.[/red]")
            return
        try:
            message = self.chat_screen.apply_connection(
                self._selected,
                self._candidate_url,
                self._candidate_key,
                model_value,
                self._candidate,
            )
        except (OSError, ValueError) as exc:
            self._set_status(f"[red]Could not save connection: {exc}[/red]")
            return
        self.app.pop_screen()
        self.chat_screen.query_one("#chat-view", ChatView).add_system(message)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        button_id = event.button.id
        if button_id == "connection-load-models":
            self._load_models()
        elif button_id == "connection-apply":
            self._apply_connection()
        elif button_id == "connection-back":
            self._reset_model_step()
        elif button_id == "connection-cancel":
            self.app.pop_screen()

    def action_dismiss(self, result: None = None) -> None:  # type: ignore[override]
        self.app.pop_screen()


__all__ = ["ConnectionScreen", "extract_model_ids"]
