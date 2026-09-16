//! UI event types and related enums.

use crate::backend::{Backend, BackendEvent};
use crate::model::{ChatResult, Message};

/// One entry of a provider model list. `status` carries the backend's load
/// state (`loaded`, `unloaded`, …) when the provider reports one; most
/// OpenAI-compatible endpoints omit it entirely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub status: Option<String>,
    /// Context window the provider publishes for this model, when known.
    pub context: Option<u64>,
}

impl ModelInfo {
    pub fn display(&self, active: &str) -> String {
        let marker = if self.id == active { "●" } else { " " };
        let mut text = if self.id == active {
            format!("{marker} {} (active)", self.id)
        } else {
            format!("{marker} {}", self.id)
        };
        if let Some(context) = self.context {
            text.push_str(&format!(" · ctx {}", compact_context(context)));
        }
        if let Some(status) = &self.status {
            text.push_str(&format!(" · {status}"));
        }
        text
    }
}

/// Context windows are round binary numbers, so spell them the way
/// people say them: `128k`, `256k`, `1M`.
fn compact_context(tokens: u64) -> String {
    if tokens >= 1024 * 1024 && tokens.is_multiple_of(1024 * 1024) {
        format!("{}M", tokens / (1024 * 1024))
    } else if tokens >= 1024 && tokens.is_multiple_of(1024) {
        format!("{}k", tokens / 1024)
    } else {
        tokens.to_string()
    }
}

/// A `UiEvent` addressed to one live pane, or to the window at large.
/// Panes are addressed by stable id, so events that arrive after a pane
/// closed are dropped instead of landing in a reused slot.
#[derive(Debug)]
pub struct Routed {
    pub pane: Option<u64>,
    pub event: UiEvent,
}

impl UiEvent {
    /// Address this event to the pane with the given id.
    pub fn at(self, pane: u64) -> Routed {
        Routed {
            pane: Some(pane),
            event: self,
        }
    }

    /// Address this event to the window (focused pane while handled).
    pub fn global(self) -> Routed {
        Routed {
            pane: None,
            event: self,
        }
    }
}

#[derive(Debug)]
pub enum UiEvent {
    Backend(BackendEvent),
    ChatDone(ChatResult),
    ChatError(String),
    ToolsDone(Vec<crate::tool::ToolResult>),
    Compacted {
        summary: String,
        recent: Vec<Message>,
    },
    /// A `#` draft round-trip finished: fill the composer with the
    /// proposed command (`Ok`) or report why drafting failed (`Err`).
    ShellDraft(Result<String, String>),
    /// A `!` shell line failed and local rules proposed fixes: the best
    /// plus up to two alternates. The UI offers the best without
    /// clobbering the composer and lists the rest in the transcript.
    ShellCorrection {
        failed: String,
        fixed: String,
        more: Vec<String>,
    },
    /// A background daemon query finished: store the values (or record
    /// the failure for backoff) without touching the composer.
    LiveValues {
        key: String,
        values: Vec<String>,
        ok: bool,
    },
    /// A model ghost round-trip finished: `suffix` extends the typed
    /// shell text (`None` when the model failed, repeated the input, or
    /// answered prose). Applied only when the composer still matches.
    AiGhost {
        seq: u64,
        for_input: String,
        suffix: Option<String>,
    },
    ModelsLoaded {
        backend: Backend,
        models: Vec<ModelInfo>,
        /// Loaded window reported by the server itself, when it does.
        runtime_context: Option<u64>,
    },
    /// Quiet context-window refresh, sent by the startup probe so the
    /// footer and auto-compaction are right before the first `/models`.
    /// Unlike `ModelsLoaded` it never opens an overlay or changes status.
    ContextObserved {
        runtime_context: Option<u64>,
        contexts: std::collections::BTreeMap<String, u64>,
    },
    Notice(String),
}

#[derive(Debug)]
pub enum Overlay {
    None,
    Providers {
        selected: usize,
        scroll: usize,
    },
    Models {
        items: Vec<ModelInfo>,
        selected: usize,
        scroll: usize,
        active: String,
    },
    ApiKey {
        provider: String,
    },
    CustomUrl {
        provider: String,
    },
    Theme {
        selected: usize,
        original: String,
    },
    Settings {
        selected: usize,
    },
    /// A tool call awaits y/a/n; the queue lives in `pending_tools`.
    Approval,
}

/// One undoable exchange: everything from a user message onward. The prompt
/// itself is `messages[0]`, so `/undo` can restore it into the composer.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub messages: Vec<Message>,
}

/// Severity of the footer status line. The line is a single superseding
/// slot (a new note always replaces the old one); the tone only colors
/// it so failures stop looking like idle notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusTone {
    #[default]
    Muted,
    Success,
    Error,
}

impl StatusTone {
    pub fn color(self) -> ratatui::style::Color {
        match self {
            StatusTone::Muted => ratatui::style::Color::White,
            StatusTone::Success => ratatui::style::Color::Green,
            StatusTone::Error => ratatui::style::Color::Red,
        }
    }
}
