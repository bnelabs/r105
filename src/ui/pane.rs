//! Panes: independently live sessions inside a tab.
//!
//! A pane owns everything that belongs to one running session — its
//! transcript, composer, model request lifecycle, queue, and completions.
//! `UiApp` derefs to the focused pane (`routing` first, for events
//! addressed at a background pane), so existing code keeps addressing
//! `self.state`, `self.input`, … without naming a pane.

use super::*;

pub(crate) struct Pane {
    /// Stable identity for event routing; survives reordering and closes.
    pub(crate) id: u64,
    /// Label in the pane frame / tab stub (`session`, `session 2`, …).
    pub(crate) title: String,
    /// A background pane finished a run; cleared when it is focused.
    pub(crate) attention: bool,
    /// Last drawn frame rect, for click-to-focus.
    pub(crate) rect: Rect,
    /// Session transcript and model context.
    pub(crate) state: ChatState,
    /// Composer text and caret.
    pub(crate) input: String,
    pub(crate) cursor: usize,
    pub(crate) mode: Mode,
    /// Transcript viewport: saved offset, follow flag, expanded details.
    pub(crate) transcript_scroll: usize,
    pub(crate) follow_transcript: bool,
    pub(crate) show_details: bool,
    /// Request lifecycle.
    pub(crate) busy: bool,
    pub(crate) streaming: String,
    /// Reasoning deltas for the in-flight request. Kept apart from
    /// `streaming` so the reply stays clean; rendered collapsed.
    pub(crate) streaming_reasoning: String,
    pub(crate) status: String,
    pub(crate) status_tone: crate::ui::events::StatusTone,
    /// When the active request started, for the slow-start hint. Cold model
    /// loads look exactly like a hung request until the first token lands.
    pub(crate) request_started: Option<Instant>,
    pub(crate) awaiting_first_token: bool,
    pub(crate) slow_hint_shown: bool,
    pub(crate) queue: VecDeque<(String, Option<String>)>,
    /// In-flight compaction backup name: doubles as the "a compaction
    /// (not a chat) failed" flag so `ChatError` reports `/compact to
    /// retry` instead of the chat `/retry` path. Cleared in both arms.
    pub(crate) compact_backup: Option<String>,
    /// Name of the session this transcript was saved as or loaded from
    /// (fork sources and checkpoint parents). Forks leave it alone: you
    /// keep working here, the copy points back at you.
    pub(crate) current_session: Option<String>,
    /// Next lazy transcript message number (`m<N>` IDs are assigned on
    /// first render so undo/redo/compact never shift section identity).
    pub(crate) next_msg_id: u64,
    /// Per-section expand overrides keyed by message ID; entries exist
    /// only where the user diverged from the global defaults.
    pub(crate) section_state: HashMap<String, bool>,
    /// Message IDs of the sections in gutter order, rebuilt every draw
    /// so `/expand n` resolves against what is currently visible. The
    /// bool is the section's global default (tool output follows
    /// `show_details`, thinking follows `thinking_default_expanded`).
    pub(crate) section_order: Vec<(String, bool)>,
    /// Per-block output filter keyed by message id (spec 0017). Entries exist
    /// only where the user has applied /filter. Pruned with sections on
    /// transcript load/save.
    pub(crate) block_filters: HashMap<String, BlockFilter>,
    /// File context resolved from `@refs`, pushed as a system message next
    /// to the user message once the backend answers.
    pub(crate) pending_context: Option<String>,
    /// Undone exchanges, newest last; any new user prompt clears the stack.
    pub(crate) redo_stack: Vec<crate::ui::events::UndoEntry>,
    /// Set by steering: the in-flight request is cancelled and its prompt
    /// must not be restored into the composer or offered via `/retry`.
    pub(crate) drop_next_restore: bool,
    pub(crate) active_user: Option<String>,
    pub(crate) last_failed_prompt: Option<String>,
    pub(crate) tool_round: usize,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) pending_connection: Option<Connection>,
    /// Tool round paused on approval cards; cleared by cancel.
    pub(crate) pending_tools: Option<PendingTools>,
    /// Shared with tool workers: `todo_write` replaces the list here,
    /// the results handler syncs it into session state.
    pub(crate) shared_todos: Arc<Mutex<Vec<crate::model::TodoItem>>>,
    /// Ghost text: visible suggestion, debounce bookkeeping, and the
    /// shell-history frequency store behind the cascade.
    pub(crate) ghost_text: Option<String>,
    pub(crate) ghost_seen_input: String,
    pub(crate) ghost_dismissed: Option<String>,
    pub(crate) ghost_changed_at: Instant,
    pub(crate) ghost_debounce: Duration,
    pub(crate) completion_on: bool,
    /// Model ghost behind the local cascade: idle-only, debounced,
    /// one flight per input generation. Toggle with `/completion`.
    pub(crate) ai_suggest_on: bool,
    pub(crate) ai_ghost_seq: u64,
    pub(crate) ai_ghost_pending: Option<String>,
    /// ↑/↓ history walk: `Some(depth)` while a past user message is
    /// previewed (1 = most recent), plus the stashed live draft the walk
    /// restored or replaced. Spec 0018.
    pub(crate) hist_depth: Option<usize>,
    pub(crate) draft_stash: String,
    /// A failed shell line's proposed fix, offered until the next edit.
    /// `→` applies it into an empty composer; any edit drops it.
    pub(crate) pending_correction: Option<crate::suggest::Correction>,
    pub(crate) last_response: String,
    /// Accumulated session token usage for the footer telemetry.
    pub(crate) session_in: u64,
    pub(crate) session_out: u64,
    /// Accumulated prefix-cache hits for the footer telemetry.
    pub(crate) session_cached: u64,
    /// `@file` completion state: selected index plus an input-keyed cache so
    /// the workspace walk only reruns when the composer text changes.
    pub(crate) at_selected: usize,
    pub(crate) at_cache_key: String,
    pub(crate) at_cache_items: Vec<String>,
    /// First-argument value completion (`/theme <Tab>`): same input-keyed
    /// cache discipline as the `@` menu.
    pub(crate) arg_selected: usize,
    pub(crate) arg_cache_key: String,
    pub(crate) arg_cache_items: Vec<String>,
    /// Shell-line Tab menu (`git check<Tab>`): unified history, spec,
    /// and file candidates with the same input-keyed cache discipline.
    pub(crate) sh_selected: usize,
    pub(crate) sh_cache_key: String,
    pub(crate) sh_cache_items: Vec<crate::suggest::ShellCandidate>,
    /// Tab opens the shell menu only when ambiguous (2+ rows); a
    /// single row applies directly. Sticky across edits until Esc,
    /// accept, or submit — the ghost stands down while it shows rows.
    pub(crate) sh_menu_invoked: bool,
    pub(crate) last_sh_rect: Option<Rect>,
    pub(crate) last_sh_count: usize,
    /// Last drawn viewport heights/rects for viewport-aware paging and
    /// mouse hit-testing. Updated in `draw`; read by input handlers.
    pub(crate) transcript_height: u16,
    pub(crate) last_transcript_rect: Rect,
    pub(crate) last_composer_rect: Rect,
    pub(crate) last_palette_rect: Option<Rect>,
    pub(crate) last_palette_count: usize,
    /// Per rendered transcript line, the collapsible section id when the
    /// line is a section header. Lets a mouse click toggle a section.
    pub(crate) transcript_header_rows: Vec<Option<String>>,
    /// Scroll offset at render time, so a click row maps back to a line.
    pub(crate) last_transcript_scroll: usize,
}

