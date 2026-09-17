//! Native windowed terminal prototype (spec 0022, AI merge 0023).
//!
//! A winit OS window with wgpu rendering and glyphon text hosts the
//! existing [`crate::terminal::PtySession`]. `Ctrl+J` opens the AI
//! composer, `Ctrl+K` the AI panel; tool approvals arrive inline.
//! Layers are composited back-to-front with explicit bounds and logical sizing.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, Viewport, Wrap,
};
use wgpu::{MultisampleState, SurfaceConfiguration};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, Ime, KeyEvent, Modifiers, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use crate::{
    assistant::{
        AiHistory, ApprovalVerdict, AssistantEvent, AssistantHandle, AssistantParts, ComposerState,
        spawn_assistant,
    },
    backend::Backend,
    model::ChatState,
    terminal::PtySession,
};

const FONT_SIZE: f32 = 15.0;
const LINE_HEIGHT: f32 = 20.0;
/// Logical cell metrics used to size the PTY grid from the window.
const CELL_W: f32 = 9.0;
const CELL_H: f32 = 20.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 8.0;

const BG: wgpu::Color = wgpu::Color {
    r: 0.003346535,
    g: 0.004391442,
    b: 0.006995410,
    a: 1.0,
};
const FG: glyphon::Color = glyphon::Color::rgb(0xC9, 0xD4, 0xE3);
const DIM: glyphon::Color = glyphon::Color::rgb(0x8A, 0x94, 0xA6);
const ACCENT: glyphon::Color = glyphon::Color::rgb(0x6E, 0xD3, 0xFF);

/// Logical-pixel status bar; the PTY grid shrinks by this height.
const STATUS_PX: f32 = 30.0;
const CHROME_MARGIN: f32 = 24.0;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const SURFACE_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const SMOKE_MAX_DURATION: Duration = Duration::from_secs(10);

fn px(color: (u8, u8, u8)) -> [f32; 4] {
    let linear = |value: u8| {
        let srgb = f32::from(value) / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    };
    [linear(color.0), linear(color.1), linear(color.2), 1.0]
}

pub struct WindowOptions {
    pub workspace: PathBuf,
    /// Exit after this many frames (smoke test / agents).
    pub smoke_frames: Option<u64>,
    /// Seed composer, AI panel, and approval bar, then render: exercises
    /// every chrome path headless under `--smoke`.
    pub smoke_chrome: bool,
    pub smoke_snapshot: Option<PathBuf>,
    pub backend: Backend,
    pub state: ChatState,
    pub parts: AssistantParts,
}

/// Keyboard focus: terminal owns keystrokes unless a surface takes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Focus {
    #[default]
    Terminal,
    Composer,
    Approval,
    AiPanel,
}

impl Focus {
    fn label(self) -> &'static str {
        match self {
            Focus::Terminal => "terminal",
            Focus::Composer => "composer",
            Focus::Approval => "approval",
            Focus::AiPanel => "ai panel",
        }
    }
}

struct WindowApproval {
    id: String,
    name: String,
    summary: String,
    preview: Option<String>,
    index: usize,
    total: usize,
}

mod input;
mod rect;
use input::{clipboard_action, pointer_cell, selection_rects};
#[cfg(test)]
mod tests_gpu;
use rect::{BoxRect, ColoredRect, RectRenderer};

/// Caret position inside a shaped composer buffer, in buffer pixels.
/// Byte-exact for LTR text; BiDi falls back to the line end.
fn caret_xy(buffer: &Buffer, cursor_chars: usize, text: &str) -> (f32, f32) {
    let cursor_byte = text
        .char_indices()
        .nth(cursor_chars)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len());
    let line = text[..cursor_byte].matches('\n').count();
    let line_start = text[..cursor_byte]
        .rfind('\n')
        .map(|at| at + 1)
        .unwrap_or(0);
    let rel_byte = cursor_byte - line_start;
    let mut runs = buffer.layout_runs().peekable();
    while let Some(run) = runs.next() {
        if run.line_i != line {
            continue;
        }
        if let Some(last) = run.glyphs.last()
            && rel_byte >= last.end
            && runs.peek().is_some_and(|next| next.line_i == line)
        {
            continue;
        }
        let mut x = 0.0;
        for glyph in run.glyphs {
            if rel_byte == glyph.start {
                return (
                    if glyph.level.is_rtl() {
                        glyph.x + glyph.w
                    } else {
                        glyph.x
                    },
                    run.line_top,
                );
            }
            if glyph.end <= rel_byte {
                x = if glyph.level.is_rtl() {
                    glyph.x
                } else {
                    glyph.x + glyph.w
                };
            } else {
                break;
            }
        }
        return (x, run.line_top);
    }
    (0.0, line as f32 * LINE_HEIGHT)
}

#[derive(Debug, Default)]
pub struct WindowReport {
    pub frames: u64,
    pub screen_bytes: usize,
    pub blocks: usize,
}

/// Open the native window and run until close (or smoke budget).
/// Must run on the main thread (winit) inside a Tokio runtime
/// (the assistant spawns tasks); `main` calls it directly.
pub fn run_window(options: WindowOptions) -> Result<WindowReport> {
    let event_loop = EventLoop::new().context("creating window event loop")?;
    let mut app = WindowApp {
        options,
        state: None,
        report: WindowReport::default(),
        error: None,
    };
    event_loop
        .run_app(&mut app)
        .context("running window event loop")?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(app.report)
}

struct WindowApp {
    options: WindowOptions,
    state: Option<WindowState>,
    report: WindowReport,
    error: Option<anyhow::Error>,
}

struct WindowState {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: SurfaceConfiguration,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    renderers: Vec<glyphon::TextRenderer>,
    rects: RectRenderer,
    buffer: Buffer,
    status_buf: Buffer,
    composer_buf: Buffer,
    list_buf: Buffer,
    detail_buf: Buffer,
    approval_buf: Buffer,
    session: PtySession,
    modifiers: Modifiers,
    input_line: String,
    last_text: String,
    last_screen: Vec<u8>,
    terminal_rects: Vec<ColoredRect>,
    last_status: String,
    last_composer: String,
    last_list: String,
    last_detail: String,
    last_approval: String,
    frames: u64,
    focus: Focus,
    composer: ComposerState,
    ai: AiHistory,
    ai_active: Option<u64>,
    ai_busy: bool,
    approval: Option<WindowApproval>,
    panel_open: bool,
    panel_sel: usize,
    panel_scroll: usize,
    assistant: AssistantHandle,
    status_note: String,
    snapshot_path: Option<PathBuf>,
    clipboard: Option<arboard::Clipboard>,
    pointer: (f32, f32),
    selection: Option<((u16, u16), (u16, u16))>,
    selecting: bool,
    preedit: String,
    approval_scroll: usize,
    next_redraw: Instant,
    smoke_started: Instant,
    render_attempts: u64,
}

impl WindowApp {
    fn grid_for(size: winit::dpi::PhysicalSize<u32>, scale: f64) -> (u16, u16) {
        let logical_w = size.width as f64 / scale;
        // Reserve the status bar; chrome overlays float above the grid.
        let status_logical = f64::from(STATUS_PX);
        let logical_h = size.height as f64 / scale - status_logical;
        let cols = ((logical_w - 2.0 * PAD_X as f64) / CELL_W as f64)
            .floor()
            .clamp(1.0, 1000.0) as u16;
        let rows = ((logical_h - 2.0 * PAD_Y as f64) / CELL_H as f64)
            .floor()
            .clamp(1.0, 1000.0) as u16;
        (rows.max(1), cols.max(1))
    }
}