impl Pane {
    /// A fresh session in `state`'s workspace. Completion settings come
    /// from the pane this one was split off (or the config for the first).
    pub(crate) fn new(
        id: u64,
        state: ChatState,
        ghost_debounce: Duration,
        completion_on: bool,
        ai_suggest_on: bool,
    ) -> Self {
        Self {
            id,
            title: String::new(),
            attention: false,
            rect: Rect::default(),
            mode: match state.mode.as_str() {
                "plan" => Mode::Plan,
                "ask" => Mode::Ask,
                _ => Mode::Build,
            },
            state,
            input: String::new(),
            cursor: 0,
            transcript_scroll: 0,
            follow_transcript: true,
            show_details: false,
            busy: false,
            streaming: String::new(),
            streaming_reasoning: String::new(),
            status: String::new(),
            status_tone: crate::ui::events::StatusTone::Muted,
            request_started: None,
            awaiting_first_token: false,
            slow_hint_shown: false,
            queue: VecDeque::new(),
            compact_backup: None,
            current_session: None,
            next_msg_id: 0,
            section_state: HashMap::new(),
            section_order: Vec::new(),
            block_filters: HashMap::new(),
            pending_context: None,
            redo_stack: Vec::new(),
            drop_next_restore: false,
            active_user: None,
            last_failed_prompt: None,
            tool_round: 0,
            cancellation: None,
            pending_connection: None,
            pending_tools: None,
            shared_todos: Arc::new(Mutex::new(Vec::new())),
            ghost_text: None,
            ghost_seen_input: String::new(),
            ghost_dismissed: None,
            ghost_changed_at: Instant::now(),
            ghost_debounce,
            completion_on,
            ai_suggest_on,
            ai_ghost_seq: 0,
            ai_ghost_pending: None,
            hist_depth: None,
            draft_stash: String::new(),
            pending_correction: None,
            last_response: String::new(),
            session_in: 0,
            session_out: 0,
            session_cached: 0,
            at_selected: 0,
            at_cache_key: String::new(),
            at_cache_items: Vec::new(),
            arg_selected: 0,
            arg_cache_key: String::new(),
            arg_cache_items: Vec::new(),
            sh_selected: 0,
            sh_cache_key: String::new(),
            sh_cache_items: Vec::new(),
            sh_menu_invoked: false,
            last_sh_rect: None,
            last_sh_count: 0,
            transcript_height: 20,
            last_transcript_rect: Rect::default(),
            last_composer_rect: Rect::default(),
            last_palette_rect: None,
            last_palette_count: 0,
            transcript_header_rows: Vec::new(),
            last_transcript_scroll: 0,
        }
    }
}

/// Inherent forwarders: `UiApp` methods call through to the focused
/// pane. Inherent (rather than deref) resolution keeps two-phase
/// borrows alive at call sites like `self.set_status(format!("{}", self.busy))`.
impl UiApp {
    pub(crate) fn push_system(&mut self, content: &str) {
        Pane::push_system(&mut *self, content);
    }

    pub(crate) fn set_status(&mut self, text: String) {
        Pane::set_status(&mut *self, text);
    }

    pub(crate) fn set_ok(&mut self, text: String) {
        Pane::set_ok(&mut *self, text);
    }

    pub(crate) fn set_error(&mut self, text: String) {
        Pane::set_error(&mut *self, text);
    }

    pub(crate) fn set_mode(&mut self, mode: Mode) {
        Pane::set_mode(&mut *self, mode);
    }

    pub(crate) fn sync_mode_from_state(&mut self) {
        Pane::sync_mode_from_state(&mut *self);
    }

    pub(crate) fn section_id(&mut self, index: usize) -> String {
        Pane::section_id(&mut *self, index)
    }

    pub(crate) fn section_expanded(&self, id: &str, default: bool) -> bool {
        Pane::section_expanded(self, id, default)
    }

    pub(crate) fn prune_sections(&mut self) {
        Pane::prune_sections(&mut *self);
    }

    pub(crate) fn reseed_msg_ids(&mut self) {
        Pane::reseed_msg_ids(&mut *self);
    }

    pub(crate) fn command_expand(&mut self, args: &[String]) {
        Pane::command_expand(&mut *self, args);
    }

    pub(crate) fn toggle_section(&mut self, id: &str, default: bool, label: &str) -> Option<bool> {
        Pane::toggle_section(&mut *self, id, default, label)
    }
}

/// Most panes one tab shows side by side. Four keeps the composer
/// usable on a normal terminal width.
pub(crate) const MAX_PANES: usize = 4;