impl ApplicationHandler for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let outcome = (|| -> Result<WindowState> {
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("r105")
                            .with_min_inner_size(winit::dpi::LogicalSize::new(480.0, 320.0))
                            .with_inner_size(winit::dpi::LogicalSize::new(900.0, 600.0)),
                    )
                    .context("creating window")?,
            );
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            window.set_ime_allowed(true);
            let surface = instance
                .create_surface(window.clone())
                .context("creating wgpu surface")?;
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    compatible_surface: Some(&surface),
                    ..Default::default()
                }))
                .context("requesting GPU adapter")?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                    .context("requesting GPU device")?;
            let size = window.inner_size();
            let caps = surface.get_capabilities(&adapter);
            let format = caps
                .formats
                .iter()
                .find(|format| format.is_srgb())
                .copied()
                .unwrap_or(caps.formats[0]);
            let config = SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | if self.options.smoke_snapshot.is_some() {
                        wgpu::TextureUsages::COPY_SRC
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width: size.width.max(1),
                height: size.height.max(1),
                present_mode: wgpu::PresentMode::AutoVsync,
                alpha_mode: caps.alpha_modes[0],
                view_formats: Vec::new(),
                desired_maximum_frame_latency: 2,
            };
            surface.configure(&device, &config);

            let mut font_system = FontSystem::new();
            let swash = SwashCache::new();
            let cache = Cache::new(&device);
            let mut viewport = Viewport::new(&device, &cache);
            viewport.update(
                &queue,
                Resolution {
                    width: config.width,
                    height: config.height,
                },
            );
            let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
            let renderers = (0..5)
                .map(|_| {
                    glyphon::TextRenderer::new(
                        &mut atlas,
                        &device,
                        MultisampleState::default(),
                        None,
                    )
                })
                .collect();
            let rects = RectRenderer::new(&device, format);
            let metrics = Metrics::new(FONT_SIZE, LINE_HEIGHT);
            let mut buffer = Buffer::new(&mut font_system, metrics);
            // The vt100 screen already wraps lines; the text buffer must not.
            buffer.set_wrap(Wrap::None);
            buffer.set_size(Some(config.width as f32), Some(config.height as f32));
            let mut status_buf = Buffer::new(&mut font_system, metrics);
            status_buf.set_wrap(Wrap::None);
            let mut composer_buf = Buffer::new(&mut font_system, metrics);
            composer_buf.set_wrap(Wrap::Word);
            let mut list_buf = Buffer::new(&mut font_system, metrics);
            list_buf.set_wrap(Wrap::None);
            let mut detail_buf = Buffer::new(&mut font_system, metrics);
            detail_buf.set_wrap(Wrap::Word);
            let mut approval_buf = Buffer::new(&mut font_system, metrics);
            approval_buf.set_wrap(Wrap::Word);

            let scale = window.scale_factor();
            let (rows, cols) = Self::grid_for(size, scale);
            let session =
                PtySession::spawn(&self.options.workspace, rows, cols).context("spawning shell")?;
            // The assistant owns its ChatState for the window session;
            // history persists across prompts until the window closes.
            let assistant = spawn_assistant(
                self.options.backend.clone(),
                self.options.state.clone(),
                self.options.parts.clone(),
            );
            Ok(WindowState {
                window,
                surface,
                device,
                queue,
                config,
                font_system,
                swash,
                atlas,
                viewport,
                renderers,
                rects,
                buffer,
                status_buf,
                composer_buf,
                list_buf,
                detail_buf,
                approval_buf,
                session,
                modifiers: Modifiers::default(),
                input_line: String::new(),
                last_text: String::new(),
                last_screen: Vec::new(),
                terminal_rects: Vec::new(),
                last_status: String::new(),
                last_composer: String::new(),
                last_list: String::new(),
                last_detail: String::new(),
                last_approval: String::new(),
                frames: 0,
                focus: Focus::Terminal,
                composer: ComposerState::new(),
                ai: AiHistory::from_messages(&self.options.state.history),
                ai_active: None,
                ai_busy: false,
                approval: None,
                panel_open: false,
                panel_sel: 0,
                panel_scroll: 0,
                assistant,
                status_note: String::from("ready — Ctrl+J asks r105"),
                snapshot_path: None,
                clipboard: arboard::Clipboard::new().ok(),
                pointer: (0.0, 0.0),
                selection: None,
                selecting: false,
                preedit: String::new(),
                approval_scroll: 0,
                next_redraw: Instant::now(),
                smoke_started: Instant::now(),
                render_attempts: 0,
            })
        })();
        match outcome {
            Ok(mut state) => {
                if self.options.smoke_chrome {
                    // Headless coverage for every chrome path: composer
                    // with text and caret, a finished AI block, and a
                    // pending approval, all rendered under `--smoke`.
                    state.composer.insert_text("explain this workspace");
                    state.session.write(b"printf '\\033[32mRender acceptance: green text\\033[0m\\ncolumns: 0123456789 | unicode: caf\\303\\251\\n'\r").context("seeding smoke shell output").ok();
                    let seq = state.ai.begin("explain this workspace");
                    state.ai.push_token(seq, "A local-first AI harness.");
                    state.ai.finish(seq, 0, 1.2);
                    state.panel_sel = 0;
                    state.panel_open = true;
                    state.focus = Focus::AiPanel;
                }
                self.state = Some(state);
            }
            Err(error) => {
                self.error = Some(error.context("window initialization failed"));
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Focused(false) => {
                state.modifiers = Modifiers::default();
                state.selecting = false;
                state.preedit.clear();
            }
            WindowEvent::Ime(Ime::Preedit(text, _)) => {
                state.preedit = text;
                state.window.request_redraw();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                state.preedit.clear();
                if state.focus == Focus::Composer {
                    state.composer.insert_text(&text);
                } else if state.focus == Focus::Terminal {
                    state.session.scroll_to_bottom();
                    if let Err(error) = state.session.write(text.as_bytes()) {
                        state.status_note = error.to_string();
                    }
                }
                state.window.request_redraw();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                state.preedit.clear();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = state.window.scale_factor() as f32;
                state.pointer = (position.x as f32 / scale, position.y as f32 / scale);
                if state.selecting {
                    let end = pointer_cell(state);
                    if let Some((_, last)) = &mut state.selection {
                        *last = end;
                    }
                }
            }
            WindowEvent::MouseInput {
                state: button_state,
                button: MouseButton::Left,
                ..
            } => {
                state.selecting = false;
                if button_state == ElementState::Pressed && state.approval.is_none() {
                    let scale = state.window.scale_factor() as f32;
                    let layout = ChromeLayout::new(
                        state.config.width as f32 / scale,
                        state.config.height as f32 / scale,
                    );
                    let (x, y) = state.pointer;
                    if state.focus == Focus::Composer && layout.composer.contains(x, y) {
                        // Keep the draft focused. Keyboard navigation controls its caret.
                    } else if state.panel_open && layout.panel_sheet().contains(x, y) {
                        state.focus = Focus::AiPanel;
                    } else if layout.terminal.contains(x, y) {
                        state.focus = Focus::Terminal;
                        let cell = pointer_cell(state);
                        state.selection = Some((cell, cell));
                        state.selecting = true;
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => (y * 3.0) as i32,
                    MouseScrollDelta::PixelDelta(p) => {
                        (p.y / state.window.scale_factor() / f64::from(LINE_HEIGHT)).round() as i32
                    }
                };
                match state.focus {
                    Focus::AiPanel => {
                        state.panel_scroll =
                            state.panel_scroll.saturating_add_signed(-lines as isize)
                    }
                    Focus::Approval => {
                        state.approval_scroll =
                            state.approval_scroll.saturating_add_signed(-lines as isize)
                    }
                    Focus::Terminal => {
                        state.session.scroll(lines);
                        state.selection = None;
                    }
                    _ => {}
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                state.modifiers = modifiers;
            }
            WindowEvent::Resized(size) => {
                if size.width == 0 || size.height == 0 {
                    return;
                }
                state.config.width = size.width;
                state.config.height = size.height;
                state.surface.configure(&state.device, &state.config);
                state.viewport.update(
                    &state.queue,
                    Resolution {
                        width: size.width,
                        height: size.height,
                    },
                );
                for buffer in [
                    &mut state.buffer,
                    &mut state.status_buf,
                    &mut state.composer_buf,
                    &mut state.list_buf,
                    &mut state.detail_buf,
                    &mut state.approval_buf,
                ] {
                    buffer.set_size(Some(size.width as f32), Some(size.height as f32));
                }
                let (rows, cols) = Self::grid_for(size, state.window.scale_factor());
                let _ = state.session.resize(rows, cols);
                state.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = state.window.inner_size();
                let (rows, cols) = Self::grid_for(size, state.window.scale_factor());
                let _ = state.session.resize(rows, cols);
                state.window.request_redraw();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        logical_key,
                        text,
                        ..
                    },
                ..
            } => {
                let mods = state.modifiers.state();
                if (mods.super_key() || (mods.control_key() && mods.shift_key()))
                    && let Key::Character(c) = &logical_key
                    && (c.eq_ignore_ascii_case("c") || c.eq_ignore_ascii_case("v"))
                {
                    clipboard_action(state, c.eq_ignore_ascii_case("v"));
                    return;
                }
                if !state.preedit.is_empty() {
                    return;
                }
                if matches!(
                    map_key(&logical_key, text.as_deref(), &state.modifiers),
                    KeyAction::Quit
                ) {
                    event_loop.exit();
                    return;
                }
                if state.approval.is_some()
                    && (logical_key == Key::Named(NamedKey::Escape)
                        || (mods.control_key()
                            && matches!(&logical_key, Key::Character(c) if c.eq_ignore_ascii_case("c"))))
                {
                    state.assistant.cancel();
                    state.approval = None;
                    state.focus = Focus::Terminal;
                    state.status_note = "cancelling…".into();
                    return;
                }
                match state.focus {
                    Focus::Approval => {
                        match logical_key {
                            Key::Named(NamedKey::PageDown) => {
                                state.approval_scroll = state.approval_scroll.saturating_add(5)
                            }
                            Key::Named(NamedKey::PageUp) => {
                                state.approval_scroll = state.approval_scroll.saturating_sub(5)
                            }
                            Key::Named(NamedKey::Home) => state.approval_scroll = 0,
                            _ => {}
                        }
                        let verdict = match (&logical_key, text.as_deref()) {
                            (Key::Character(c), _) if c.as_str() == "y" || c.as_str() == "Y" => {
                                Some(ApprovalVerdict::Once)
                            }
                            (Key::Character(c), _) if c.as_str() == "a" || c.as_str() == "A" => {
                                Some(ApprovalVerdict::Always)
                            }
                            (Key::Character(c), _) if c.as_str() == "n" || c.as_str() == "N" => {
                                Some(ApprovalVerdict::Deny)
                            }
                            (Key::Named(NamedKey::Escape), _) => Some(ApprovalVerdict::Deny),
                            _ => None,
                        };
                        if let Some(verdict) = verdict {
                            if let Some(approval) = &state.approval {
                                state.assistant.verdict(approval.id.clone(), verdict);
                            }
                            state.approval = None;
                            state.focus = Focus::Terminal;
                        }
                    }
                    Focus::Composer => {
                        route_composer(state, &logical_key, text.as_deref(), mods);
                    }
                    Focus::AiPanel => {
                        route_panel(state, &logical_key, text.as_deref(), mods);
                    }
                    Focus::Terminal => {
                        // Window chrome first: composer, panel, quit.
                        if mods.control_key()
                            && !mods.alt_key()
                            && !mods.super_key()
                            && let Key::Character(c) = &logical_key
                        {
                            match c.as_str() {
                                "j" | "J" => {
                                    state.focus = Focus::Composer;
                                    state.window.request_redraw();
                                    return;
                                }
                                "k" | "K" => {
                                    state.panel_open = !state.panel_open;
                                    if state.panel_open {
                                        state.focus = Focus::AiPanel;
                                        clamp_panel(state);
                                    }
                                    state.window.request_redraw();
                                    return;
                                }
                                _ => {}
                            }
                        } // Esc stops a run when one is live; otherwise the
                        // shell owns it (vim and friends need it).
                        if logical_key == Key::Named(NamedKey::Escape) && state.ai_busy {
                            state.assistant.cancel();
                            state.status_note = "cancelled".to_string();
                            state.window.request_redraw();
                            return;
                        }
                        match map_key(&logical_key, text.as_deref(), &state.modifiers) {
                            KeyAction::Quit => event_loop.exit(),
                            KeyAction::Ignore => {}
                            KeyAction::Bytes(bytes) => {
                                state.session.scroll_to_bottom();
                                state.selection = None;
                                if bytes == b"\r" {
                                    let line = std::mem::take(&mut state.input_line);
                                    state.session.begin_block(&line);
                                } else if bytes == [0x7f] {
                                    state.input_line.pop();
                                } else if bytes.len() == 1
                                    && bytes[0] >= 0x20
                                    && let Some(text) = text
                                {
                                    state.input_line.push_str(&text);
                                }
                                if let Err(error) = state.session.write(&bytes) {
                                    state.status_note = format!("shell input failed: {error}");
                                }
                            }
                        }
                    }
                }
                state.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if self
                    .options
                    .smoke_frames
                    .is_some_and(|budget| state.frames >= budget)
                {
                    event_loop.exit();
                    return;
                }
                state.render_attempts = state.render_attempts.saturating_add(1);
                if let Some(budget) = self.options.smoke_frames {
                    let attempt_limit = budget.saturating_mul(4).max(60);
                    if state.render_attempts > attempt_limit
                        || state.smoke_started.elapsed() > SMOKE_MAX_DURATION
                    {
                        self.error = Some(anyhow::anyhow!(
                            "window smoke stalled: presented {} of {} frames after {} render attempts",
                            state.frames,
                            budget,
                            state.render_attempts,
                        ));
                        event_loop.exit();
                        return;
                    }
                }
                if self.options.smoke_chrome {
                    if state.frames == 30 {
                        state.focus = Focus::Composer;
                    }
                    if state.frames == 60 {
                        state.approval = Some(WindowApproval {
                            id: "smoke".into(),
                            name: "write_file".into(),
                            summary: "write_file notes/demo.txt".into(),
                            preview: Some("create notes/demo.txt (12 bytes)".into()),
                            index: 0,
                            total: 1,
                        });
                        state.focus = Focus::Approval;
                    }
                }
                if self
                    .options
                    .smoke_frames
                    .is_some_and(|budget| state.frames + 1 >= budget)
                {
                    state.snapshot_path = self.options.smoke_snapshot.clone();
                }
                match render_frame(state) {
                    // Timeout/Occluded surfaces skip the frame; only count
                    // presented frames toward the smoke budget.
                    Ok(presented) => {
                        state.next_redraw = Instant::now()
                            + if presented {
                                FRAME_INTERVAL
                            } else {
                                SURFACE_RETRY_INTERVAL
                            };
                        if presented {
                            state.frames += 1;
                            self.report.frames = state.frames;
                        }
                        self.report.screen_bytes = state.last_text.len();
                        self.report.blocks = state.session.blocks.len();
                        if let Some(budget) = self.options.smoke_frames
                            && state.frames >= budget
                        {
                            event_loop.exit();
                        }
                    }
                    Err(error) => {
                        self.error = Some(error.context("window rendering failed"));
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(state) = self.state.as_mut() {
            let now = Instant::now();
            if now >= state.next_redraw {
                state.window.request_redraw();
                state.next_redraw = now + FRAME_INTERVAL;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(state.next_redraw));
        }
    }
}

/// Composer key handling: readline-ish editing, `Enter` submits,
/// `Alt+Enter` inserts a newline, `Esc` closes, `Ctrl+C` cancels a run.
fn route_composer(
    state: &mut WindowState,
    logical_key: &Key,
    text: Option<&str>,
    mods: winit::keyboard::ModifiersState,
) {
    let ctrl = mods.control_key() && !mods.super_key() && !mods.alt_key();
    if ctrl {
        if let Key::Character(c) = logical_key {
            match c.as_str() {
                "c" | "C" => {
                    state.assistant.cancel();
                    state.status_note = "cancelled".to_string();
                    return;
                }
                "j" | "J" => {
                    state.focus = Focus::Terminal;
                    return;
                }
                "w" | "W" => {
                    state.composer.delete_word_back();
                    return;
                }
                "u" | "U" => {
                    state.composer.clear();
                    return;
                }
                "a" | "A" => {
                    state.composer.move_home();
                    return;
                }
                "e" | "E" => {
                    state.composer.move_end();
                    return;
                }
                _ => return,
            }
        }
        return;
    }
    match logical_key {
        Key::Named(NamedKey::Escape) => {
            state.focus = Focus::Terminal;
        }
        Key::Named(NamedKey::Enter) => {
            if mods.alt_key() {
                state.composer.insert_newline();
            } else {
                submit_composer(state);
            }
        }
        Key::Named(NamedKey::Backspace) => state.composer.backspace(),
        Key::Named(NamedKey::ArrowLeft) => state.composer.move_left(),
        Key::Named(NamedKey::ArrowRight) => state.composer.move_right(),
        Key::Named(NamedKey::Home) => state.composer.move_home(),
        Key::Named(NamedKey::End) => state.composer.move_end(),
        Key::Character(_) => {
            if !mods.super_key()
                && !mods.alt_key()
                && let Some(text) = text
            {
                state.composer.insert_text(text);
            }
        }
        _ => {}
    }
}

fn submit_composer(state: &mut WindowState) {
    // Events belong to the one active UI block. Preserve a second draft until
    // it can be submitted rather than attaching the first reply to that draft.
    if state.ai_busy {
        state.status_note = "AI is working — Ctrl+C cancels; draft preserved".into();
        return;
    }
    let prompt = state.composer.text().trim().to_string();
    state.composer.clear();
    state.focus = Focus::AiPanel;
    if prompt.is_empty() {
        return;
    }
    let seq = state.ai.begin(&prompt);
    state.ai_active = Some(seq);
    state.ai_busy = true;
    state.panel_sel = state.ai.len() - 1;
    state.panel_scroll = 0;
    state.panel_open = true;
    state.status_note = "asking…".to_string();
    state.assistant.ask(prompt);
}

/// AI panel navigation: arrows/`j`/`k` move, pages scroll detail.
fn route_panel(
    state: &mut WindowState,
    logical_key: &Key,
    text: Option<&str>,
    mods: winit::keyboard::ModifiersState,
) {
    if mods.control_key()
        && let Key::Character(c) = logical_key
    {
        if c.eq_ignore_ascii_case("j") {
            state.focus = Focus::Composer;
            return;
        }
        if c.eq_ignore_ascii_case("c") {
            state.assistant.cancel();
            return;
        }
    }
    if mods.control_key()
        && !mods.alt_key()
        && !mods.super_key()
        && let Key::Character(c) = logical_key
        && (c.as_str() == "k" || c.as_str() == "K")
    {
        state.panel_open = false;
        state.focus = Focus::Terminal;
        return;
    }
    let down = matches!(logical_key, Key::Named(NamedKey::ArrowDown)) || text == Some("j");
    let up = matches!(logical_key, Key::Named(NamedKey::ArrowUp)) || text == Some("k");
    match logical_key {
        Key::Named(NamedKey::Escape) => {
            state.panel_open = false;
            state.focus = Focus::Terminal;
        }
        Key::Named(NamedKey::PageDown) => {
            state.panel_scroll = state.panel_scroll.saturating_add(10);
        }
        Key::Named(NamedKey::PageUp) => {
            state.panel_scroll = state.panel_scroll.saturating_sub(10);
        }
        Key::Named(NamedKey::Home) => {
            state.panel_sel = 0;
            state.panel_scroll = 0;
        }
        Key::Named(NamedKey::End) => {
            state.panel_sel = state.ai.len().saturating_sub(1);
            state.panel_scroll = 0;
        }
        _ if down => {
            state.panel_sel = (state.panel_sel + 1).min(state.ai.len().saturating_sub(1));
            state.panel_scroll = 0;
        }
        _ if up => {
            state.panel_sel = state.panel_sel.saturating_sub(1);
            state.panel_scroll = 0;
        }
        _ => {}
    }
    clamp_panel(state);
}

fn clamp_panel(state: &mut WindowState) {
    if state.ai.is_empty() {
        state.panel_sel = 0;
    } else {
        state.panel_sel = state.panel_sel.min(state.ai.len() - 1);
    }
}

/// Drain assistant events into history, approvals, and status.
fn pump_assistant(state: &mut WindowState) {
    while let Ok(event) = state.assistant.events.try_recv() {
        match event {
            AssistantEvent::Token(text) => {
                if let Some(seq) = state.ai_active {
                    state.ai.push_token(seq, &text);
                }
            }
            AssistantEvent::Reasoning(text) => {
                if let Some(seq) = state.ai_active {
                    state.ai.push_reasoning(seq, &text);
                }
            }
            AssistantEvent::Status(note) => {
                state.status_note = note;
            }
            AssistantEvent::ToolStarted(names) => {
                state.status_note = format!("running tools: {}", names.join(", "));
            }
            AssistantEvent::ApprovalNeeded {
                id,
                index,
                total,
                name,
                summary,
                preview,
            } => {
                state.approval_scroll = 0;
                state.approval = Some(WindowApproval {
                    id,
                    name,
                    summary,
                    preview,
                    index,
                    total,
                });
                state.focus = Focus::Approval;
            }
            AssistantEvent::Done {
                response,
                tool_rounds,
                wall_seconds,
            } => {
                if let Some(seq) = state.ai_active.take() {
                    state.ai.set_response_if_empty(seq, &response);
                    state.ai.finish(seq, tool_rounds, wall_seconds);
                }
                state.ai_busy = false;
                state.approval = None;
                if state.focus == Focus::Approval {
                    state.focus = Focus::Terminal;
                }
                state.status_note =
                    format!("done in {wall_seconds:.1}s · {tool_rounds} tool rounds");
            }
            AssistantEvent::Error(error) => {
                if let Some(seq) = state.ai_active.take() {
                    state.ai.fail(seq, error.clone());
                }
                state.ai_busy = false;
                state.approval = None;
                if state.focus == Focus::Approval {
                    state.focus = Focus::Terminal;
                }
                state.status_note = format!("error: {error}");
            }
        }
    }
}

fn status_line(state: &WindowState) -> String {
    let ai_state = if state.approval.is_some() {
        "approval needed"
    } else if state.ai_busy {
        "thinking…"
    } else {
        "idle"
    };
    let (rows, cols) = state.session.size();
    format!(
        "r105 · {} · ai {ai_state} · {} · sh blocks {} · ai {} · {cols}x{rows}",
        state.focus.label(),
        state.status_note,
        state.session.blocks.len(),
        state.ai.len(),
    )
}

/// Shape against the actual layer width, including after a resize with unchanged text.
fn set_cached(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    last: &mut String,
    text: &str,
    width: f32,
) {
    buffer.set_size(Some(width.max(1.0)), None);
    if *last != text {
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        *last = text.to_string();
    }
    buffer.shape_until_scroll(font_system, false);
}

/// `bounds` fixes the glyph origin and wrap width; `clip` optionally
/// restricts the visible region so overlays can hide terminal rows.
fn text_area(
    buffer: &Buffer,
    bounds: BoxRect,
    scale: f32,
    color: glyphon::Color,
    scroll: f32,
    clip: Option<BoxRect>,
) -> TextArea<'_> {
    TextArea {
        buffer,
        left: bounds.x * scale,
        top: (bounds.y - scroll) * scale,
        scale,
        bounds: clip.unwrap_or(bounds).bounds(scale),
        default_color: color,
        custom_glyphs: &[],
    }
}

/// First terminal row boundary at or below `y`, so overlay backgrounds
/// start on whole rows and never bisect a glyph.
fn row_floor(y: f32) -> f32 {
    PAD_Y + (((y - PAD_Y) / CELL_H).floor() * CELL_H).max(0.0)
}

/// Shared geometry keeps background, clipping, layout width and hit regions aligned.
#[derive(Debug)]
struct ChromeLayout {
    size: (f32, f32),
    terminal: BoxRect,
    status: BoxRect,
    list: BoxRect,
    detail: BoxRect,
    composer: BoxRect,
    approval: BoxRect,
}

impl ChromeLayout {
    fn new(width: f32, height: f32) -> Self {
        let status = BoxRect::new(
            0.0,
            (height - STATUS_PX).max(0.0),
            width,
            STATUS_PX.min(height),
        );
        let margin = CHROME_MARGIN.min(width * 0.04).min(height * 0.04);
        let panel = BoxRect::new(
            margin,
            margin,
            width - 2.0 * margin,
            status.y - 2.0 * margin,
        );
        let list_w = (panel.w * 0.3).clamp(0.0, 260.0);
        let list = BoxRect::new(panel.x, panel.y, list_w, panel.h).inset(12.0);
        let detail = BoxRect::new(panel.x + list_w, panel.y, panel.w - list_w, panel.h).inset(12.0);
        let composer_h = (height * 0.42).clamp(0.0, 240.0).min(status.y);
        let composer = BoxRect::new(
            margin,
            (status.y - composer_h - 8.0).max(0.0),
            width - margin * 2.0,
            composer_h,
        );
        // Keep the decision keys on a dedicated line; preview scrolls above them.
        let approval_h = (height * 0.5).clamp(0.0, 280.0).min(status.y);
        let approval = BoxRect::new(
            margin,
            (status.y - approval_h - 8.0).max(0.0),
            width - margin * 2.0,
            approval_h,
        );
        Self {
            size: (width, height),
            terminal: BoxRect::new(PAD_X, PAD_Y, width - 2.0 * PAD_X, status.y - 2.0 * PAD_Y),
            status,
            list,
            detail,
            composer,
            approval,
        }
    }

    /// Full-bleed sheet behind the open panel. Terminal text runs to the
    /// window edge, so an inset background would leave glyph slivers in
    /// the margin; hit-testing shares this rectangle too.
    fn panel_sheet(&self) -> BoxRect {
        BoxRect::new(0.0, 0.0, self.size.0, self.status.y)
    }
}

fn terminal_color(color: vt100::Color, fallback: glyphon::Color) -> glyphon::Color {
    match color {
        vt100::Color::Default => fallback,
        vt100::Color::Rgb(r, g, b) => glyphon::Color::rgb(r, g, b),
        vt100::Color::Idx(index) => {
            const BASE: [[u8; 3]; 16] = [
                [0, 0, 0],
                [205, 49, 49],
                [13, 188, 121],
                [229, 229, 16],
                [36, 114, 200],
                [188, 63, 188],
                [17, 168, 205],
                [229, 229, 229],
                [102, 102, 102],
                [241, 76, 76],
                [35, 209, 139],
                [245, 245, 67],
                [59, 142, 234],
                [214, 112, 214],
                [41, 184, 219],
                [255, 255, 255],
            ];
            let [r, g, b] = if index < 16 {
                BASE[index as usize]
            } else if index < 232 {
                let n = index - 16;
                let channel = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                [channel(n / 36), channel((n / 6) % 6), channel(n % 6)]
            } else {
                [8 + (index - 232) * 10; 3]
            };
            glyphon::Color::rgb(r, g, b)
        }
    }
}

fn color_rect(color: glyphon::Color) -> [f32; 4] {
    px((color.r(), color.g(), color.b()))
}

/// Read cells rather than contents(): soft-wrapped rows must remain separate,
/// and color/background/wide-cell state must survive the PTY-to-GPU boundary.
fn terminal_spans(screen: &vt100::Screen) -> (Vec<(String, Attrs<'static>)>, Vec<ColoredRect>) {
    let (rows, cols) = screen.size();
    let mut spans = Vec::new();
    let mut rects = Vec::new();
    let default_bg = glyphon::Color::rgb(11, 14, 20);
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let mut fg = terminal_color(cell.fgcolor(), FG);
            let mut bg = terminal_color(cell.bgcolor(), default_bg);
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let width = if cell.is_wide() { 2.0 } else { 1.0 } * CELL_W;
            if bg != default_bg {
                rects.push((
                    BoxRect::new(
                        PAD_X + f32::from(col) * CELL_W,
                        PAD_Y + f32::from(row) * CELL_H,
                        width,
                        CELL_H,
                    ),
                    color_rect(bg),
                ));
            }
            if cell.underline() {
                rects.push((
                    BoxRect::new(
                        PAD_X + f32::from(col) * CELL_W,
                        PAD_Y + f32::from(row) * CELL_H + CELL_H - 2.0,
                        width,
                        1.0,
                    ),
                    color_rect(fg),
                ));
            }
            let text = cell.contents();
            let attrs = Attrs::new()
                .family(Family::Monospace)
                .color(fg)
                .weight(if cell.bold() {
                    glyphon::Weight::BOLD
                } else {
                    glyphon::Weight::NORMAL
                })
                .style(if cell.italic() {
                    glyphon::Style::Italic
                } else {
                    glyphon::Style::Normal
                });
            spans.push((if text.is_empty() { " ".into() } else { text }, attrs));
        }
        if row + 1 < rows {
            spans.push(("\n".into(), Attrs::new().family(Family::Monospace)));
        }
    }
    (spans, rects)
}

fn render_frame(state: &mut WindowState) -> Result<bool> {
    state.session.drain();
    pump_assistant(state);
    let scale = state.window.scale_factor() as f32;
    let layout = ChromeLayout::new(
        state.config.width as f32 / scale,
        state.config.height as f32 / scale,
    );
    let screen = state.session.screen().contents_formatted();
    if screen != state.last_screen {
        let (spans, rects) = terminal_spans(state.session.screen());
        state.buffer.set_rich_text(
            spans
                .iter()
                .map(|(text, attrs)| (text.as_str(), attrs.clone())),
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        state.terminal_rects = rects;
        state.last_screen = screen;
        state.last_text = state.session.screen_text();
    }
    state.buffer.set_monospace_width(Some(CELL_W));
    state.buffer.set_size(Some(layout.terminal.w), None);
    state
        .buffer
        .shape_until_scroll(&mut state.font_system, false);
    let status = status_line(state);
    set_cached(
        &mut state.font_system,
        &mut state.status_buf,
        &mut state.last_status,
        &status,
        layout.status.w - 24.0,
    );

    if state.panel_open {
        let rows = ((layout.list.h / LINE_HEIGHT) as usize)
            .saturating_sub(2)
            .max(1);
        let start = state.panel_sel.saturating_sub(rows - 1);
        let mut list_text = String::from("AI · ↑↓ select\nPgUp/PgDn scroll\n");
        let max_chars = (layout.list.w / CELL_W).floor() as usize;
        for (index, block) in state.ai.list().iter().enumerate().skip(start).take(rows) {
            let first = block.prompt.lines().next().unwrap_or("(empty)");
            let prefix = format!("{} ", if index == state.panel_sel { "›" } else { " " });
            let line = format!("{prefix}{first}");
            let truncated = line.chars().count() > max_chars;
            let mut line: String = line.chars().take(max_chars).collect();
            if truncated {
                line.pop();
                line.push('…');
            }
            list_text.push_str(&format!("{line}\n"));
        }
        if state.ai.is_empty() {
            list_text.push_str("Ctrl+J to ask");
        }
        set_cached(
            &mut state.font_system,
            &mut state.list_buf,
            &mut state.last_list,
            &list_text,
            layout.list.w,
        );
        let detail = match state.ai.list().get(state.panel_sel) {
            Some(block) => format!(
                "› {}\n\n● {}\n\n· {} rounds · {:.1}s · {}",
                block.prompt,
                if block.response.is_empty() && !block.done {
                    "…"
                } else {
                    &block.response
                },
                block.tool_rounds,
                block.wall_seconds,
                if block.done {
                    block.error.as_deref().unwrap_or("done")
                } else {
                    "streaming"
                }
            ),
            None => "No answers yet. Ctrl+J to ask.".into(),
        };
        set_cached(
            &mut state.font_system,
            &mut state.detail_buf,
            &mut state.last_detail,
            &detail,
            layout.detail.w,
        );
        let visual_lines = state.detail_buf.layout_runs().count();
        state.panel_scroll = state
            .panel_scroll
            .min(visual_lines.saturating_sub((layout.detail.h / LINE_HEIGHT) as usize));
    }

    let composer_body = layout.composer.inset(12.0);
    let mut caret = None;
    let mut composer_scroll = 0.0;
    if state.focus == Focus::Composer {
        let title = if state.ai_busy {
            "AI is working · draft preserved · Ctrl+C cancels"
        } else {
            "Ask r105 · Enter sends · Alt+Enter newline · Esc closes"
        };
        let mut body = state.composer.text().to_string();
        let byte = body
            .char_indices()
            .nth(state.composer.cursor())
            .map(|(at, _)| at)
            .unwrap_or(body.len());
        body.insert_str(byte, &state.preedit);
        let full = format!("{title}\n{body}");
        set_cached(
            &mut state.font_system,
            &mut state.composer_buf,
            &mut state.last_composer,
            &full,
            composer_body.w,
        );
        let (x, y) = caret_xy(
            &state.composer_buf,
            title.chars().count() + 1 + state.composer.cursor() + state.preedit.chars().count(),
            &full,
        );
        composer_scroll = (y + LINE_HEIGHT - composer_body.h).max(0.0);
        caret = Some(BoxRect::new(
            composer_body.x + x,
            composer_body.y + y - composer_scroll,
            2.0,
            LINE_HEIGHT.min(composer_body.h),
        ));
        state.window.set_ime_cursor_area(
            winit::dpi::LogicalPosition::new(
                composer_body.x + x,
                composer_body.y + y - composer_scroll,
            ),
            winit::dpi::LogicalSize::new(CELL_W, LINE_HEIGHT),
        );
    }

    let approval_body = layout.approval.inset(12.0);
    if let Some(approval) = &state.approval {
        let full = format!(
            "[y] Once  [a] Always  [n] Deny  Esc cancels\nApproval {}/{} · {}\n{}\n{}",
            approval.index + 1,
            approval.total,
            approval.name,
            approval.summary,
            approval.preview.as_deref().unwrap_or("")
        );
        set_cached(
            &mut state.font_system,
            &mut state.approval_buf,
            &mut state.last_approval,
            &full,
            approval_body.w,
        );
        state.approval_scroll = state.approval_scroll.min(
            state
                .approval_buf
                .layout_runs()
                .count()
                .saturating_sub((approval_body.h / LINE_HEIGHT) as usize),
        );
    }

    // Each entry is a separate text renderer: preparing the next layer must not
    // overwrite the previous layer's glyph vertices before queue submission.
    // The terminal canvas is visible only above the topmost open overlay;
    // clipping on a row boundary keeps every row either fully drawn or
    // fully covered by an overlay sheet.
    let terminal_bottom = if state.panel_open {
        PAD_Y
    } else if state.approval.is_some() {
        row_floor(layout.approval.y)
    } else if state.focus == Focus::Composer {
        row_floor(layout.composer.y)
    } else {
        layout.terminal.y + layout.terminal.h
    };
    let mut layers: Vec<(Vec<ColoredRect>, Vec<TextArea<'_>>)> = Vec::with_capacity(5);
    let mut terminal_rects = state.terminal_rects.clone();
    terminal_rects.extend(selection_rects(state));
    if !state.session.screen().hide_cursor()
        && state.focus == Focus::Terminal
        && state.session.screen().scrollback() == 0
    {
        let (row, col) = state.session.cursor();
        terminal_rects.push((
            BoxRect::new(
                PAD_X + f32::from(col) * CELL_W,
                PAD_Y + f32::from(row) * CELL_H + CELL_H - 2.0,
                CELL_W,
                2.0,
            ),
            color_rect(ACCENT),
        ));
        state.window.set_ime_cursor_area(
            winit::dpi::LogicalPosition::new(
                PAD_X + f32::from(col) * CELL_W,
                PAD_Y + f32::from(row) * CELL_H,
            ),
            winit::dpi::LogicalSize::new(CELL_W, CELL_H),
        );
    }
    layers.push((
        terminal_rects,
        vec![text_area(
            &state.buffer,
            layout.terminal,
            scale,
            FG,
            0.0,
            Some(BoxRect::new(
                0.0,
                PAD_Y,
                layout.size.0,
                terminal_bottom - PAD_Y,
            )),
        )],
    ));
    layers.push((
        vec![(layout.status, px((0x15, 0x1D, 0x2C)))],
        vec![text_area(
            &state.status_buf,
            BoxRect::new(
                12.0,
                layout.status.y + 5.0,
                layout.status.w - 24.0,
                layout.status.h - 5.0,
            ),
            scale,
            DIM,
            0.0,
            None,
        )],
    ));
    let mut panel_rects = Vec::new();
    let mut panel_areas = Vec::new();
    if state.panel_open {
        let sheet = layout.panel_sheet();
        panel_rects.push((sheet, px((0x0E, 0x13, 0x1F))));
        panel_rects.push((
            BoxRect::new(layout.detail.x - 12.0, 0.0, 1.0, layout.status.y),
            px((0x2A, 0x35, 0x4A)),
        ));
        panel_areas.push(text_area(
            &state.list_buf,
            layout.list,
            scale,
            DIM,
            0.0,
            None,
        ));
        panel_areas.push(text_area(
            &state.detail_buf,
            layout.detail,
            scale,
            FG,
            state.panel_scroll as f32 * LINE_HEIGHT,
            None,
        ));
    }
    layers.push((panel_rects, panel_areas));
    let mut composer_rects = Vec::new();
    let mut composer_areas = Vec::new();
    if state.focus == Focus::Composer {
        let top = row_floor(layout.composer.y);
        composer_rects.push((
            BoxRect::new(0.0, top, layout.size.0, layout.status.y - top),
            px((0x10, 0x16, 0x24)),
        ));
        if let Some(caret) = caret {
            composer_rects.push((caret, color_rect(ACCENT)));
        }
        composer_areas.push(text_area(
            &state.composer_buf,
            composer_body,
            scale,
            FG,
            composer_scroll,
            None,
        ));
    }
    layers.push((composer_rects, composer_areas));
    layers.push(if state.approval.is_some() {
        let top = row_floor(layout.approval.y);
        (
            vec![(
                BoxRect::new(0.0, top, layout.size.0, layout.status.y - top),
                px((0x2A, 0x20, 0x10)),
            )],
            vec![text_area(
                &state.approval_buf,
                approval_body,
                scale,
                ACCENT,
                state.approval_scroll as f32 * LINE_HEIGHT,
                None,
            )],
        )
    } else {
        (Vec::new(), Vec::new())
    });

    let mut rects = Vec::new();
    let mut ranges = Vec::new();
    for (index, (backgrounds, areas)) in layers.into_iter().enumerate() {
        let start = rects.len();
        rects.extend(backgrounds);
        ranges.push(start..rects.len());
        state.renderers[index]
            .prepare(
                &state.device,
                &state.queue,
                &mut state.font_system,
                &mut state.atlas,
                &state.viewport,
                areas,
                &mut state.swash,
            )
            .context("preparing layer text")?;
    }
    state
        .rects
        .prepare(&state.device, &state.queue, layout.size, &rects);
    let frame = match state.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(frame)
        | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            state.surface.configure(&state.device, &state.config);
            return Ok(false);
        }
        wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
            return Ok(false);
        }
        wgpu::CurrentSurfaceTexture::Validation => anyhow::bail!("GPU surface validation failed"),
    };
    let view = frame.texture.create_view(&Default::default());
    let mut encoder = state.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(BG),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        paint_layers(
            &state.rects,
            &state.renderers,
            &state.atlas,
            &state.viewport,
            &mut pass,
            ranges,
        )?;
    }
    state.queue.submit([encoder.finish()]);
    if let Some(path) = state.snapshot_path.take() {
        save_frame(
            &state.device,
            &state.queue,
            &frame.texture,
            state.config.format,
            state.config.width,
            state.config.height,
            &path,
        )?;
    }
    state.queue.present(frame);
    state.atlas.trim();
    Ok(true)
}

/// GPU readback of the real production frame, not a second renderer. PPM keeps
/// capture dependency-free and can be opened by image tools or converted to PNG.
fn save_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Result<()> {
    use std::io::Write;
    anyhow::ensure!(
        matches!(
            format,
            wgpu::TextureFormat::Rgba8Unorm
                | wgpu::TextureFormat::Rgba8UnormSrgb
                | wgpu::TextureFormat::Bgra8Unorm
                | wgpu::TextureFormat::Bgra8UnormSrgb
        ),
        "snapshot requires an 8-bit RGBA surface"
    );
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let bytes = buffer.slice(..).get_mapped_range()?;
    let mut file =
        std::io::BufWriter::new(std::fs::File::create(path).context("creating smoke snapshot")?);
    write!(file, "P6\n{width} {height}\n255\n")?;
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    for row in bytes.chunks_exact(stride as usize) {
        for pixel in row[..width as usize * 4].as_chunks::<4>().0 {
            file.write_all(&if bgra {
                [pixel[2], pixel[1], pixel[0]]
            } else {
                [pixel[0], pixel[1], pixel[2]]
            })?;
        }
    }
    file.flush()?;
    Ok(())
}

fn paint_layers(
    rects: &RectRenderer,
    renderers: &[glyphon::TextRenderer],
    atlas: &TextAtlas,
    viewport: &Viewport,
    pass: &mut wgpu::RenderPass<'_>,
    ranges: Vec<std::ops::Range<usize>>,
) -> Result<()> {
    for (index, range) in ranges.into_iter().enumerate() {
        rects.draw(pass, range);
        renderers[index]
            .render(atlas, viewport, pass)
            .context("rendering layer text")?;
    }
    Ok(())
}