impl UiApp {
    /// Split right (Ctrl+Shift+D): a fresh session beside the
    /// focused one, which becomes focused. The running pane keeps
    /// streaming — that is the point of panes.
    pub(crate) fn pane_split(&mut self) {
        if self.panes.len() >= MAX_PANES {
            self.set_status(format!("{MAX_PANES} panes is the limit"));
            return;
        }
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        let state = self.state.fresh_like();
        let mut pane = Pane::new(
            id,
            state,
            self.ghost_debounce,
            self.completion_on,
            self.ai_suggest_on,
        );
        pane.title = format!("session {}", self.panes.len() + 1);
        pane.status = "Ready · new pane".into();
        let index = self.focus + 1;
        self.panes.insert(index, pane);
        self.focus = index;
        self.hist_search = None;
        self.sync_tab_layout();
        self.save_tabs();
        self.set_status(format!(
            "Split · pane {} of {} — Ctrl+Shift+←/→ focuses",
            self.focus + 1,
            self.panes.len()
        ));
    }

    /// Close the focused pane (Ctrl+Shift+W in a split). The last
    /// pane falls back to closing its tab, matching tab behavior.
    pub(crate) fn pane_close(&mut self) {
        if self.panes.len() <= 1 {
            self.tab_close();
            return;
        }
        if self.busy {
            self.set_status("Busy — finish or cancel first".into());
            return;
        }
        let paths = self.paths.clone();
        let mut closed = self.panes.remove(self.focus);
        closed.autosave(&paths);
        if self.focus >= self.panes.len() {
            self.focus = self.panes.len() - 1;
        }
        self.sync_tab_layout();
        self.save_tabs();
        self.set_status(format!(
            "Closed pane · {} of {} left",
            self.focus + 1,
            self.panes.len()
        ));
    }

    /// Focus a pane by index, clearing its attention marker.
    pub(crate) fn pane_focus(&mut self, index: usize) {
        if index >= self.panes.len() || index == self.focus {
            return;
        }
        self.focus = index;
        self.panes[index].attention = false;
        self.hist_search = None;
        self.refresh_git_branch();
        self.refresh_custom_commands();
        self.refresh_sidebar();
    }

    /// Move focus to the next/previous pane, wrapping.
    pub(crate) fn pane_next(&mut self, forward: bool) {
        let count = self.panes.len();
        if count <= 1 {
            return;
        }
        let index = if forward {
            (self.focus + 1) % count
        } else {
            (self.focus + count - 1) % count
        };
        self.pane_focus(index);
        self.set_status(format!("Pane {} of {}", self.focus + 1, count));
    }

    /// The pane whose last drawn frame contains the point.
    pub(crate) fn pane_at(&self, column: u16, row: u16) -> Option<usize> {
        self.panes.iter().position(|pane| {
            let area = pane.rect;
            area.width > 0
                && column >= area.x
                && column < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        })
    }
}

impl Pane {
    /// Persist this pane's transcript when it holds anything: back to
    /// its own name, else a timestamped autosave. Returns the name used.
    pub(crate) fn autosave(&mut self, paths: &ConfigPaths) -> Option<String> {
        if self.state.history.is_empty() {
            return None;
        }
        let name = self.current_session.clone().unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| format!("autosave-{}", value.as_secs()))
                .unwrap_or_else(|_| "autosave".to_string())
        });
        match session::save(paths, &name, &self.state) {
            Ok(_) => {
                self.current_session = Some(name.clone());
                Some(name)
            }
            Err(_) => None,
        }
    }
}

impl std::ops::Deref for UiApp {
    type Target = Pane;

    /// The focused pane, or the pane an event is currently being routed
    /// to. User focus lives in `focus`; `routing` only ever covers the
    /// synchronous span of one event handler.
    fn deref(&self) -> &Pane {
        &self.panes[self.routing.unwrap_or(self.focus)]
    }
}

impl std::ops::DerefMut for UiApp {
    fn deref_mut(&mut self) -> &mut Pane {
        let index = self.routing.unwrap_or(self.focus);
        &mut self.panes[index]
    }
}