enum KeyAction {
    Quit,
    Ignore,
    Bytes(Vec<u8>),
}

/// Map a winit key event to PTY bytes. `Cmd/Ctrl+Q` quits the window;
/// everything else forwards to the shell.
fn map_key(logical_key: &Key, text: Option<&str>, modifiers: &Modifiers) -> KeyAction {
    let state = modifiers.state();
    let quit_held = state.control_key() || state.super_key();
    if quit_held
        && let Key::Character(name) = logical_key
        && (name.as_str() == "q" || name.as_str() == "Q")
    {
        return KeyAction::Quit;
    }
    // Forward other Ctrl+keys as control bytes (Ctrl+C, Ctrl+D, ...).
    if state.control_key()
        && !state.super_key()
        && !state.alt_key()
        && let Key::Character(name) = logical_key
        && name.len() == 1
    {
        let lower = name.to_ascii_lowercase();
        let byte = lower.as_bytes()[0];
        if byte.is_ascii_lowercase() {
            return KeyAction::Bytes(vec![byte - b'a' + 1]);
        }
        return KeyAction::Ignore;
    }
    match logical_key {
        Key::Named(NamedKey::Enter) => KeyAction::Bytes(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => KeyAction::Bytes(vec![0x7f]),
        Key::Named(NamedKey::Tab) => {
            if state.shift_key() {
                KeyAction::Bytes(vec![0x1b, b'[', b'Z'])
            } else {
                KeyAction::Bytes(vec![b'\t'])
            }
        }
        Key::Named(NamedKey::Escape) => KeyAction::Bytes(vec![0x1b]),
        Key::Named(NamedKey::ArrowLeft) => KeyAction::Bytes(vec![0x1b, b'[', b'D']),
        Key::Named(NamedKey::ArrowRight) => KeyAction::Bytes(vec![0x1b, b'[', b'C']),
        Key::Named(NamedKey::ArrowUp) => KeyAction::Bytes(vec![0x1b, b'[', b'A']),
        Key::Named(NamedKey::ArrowDown) => KeyAction::Bytes(vec![0x1b, b'[', b'B']),
        Key::Named(NamedKey::Home) => KeyAction::Bytes(vec![0x1b, b'[', b'H']),
        Key::Named(NamedKey::End) => KeyAction::Bytes(vec![0x1b, b'[', b'F']),
        Key::Named(NamedKey::Delete) => KeyAction::Bytes(vec![0x1b, b'[', b'3', b'~']),
        Key::Named(NamedKey::Insert) => KeyAction::Bytes(vec![0x1b, b'[', b'2', b'~']),
        Key::Named(NamedKey::PageUp) => KeyAction::Bytes(vec![0x1b, b'[', b'5', b'~']),
        Key::Named(NamedKey::PageDown) => KeyAction::Bytes(vec![0x1b, b'[', b'6', b'~']),
        Key::Named(NamedKey::F1) => KeyAction::Bytes(vec![0x1b, b'O', b'P']),
        Key::Named(NamedKey::F2) => KeyAction::Bytes(vec![0x1b, b'O', b'Q']),
        Key::Named(NamedKey::F3) => KeyAction::Bytes(vec![0x1b, b'O', b'R']),
        Key::Named(NamedKey::F4) => KeyAction::Bytes(vec![0x1b, b'O', b'S']),
        Key::Named(NamedKey::F5) => KeyAction::Bytes(b"\x1b[15~".to_vec()),
        Key::Named(NamedKey::F6) => KeyAction::Bytes(b"\x1b[17~".to_vec()),
        Key::Named(NamedKey::F7) => KeyAction::Bytes(b"\x1b[18~".to_vec()),
        Key::Named(NamedKey::F8) => KeyAction::Bytes(b"\x1b[19~".to_vec()),
        Key::Named(NamedKey::F9) => KeyAction::Bytes(b"\x1b[20~".to_vec()),
        Key::Named(NamedKey::F10) => KeyAction::Bytes(b"\x1b[21~".to_vec()),
        Key::Named(NamedKey::F11) => KeyAction::Bytes(b"\x1b[23~".to_vec()),
        Key::Named(NamedKey::F12) => KeyAction::Bytes(b"\x1b[24~".to_vec()),
        Key::Character(_) => {
            if state.super_key() || state.alt_key() {
                return KeyAction::Ignore;
            }
            match text {
                Some(text) => KeyAction::Bytes(text.as_bytes().to_vec()),
                None => KeyAction::Ignore,
            }
        }
        _ => KeyAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::{KeyCode, PhysicalKey};

    fn no_modifiers() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn grid_clamps_to_at_least_one_cell() {
        let (rows, cols) = WindowApp::grid_for(winit::dpi::PhysicalSize::new(10, 10), 1.0);
        assert_eq!((rows, cols), (1, 1));
    }

    #[test]
    fn grid_grows_with_window() {
        let small = WindowApp::grid_for(winit::dpi::PhysicalSize::new(200, 200), 1.0);
        let big = WindowApp::grid_for(winit::dpi::PhysicalSize::new(1800, 1200), 1.0);
        assert!(big.0 > small.0 && big.1 > small.1);
    }

    #[test]
    fn retina_grid_matches_logical_layout() {
        let normal = WindowApp::grid_for(winit::dpi::PhysicalSize::new(900, 600), 1.0);
        let retina = WindowApp::grid_for(winit::dpi::PhysicalSize::new(1800, 1200), 2.0);
        assert_eq!(normal, retina);
        let layout = ChromeLayout::new(900.0, 600.0);
        assert!(f32::from(normal.0) * CELL_H <= layout.terminal.h);
        assert!(f32::from(normal.1) * CELL_W <= layout.terminal.w);
    }

    #[test]
    fn terminal_preserves_soft_wrap_blank_rows_and_ansi_backgrounds() {
        let mut parser = vt100::Parser::new(4, 5, 0);
        parser.process(b"abcdefghij\r\n\r\n\x1b[41mZ");
        let (spans, rects) = terminal_spans(parser.screen());
        let text = spans
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<String>();
        assert_eq!(text, "abcde\nfghij\n     \nZ    ");
        assert!(rects.iter().any(|(rect, _)| rect.y == PAD_Y + 3.0 * CELL_H));
    }

    #[test]
    fn cached_text_reflows_after_resize_and_caret_follows_wrap() {
        let text = "one two three four five six seven eight nine ten";
        let (mut fonts, mut buffer) = shaped_buffer(text);
        let mut last = text.to_string();
        set_cached(&mut fonts, &mut buffer, &mut last, text, 100.0);
        let narrow = buffer.layout_runs().count();
        assert!(narrow > 1);
        let (_, y) = caret_xy(&buffer, text.chars().count(), text);
        assert!(y >= LINE_HEIGHT);
        set_cached(&mut fonts, &mut buffer, &mut last, text, 800.0);
        assert_eq!(buffer.layout_runs().count(), 1);
        assert_eq!(caret_xy(&buffer, text.chars().count(), text).1, 0.0);
    }

    #[test]
    fn overlay_backgrounds_start_on_whole_terminal_rows() {
        for (width, height) in [(900.0, 600.0), (640.0, 480.0), (1440.0, 900.0)] {
            let layout = ChromeLayout::new(width, height);
            for y in [layout.composer.y, layout.approval.y] {
                let top = row_floor(y);
                assert_eq!((top - PAD_Y) % CELL_H, 0.0, "{width}x{height}: y={y}");
                assert!(top <= y && top + CELL_H > y, "{width}x{height}: y={y}");
                assert!(
                    top >= PAD_Y && top < layout.status.y,
                    "{width}x{height}: y={y}"
                );
            }
            let sheet = layout.panel_sheet();
            assert_eq!(
                (sheet.x, sheet.y, sheet.w, sheet.h),
                (0.0, 0.0, width, layout.status.y)
            );
        }
    }

    #[test]
    fn enter_maps_to_carriage_return() {
        let action = map_key(&Key::Named(NamedKey::Enter), None, &no_modifiers());
        assert!(matches!(action, KeyAction::Bytes(bytes) if bytes == b"\r"));
    }

    #[test]
    fn arrows_map_to_escape_sequences() {
        let action = map_key(&Key::Named(NamedKey::ArrowUp), None, &no_modifiers());
        assert!(matches!(action, KeyAction::Bytes(bytes) if bytes == [0x1b, b'[', b'A']));
    }

    #[test]
    fn physical_key_is_ignored_without_logical_text() {
        // Physical location alone never types; the logical key decides.
        let _physical = PhysicalKey::Code(KeyCode::KeyQ);
        let action = map_key(&Key::Character("q".into()), Some("q"), &no_modifiers());
        assert!(matches!(action, KeyAction::Bytes(bytes) if bytes == b"q"));
    }

    fn shaped_buffer(text: &str) -> (FontSystem, Buffer) {
        let mut font_system = FontSystem::new();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(FONT_SIZE, LINE_HEIGHT));
        buffer.set_wrap(Wrap::Word);
        buffer.set_size(Some(800.0), Some(600.0));
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Monospace),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);
        (font_system, buffer)
    }

    #[test]
    fn caret_at_zero_sits_at_line_start() {
        let (_fonts, buffer) = shaped_buffer("hello");
        let (x, y) = caret_xy(&buffer, 0, "hello");
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn caret_advances_with_text() {
        let (_fonts, buffer) = shaped_buffer("hello");
        let (start, _) = caret_xy(&buffer, 0, "hello");
        let (end, _) = caret_xy(&buffer, 5, "hello");
        assert!(end > start, "start={start} end={end}");
    }

    #[test]
    fn caret_tracks_second_line() {
        let (_fonts, buffer) = shaped_buffer("ab\ncdef");
        let (x, y) = caret_xy(&buffer, 4, "ab\ncdef");
        assert!(y > 0.0, "y={y}");
        assert!(x > 0.0, "x={x}");
    }
}
