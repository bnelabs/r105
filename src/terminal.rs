//! Terminal core (spec 0021).
//!
//! PTY sessions via `portable-pty`, screen emulation via `vt100`, and a
//! block store recording command, working directory, and exit status.
//! Interactive shells receive lightweight OSC 7/133/633 integration when
//! their shell is recognized; the parser also accepts markers emitted by
//! user shell configuration and falls back honestly when none are present.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyModifiers};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use uuid::Uuid;

/// Keep the last 32 KiB of PTY output per block and per one-shot run.
pub const OUTPUT_TAIL_MAX: usize = 32 * 1024;

/// One command block: what ran, where, when, and how it ended.
#[derive(Debug, Clone)]
pub struct TerminalBlock {
    pub seq: u64,
    pub command: String,
    pub cwd: String,
    pub started: SystemTime,
    pub ended: Option<SystemTime>,
    pub exit_code: Option<i32>,
    pub output_tail: String,
}

impl TerminalBlock {
    pub fn status_label(&self) -> String {
        match self.exit_code {
            Some(code) => format!("exit {code}"),
            None if self.ended.is_some() => "done".to_string(),
            None => "running".to_string(),
        }
    }
}

/// Append-only block list with sequence numbers starting at 1.
#[derive(Debug, Default)]
pub struct BlockStore {
    blocks: Vec<TerminalBlock>,
    next_seq: u64,
}

impl BlockStore {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_seq: 1,
        }
    }

    pub fn push(
        &mut self,
        command: &str,
        cwd: &str,
        exit_code: Option<i32>,
        output_tail: String,
    ) -> &TerminalBlock {
        let block = TerminalBlock {
            seq: self.next_seq,
            command: command.to_string(),
            cwd: cwd.to_string(),
            started: SystemTime::now(),
            ended: Some(SystemTime::now()),
            exit_code,
            output_tail,
        };
        self.next_seq += 1;
        self.blocks.push(block);
        self.blocks.last().expect("just pushed")
    }

    /// Start an interactive block. Shells without integration are still
    /// sealed honestly when the next command begins.
    pub fn begin(&mut self, command: &str, cwd: &str) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.blocks.push(TerminalBlock {
            seq,
            command: command.to_string(),
            cwd: cwd.to_string(),
            started: SystemTime::now(),
            ended: None,
            exit_code: None,
            output_tail: String::new(),
        });
        seq
    }

    /// Seal the open block (if any) with its end time and output tail.
    pub fn seal_open(&mut self, output_tail: String) {
        let _ = self.finish_open(None, output_tail);
    }

    /// Finish the open block with an optional shell-reported exit status.
    /// Returns whether a block was actually open.
    pub fn finish_open(&mut self, exit_code: Option<i32>, output_tail: String) -> bool {
        if let Some(last) = self.blocks.last_mut()
            && last.ended.is_none()
        {
            last.ended = Some(SystemTime::now());
            last.exit_code = exit_code;
            last.output_tail = output_tail;
            return true;
        }
        false
    }

    /// Replace the command text for a shell integration event that carries
    /// the command itself (OSC 633;E). Normal key input already supplies it.
    pub fn set_open_command(&mut self, command: &str) {
        if let Some(last) = self.blocks.last_mut()
            && last.ended.is_none()
            && !command.trim().is_empty()
        {
            last.command = command.trim().to_string();
        }
    }

    pub fn list(&self) -> &[TerminalBlock] {
        &self.blocks
    }

    pub fn last(&self) -> Option<&TerminalBlock> {
        self.blocks.last()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Shell for interactive sessions: `$SHELL`, else `sh` (Unix) or
/// `cmd.exe` (Windows).
pub fn default_shell() -> String {
    if let Some(shell) = std::env::var_os("SHELL").map(PathBuf::from)
        && shell.is_absolute()
    {
        return shell.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    return "cmd.exe".to_string();
    #[cfg(not(windows))]
    return "sh".to_string();
}

const MARKER_PAYLOAD_MAX: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ShellMarker {
    Cwd(String),
    PromptStart,
    CommandStart,
    CommandOutput,
    CommandEnd(Option<i32>),
    CommandText(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum MarkerState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}

/// Incremental parser for OSC shell-integration records. PTY reads can split
/// an escape sequence at any byte, so this deliberately keeps only one
/// bounded OSC payload between calls and ignores unrelated terminal output.
#[derive(Debug, Default)]
struct ShellMarkerParser {
    state: MarkerState,
    payload: Vec<u8>,
}

impl ShellMarkerParser {
    fn push(&mut self, bytes: &[u8]) -> Vec<ShellMarker> {
        let mut events = Vec::new();
        for &byte in bytes {
            match self.state {
                MarkerState::Ground => {
                    if byte == 0x1b {
                        self.state = MarkerState::Esc;
                    }
                }
                MarkerState::Esc => match byte {
                    b']' => {
                        self.payload.clear();
                        self.state = MarkerState::Osc;
                    }
                    0x1b => {}
                    _ => self.state = MarkerState::Ground,
                },
                MarkerState::Osc => match byte {
                    0x07 => self.finish(&mut events),
                    0x1b => self.state = MarkerState::OscEsc,
                    _ => self.push_payload(byte),
                },
                MarkerState::OscEsc => match byte {
                    b'\\' | 0x07 => self.finish(&mut events),
                    0x1b => {}
                    _ => {
                        self.push_payload(0x1b);
                        self.push_payload(byte);
                        if self.state != MarkerState::Ground {
                            self.state = MarkerState::Osc;
                        }
                    }
                },
            }
        }
        events
    }

    fn push_payload(&mut self, byte: u8) {
        if self.payload.len() >= MARKER_PAYLOAD_MAX {
            self.payload.clear();
            self.state = MarkerState::Ground;
        } else {
            self.payload.push(byte);
        }
    }

    fn finish(&mut self, events: &mut Vec<ShellMarker>) {
        let bytes = std::mem::take(&mut self.payload);
        let payload = String::from_utf8_lossy(&bytes);
        if let Some(event) = parse_shell_marker(&payload) {
            events.push(event);
        }
        self.state = MarkerState::Ground;
    }
}

fn parse_shell_marker(payload: &str) -> Option<ShellMarker> {
    let (kind, fields) = payload.split_once(';').unwrap_or((payload, ""));
    match kind {
        "7" => parse_marker_cwd(fields).map(ShellMarker::Cwd),
        "133" | "633" => {
            let (event, rest) = fields.split_once(';').unwrap_or((fields, ""));
            match event {
                "A" => Some(ShellMarker::PromptStart),
                "B" => Some(ShellMarker::CommandStart),
                "C" => Some(ShellMarker::CommandOutput),
                "D" => Some(ShellMarker::CommandEnd(parse_marker_exit(rest))),
                "E" if kind == "633" => {
                    (!rest.is_empty()).then(|| ShellMarker::CommandText(rest.to_string()))
                }
                "P" if kind == "633" => rest
                    .strip_prefix("Cwd=")
                    .and_then(parse_marker_cwd)
                    .map(ShellMarker::Cwd),
                _ => None,
            }
        }
        _ => None,
    }
}

fn parse_marker_exit(fields: &str) -> Option<i32> {
    for field in fields.split(';') {
        let value = field
            .strip_prefix("exit=")
            .or_else(|| field.strip_prefix("exit_code="))
            .or_else(|| field.strip_prefix("status="));
        if let Some(value) = value {
            return value.parse().ok();
        }
    }
    fields.trim().parse().ok()
}

fn parse_marker_cwd(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(url) = url::Url::parse(value)
        && url.scheme() == "file"
    {
        if let Ok(path) = url.to_file_path() {
            return Some(path.to_string_lossy().into_owned());
        }
        return Some(percent_decode_path(url.path()));
    }
    Some(percent_decode_path(
        value.strip_prefix("file://").unwrap_or(value),
    ))
}

fn percent_decode_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Temporary shell startup files install markers without replacing the
/// user's shell binary or requiring a separate helper executable.
struct ShellIntegration {
    temp_root: Option<PathBuf>,
}

impl ShellIntegration {
    fn configure(command: &mut CommandBuilder, shell: &Path) -> Result<Self> {
        let name = shell
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match name.as_str() {
            "bash" | "rbash" => Self::configure_bash(command),
            "zsh" => Self::configure_zsh(command),
            "fish" => Self::configure_fish(command),
            _ => Ok(Self { temp_root: None }),
        }
    }

    fn configure_bash(command: &mut CommandBuilder) -> Result<Self> {
        let root = shell_temp_root()?;
        let rc = root.join("bashrc");
        let source = source_shell_file(home_dir().map(|home| home.join(".bashrc")), false);
        let contents = format!(
            r#"{source}
__r105_user_prompt_command=${{PROMPT_COMMAND-}}
__r105_prompt_command() {{
  local __r105_status=$?
  if [ -n "$__r105_user_prompt_command" ]; then
    eval "$__r105_user_prompt_command"
  fi
  printf '\033]133;D;exit=%s\a' "$__r105_status"
  printf '\033]7;file://%s%s\a' "${{HOSTNAME:-}}" "$PWD"
  printf '\033]133;A\a'
}}
PROMPT_COMMAND=__r105_prompt_command
PS0=$'\033]133;B\a\033]133;C\a'"${{PS0-}}"
"#
        );
        fs::write(&rc, contents).context("writing bash shell integration")?;
        command.arg("--rcfile");
        command.arg(&rc);
        command.arg("-i");
        Ok(Self {
            temp_root: Some(root),
        })
    }

    fn configure_zsh(command: &mut CommandBuilder) -> Result<Self> {
        let root = shell_temp_root()?;
        let source_env = source_shell_file(home_dir().map(|home| home.join(".zshenv")), true);
        let source_rc = source_shell_file(home_dir().map(|home| home.join(".zshrc")), true);
        fs::write(root.join(".zshenv"), source_env)
            .context("writing zsh environment integration")?;
        let contents = format!(
            r#"{source_rc}
autoload -Uz add-zsh-hook
__r105_precmd() {{
  local __r105_status=$?
  printf '\033]133;D;exit=%s\a' "$__r105_status"
  printf '\033]7;file://%s%s\a' "${{HOST:-$HOSTNAME}}" "$PWD"
  printf '\033]133;A\a'
}}
__r105_preexec() {{
  printf '\033]133;B\a\033]133;C\a'
}}
add-zsh-hook precmd __r105_precmd
add-zsh-hook preexec __r105_preexec
"#
        );
        fs::write(root.join(".zshrc"), contents).context("writing zsh shell integration")?;
        command.env("ZDOTDIR", &root);
        command.arg("-i");
        Ok(Self {
            temp_root: Some(root),
        })
    }

    fn configure_fish(command: &mut CommandBuilder) -> Result<Self> {
        let root = shell_temp_root()?;
        let fish_dir = root.join("fish");
        fs::create_dir_all(&fish_dir).context("creating fish integration directory")?;
        let original = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".config")))
            .map(|path| path.join("fish/config.fish"));
        let source = source_fish_file(original);
        let contents = format!(
            r#"{source}
function __r105_preexec --on-event fish_preexec
    printf '\033]133;B\a\033]133;C\a'
end
function __r105_postexec --on-event fish_postexec
    set -l __r105_status $status
    printf '\033]133;D;exit=%s\a' $__r105_status
    printf '\033]7;file://%s%s\a' (hostname) $PWD
    printf '\033]133;A\a'
end
"#
        );
        fs::write(fish_dir.join("config.fish"), contents)
            .context("writing fish shell integration")?;
        command.env("XDG_CONFIG_HOME", &root);
        command.arg("-i");
        Ok(Self {
            temp_root: Some(root),
        })
    }
}

impl Drop for ShellIntegration {
    fn drop(&mut self) {
        if let Some(root) = self.temp_root.take() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn shell_temp_root() -> Result<PathBuf> {
    let root = env::temp_dir().join(format!("r105-shell-{}", Uuid::new_v4().simple()));
    fs::create_dir(&root).with_context(|| format!("creating shell integration {root:?}"))?;
    Ok(root)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn source_shell_file(path: Option<PathBuf>, zsh: bool) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let quoted = shell_quote(&path);
    if zsh {
        format!("if [[ -r {quoted} ]]; then source {quoted}; fi\n")
    } else {
        format!("if [ -r {quoted} ]; then . {quoted}; fi\n")
    }
}

fn source_fish_file(path: Option<PathBuf>) -> String {
    let Some(path) = path else {
        return String::new();
    };
    let quoted = shell_quote(&path);
    format!("if test -r {quoted}\n    source {quoted}\nend\n")
}

/// A persistent interactive shell in a PTY with vt100 screen state.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _shell_integration: ShellIntegration,
    writer: Option<Box<dyn Write + Send>>,
    reader_thread: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
    rows: u16,
    cols: u16,
    /// Current working directory reported by OSC 7/633 markers, falling back
    /// to the launch directory when the shell has no integration.
    pub cwd: String,
    pub blocks: BlockStore,
    block_tail: Vec<u8>,
    marker_parser: ShellMarkerParser,
}

impl PtySession {
    pub fn spawn(cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let system = native_pty_system();
        let pair = system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening pty")?;
        let shell = default_shell();
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let shell_integration = ShellIntegration::configure(&mut cmd, Path::new(&shell))?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("spawning shell in pty")?;
        let writer = pair.master.take_writer().context("taking pty writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("cloning pty reader")?;
        // Apply backpressure rather than allocating without bound on noisy jobs.
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(128);
        let reader_thread = std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            master: pair.master,
            child,
            _shell_integration: shell_integration,
            writer: Some(writer),
            reader_thread: Some(reader_thread),
            rx,
            parser: vt100::Parser::new(rows, cols, 1000),
            rows,
            cols,
            cwd: cwd.to_string_lossy().into_owned(),
            blocks: BlockStore::new(),
            block_tail: Vec::new(),
            marker_parser: ShellMarkerParser::default(),
        })
    }

    /// Write input bytes to the shell.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        if let Some(writer) = self.writer.as_mut() {
            writer.write_all(bytes).context("writing to pty")?;
            writer.flush().context("flushing pty")?;
        }
        Ok(())
    }

    /// Drain pending output, feed the vt100 parser, return raw bytes.
    pub fn drain(&mut self) -> Vec<u8> {
        let mut combined = Vec::new();
        while combined.len() < 256 * 1024 {
            let Ok(chunk) = self.rx.try_recv() else {
                break;
            };
            combined.extend_from_slice(&chunk);
        }
        if !combined.is_empty() {
            self.parser.process(&combined);
            if self
                .blocks
                .last()
                .is_some_and(|block| block.ended.is_none())
            {
                self.block_tail.extend_from_slice(&combined);
                if self.block_tail.len() > OUTPUT_TAIL_MAX {
                    let excess = self.block_tail.len() - OUTPUT_TAIL_MAX;
                    self.block_tail.drain(..excess);
                }
            }
            let markers = self.marker_parser.push(&combined);
            self.apply_markers(markers);
        }
        combined
    }

    pub fn screen_text(&self) -> String {
        self.parser.screen().contents()
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub fn scroll(&mut self, lines: i32) {
        let offset = self
            .parser
            .screen()
            .scrollback()
            .saturating_add_signed(lines as isize);
        self.parser.set_scrollback(offset);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.parser.set_scrollback(0);
    }

    pub fn cursor(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let rows = rows.max(1);
        let cols = cols.max(1);
        self.rows = rows;
        self.cols = cols;
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing pty")?;
        self.parser.set_size(rows, cols);
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        // Read back from the kernel so callers see the live size.
        self.master
            .get_size()
            .map(|size| (size.rows, size.cols))
            .unwrap_or((self.rows, self.cols))
    }

    /// Snapshot an interactive block for the just-submitted input line.
    pub fn begin_block(&mut self, command_line: &str) -> u64 {
        let command = command_line.trim().to_string();
        if command.is_empty() {
            return 0;
        }
        let tail = std::mem::take(&mut self.block_tail);
        self.blocks
            .seal_open(String::from_utf8_lossy(&tail).into_owned());
        self.blocks.begin(&command, &self.cwd.clone())
    }

    fn apply_markers(&mut self, markers: Vec<ShellMarker>) {
        for marker in markers {
            match marker {
                ShellMarker::Cwd(cwd) => self.cwd = cwd,
                ShellMarker::PromptStart => {
                    if self
                        .blocks
                        .last()
                        .is_some_and(|block| block.ended.is_none())
                    {
                        let tail = std::mem::take(&mut self.block_tail);
                        self.blocks
                            .finish_open(None, String::from_utf8_lossy(&tail).into_owned());
                    }
                }
                ShellMarker::CommandStart | ShellMarker::CommandOutput => {}
                ShellMarker::CommandEnd(exit_code) => {
                    if self
                        .blocks
                        .last()
                        .is_some_and(|block| block.ended.is_none())
                    {
                        let tail = std::mem::take(&mut self.block_tail);
                        self.blocks
                            .finish_open(exit_code, String::from_utf8_lossy(&tail).into_owned());
                    }
                }
                ShellMarker::CommandText(command) => {
                    self.blocks.set_open_command(&command);
                }
            }
        }
    }

    pub fn child_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        self.writer.take();
        self.kill();
        // A descendant can retain the slave PTY after the shell exits. Do not
        // block the UI joining a reader waiting on that descendant. Disconnect
        // the bounded channel now so a reader waiting to send can also exit.
        let (_, empty) = mpsc::channel();
        self.rx = empty;
        self.reader_thread.take();
    }
}

/// Output of a one-shot PTY command with exact exit and cwd.
#[derive(Debug, Clone)]
pub struct OneShotOutput {
    pub exit_code: i32,
    pub output: String,
    pub cwd: String,
}

/// Run one command in a fresh PTY, wait for it, capture output.
///
/// Used by `r105 terminal -- <cmd>` and by tests. Not a persistent
/// shell: each call spawns, waits with `timeout`, kills on expiry.
pub fn run_command_in_pty(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
) -> Result<OneShotOutput> {
    let system = native_pty_system();
    let pair = system
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening pty")?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args.iter().map(|arg| arg.as_str()));
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .context("spawning command in pty")?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("cloning pty reader")?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait().context("polling pty child")? {
            Some(status) => break status,
            None => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader_thread.join();
                    let mut raw = Vec::new();
                    while let Ok(chunk) = rx.try_recv() {
                        raw.extend_from_slice(&chunk);
                    }
                    return Ok(OneShotOutput {
                        exit_code: 124,
                        output: tail_text(&raw),
                        cwd: cwd.to_string_lossy().into_owned(),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let _ = reader_thread.join();
    let mut raw = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        raw.extend_from_slice(&chunk);
    }
    // vt100 parse keeps the helper honest about escape-heavy output.
    let mut parser = vt100::Parser::new(24, 100, 0);
    parser.process(&raw);
    let _ = parser.screen().contents();
    Ok(OneShotOutput {
        exit_code: status.exit_code() as i32,
        output: tail_text(&raw),
        cwd: cwd.to_string_lossy().into_owned(),
    })
}

fn tail_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw).into_owned();
    if text.len() > OUTPUT_TAIL_MAX {
        text[text.len() - OUTPUT_TAIL_MAX..].to_string()
    } else {
        text
    }
}

/// Run the interactive terminal TUI: a persistent shell in a PTY with
/// a vt100 screen, a status bar, and a block-list overlay.
///
/// Keys are forwarded to the shell; `Ctrl+Q` quits, `Ctrl+B` toggles
/// the block list. `Enter` snapshots a block (input mirror, current cwd,
/// and marker-reported exit when available). Owns the alternate screen
/// until quit; the caller prints nothing while it runs.
pub fn run_interactive(workspace: &Path) -> Result<()> {
    use crossterm::{
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };

    enable_raw_mode().context("enabling terminal raw mode")?;
    let mut output = std::io::stdout();
    execute!(output, EnterAlternateScreen).context("entering alternate screen")?;

    let outcome = run_loop_in(workspace);

    // Always leave the alternate screen, even on error.
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    let blocks = outcome?;
    eprintln!(
        "terminal: {} block{} recorded",
        blocks.len(),
        if blocks.len() == 1 { "" } else { "s" }
    );
    Ok(())
}

pub(crate) fn run_loop_in(workspace: &Path) -> Result<BlockStore> {
    use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEventKind};
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Clear, Paragraph},
    };
    let mut terminal =
        Terminal::new(CrosstermBackend::new(std::io::stdout())).context("creating terminal")?;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((100, 30));
    let mut session = PtySession::spawn(workspace, rows.max(2) - 1, cols)?;
    let mut input_line = String::new();
    let mut show_blocks = false;
    let mut status = String::from("terminal");
    loop {
        // Drain PTY output before every frame so the screen is fresh.
        session.drain();
        if !session.child_alive() {
            status = "shell exited — Ctrl+Q to leave".to_string();
        }
        terminal
            .draw(|frame| {
                let area = frame.area();
                let screen_height = area.height.saturating_sub(1) as usize;
                let lines: Vec<Line> = session
                    .screen_text()
                    .lines()
                    .take(screen_height)
                    .map(|line| Line::from(line.to_string()))
                    .collect();
                let screen_rect =
                    Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1));
                frame.render_widget(Paragraph::new(lines), screen_rect);
                let last_exit = session
                    .blocks
                    .last()
                    .map(|block| block.status_label())
                    .unwrap_or_else(|| "no blocks yet".to_string());
                let (size_rows, size_cols) = session.size();
                let bar = Line::from(vec![
                    Span::styled(
                        format!(" block #{} ", session.blocks.len() + 1,),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!(
                        " {} | {} | {}x{} | {} | Ctrl+B blocks Ctrl+Q quit",
                        session.cwd, last_exit, size_cols, size_rows, status,
                    )),
                ]);
                frame.render_widget(
                    Paragraph::new(bar).style(Style::default().bg(Color::DarkGray)),
                    Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
                );
                if show_blocks {
                    let items: Vec<Line> = if session.blocks.is_empty() {
                        vec![Line::from("no blocks yet — type a command and press Enter")]
                    } else {
                        session
                            .blocks
                            .list()
                            .iter()
                            .rev()
                            .take(screen_height.saturating_sub(2).max(1))
                            .map(|block| {
                                let label = if block.command.is_empty() {
                                    "(empty line)".to_string()
                                } else {
                                    block.command.clone()
                                };
                                let elapsed = block
                                    .ended
                                    .unwrap_or_else(SystemTime::now)
                                    .duration_since(block.started)
                                    .map(|span| format!("{:.1}s", span.as_secs_f64()))
                                    .unwrap_or_else(|_| "?".to_string());
                                Line::from(format!(
                                    " #{} {} {} [{}] {}",
                                    block.seq,
                                    block.status_label(),
                                    elapsed,
                                    block.cwd,
                                    label,
                                ))
                            })
                            .collect()
                    };
                    let overlay = Rect::new(
                        area.x + area.width.min(4) / 2,
                        area.y + 1,
                        area.width.saturating_sub(4).max(10),
                        (items.len() as u16 + 2).min(area.height.max(3) - 1),
                    );
                    frame.render_widget(Clear, overlay);
                    frame.render_widget(
                        Paragraph::new(items).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" blocks (OSC shell integration) "),
                        ),
                        overlay,
                    );
                }
                // Place the hardware cursor where the shell thinks it is.
                let (cursor_row, cursor_col) = session.cursor();
                frame.set_cursor_position((
                    screen_rect.x + cursor_col.min(screen_rect.width.saturating_sub(1)),
                    screen_rect.y + cursor_row.min(screen_rect.height.saturating_sub(1)),
                ));
            })
            .context("drawing terminal frame")?;

        if !event::poll(Duration::from_millis(50)).context("polling events")? {
            continue;
        }
        match event::read().context("reading event")? {
            Event::Resize(cols, rows) => {
                let _ = session.resize(rows.max(2) - 1, cols);
            }
            Event::Mouse(mouse) => match mouse.kind {
                // The TUI owns no mouse surface; swallow so the
                // shell never sees stray escape sequences.
                MouseEventKind::Down(_)
                | MouseEventKind::Up(_)
                | MouseEventKind::Drag(_)
                | MouseEventKind::Moved
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight => {}
            },
            Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) if kind != KeyEventKind::Release => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    match code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => {
                            break;
                        }
                        KeyCode::Char('b') | KeyCode::Char('B') => {
                            show_blocks = !show_blocks;
                            continue;
                        }
                        KeyCode::Char(c) => {
                            // Forward other Ctrl+keys as control bytes.
                            let lower = c.to_ascii_lowercase() as u8;
                            if lower.is_ascii_lowercase() {
                                let _ = session.write(&[lower - b'a' + 1]);
                                continue;
                            }
                        }
                        _ => {}
                    }
                }
                match key_bytes(code, modifiers) {
                    Some(KeyAction::Quit) => break,
                    Some(KeyAction::ToggleBlocks) => show_blocks = !show_blocks,
                    Some(KeyAction::Bytes(bytes)) => {
                        if code == KeyCode::Enter {
                            let line = std::mem::take(&mut input_line);
                            session.begin_block(&line);
                        } else {
                            mirror_edit(&mut input_line, code, modifiers);
                        }
                        let _ = session.write(&bytes);
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }
    session.kill();
    Ok(std::mem::replace(&mut session.blocks, BlockStore::new()))
}

enum KeyAction {
    Quit,
    ToggleBlocks,
    Bytes(Vec<u8>),
}

/// Map a crossterm key to PTY bytes. `Ctrl+Q`/`Ctrl+B` are UI keys and
/// handled by the caller; everything else forwards to the shell.
fn key_bytes(code: KeyCode, modifiers: KeyModifiers) -> Option<KeyAction> {
    if modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Some(KeyAction::Quit),
            KeyCode::Char('b') | KeyCode::Char('B') => {
                return Some(KeyAction::ToggleBlocks);
            }
            _ => return None,
        }
    }
    let bytes = match code {
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => vec![0x1b, b'[', b'Z'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => vec![0x1b, b'[', b'D'],
        KeyCode::Right => vec![0x1b, b'[', b'C'],
        KeyCode::Up => vec![0x1b, b'[', b'A'],
        KeyCode::Down => vec![0x1b, b'[', b'B'],
        KeyCode::Home => vec![0x1b, b'[', b'H'],
        KeyCode::End => vec![0x1b, b'[', b'F'],
        KeyCode::Delete => vec![0x1b, b'[', b'3', b'~'],
        KeyCode::Insert => vec![0x1b, b'[', b'2', b'~'],
        KeyCode::PageUp => vec![0x1b, b'[', b'5', b'~'],
        KeyCode::PageDown => vec![0x1b, b'[', b'6', b'~'],
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::F(1) => vec![0x1b, b'O', b'P'],
        KeyCode::F(2) => vec![0x1b, b'O', b'Q'],
        KeyCode::F(3) => vec![0x1b, b'O', b'R'],
        KeyCode::F(4) => vec![0x1b, b'O', b'S'],
        KeyCode::F(n @ 5..=12) => format!("\x1b[{0}~", 15 + (n - 5)).into_bytes(),
        _ => return None,
    };
    Some(KeyAction::Bytes(bytes))
}

/// Approximate the submitted command line for block snapshots.
/// The shell owns the true line editor; this mirror only needs the
/// printable tail for block titles.
fn mirror_edit(line: &mut String, code: KeyCode, modifiers: KeyModifiers) {
    if modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT) {
        if code == KeyCode::Backspace {
            line.clear();
        }
        return;
    }
    match code {
        KeyCode::Char(c) => line.push(c),
        KeyCode::Backspace => {
            line.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn workspace() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn one_shot_records_exit_zero() {
        let output = run_command_in_pty(
            "echo",
            &["hi".to_string()],
            &workspace(),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(output.exit_code, 0);
        assert!(output.output.contains("hi"), "output: {:?}", output.output);
        assert!(!output.cwd.is_empty());
    }

    #[test]
    fn one_shot_records_nonzero_exit() {
        let (program, args): (&str, Vec<String>) = if cfg!(windows) {
            ("cmd", vec!["/C".into(), "exit 3".into()])
        } else {
            ("sh", vec!["-c".into(), "exit 3".into()])
        };
        let output =
            run_command_in_pty(program, &args, &workspace(), Duration::from_secs(10)).unwrap();
        assert_eq!(output.exit_code, 3);
    }

    #[test]
    fn block_store_sequences_and_status() {
        let mut store = BlockStore::new();
        store.push("echo hi", "/tmp", Some(0), "hi\n".to_string());
        let seq = store.begin("sleep 1", "/tmp");
        assert_eq!(seq, 2);
        assert_eq!(store.len(), 2);
        assert_eq!(store.list()[0].status_label(), "exit 0");
        assert_eq!(store.last().unwrap().status_label(), "running");
        assert_eq!(store.last().unwrap().exit_code, None);
        store.seal_open("pending output".into());
        assert_eq!(store.last().unwrap().status_label(), "done");
        let seq = store.begin("false", "/tmp");
        assert_eq!(seq, 3);
        assert!(store.finish_open(Some(1), "failed\n".into()));
        assert_eq!(store.last().unwrap().status_label(), "exit 1");
        assert_eq!(store.last().unwrap().output_tail, "failed\n");
    }

    #[test]
    fn shell_marker_parser_handles_split_bel_and_st_records() {
        let mut parser = ShellMarkerParser::default();
        assert!(parser.push(b"noise\x1b]133;D;exit=").is_empty());
        assert_eq!(
            parser.push(b"7\x07\x1b]7;file:///tmp/with%20space\x1b\\"),
            vec![
                ShellMarker::CommandEnd(Some(7)),
                ShellMarker::Cwd("/tmp/with space".into()),
            ]
        );
    }

    #[test]
    fn shell_marker_parser_accepts_633_command_text_and_exit_fields() {
        let mut parser = ShellMarkerParser::default();
        assert_eq!(
            parser.push(b"\x1b]633;E;printf READY\x07\x1b]633;D;exit_code=0;duration=2\x07"),
            vec![
                ShellMarker::CommandText("printf READY".into()),
                ShellMarker::CommandEnd(Some(0)),
            ]
        );
    }

    #[test]
    fn pty_session_echoes_and_resizes() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut session = PtySession::spawn(dir.path(), 24, 80).unwrap();
        session.write(b"echo pty-probe-123\n").unwrap();
        let mut seen = String::new();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(50));
            let chunk = session.drain();
            if !chunk.is_empty() {
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
            if seen.contains("pty-probe-123") {
                break;
            }
        }
        assert!(
            seen.contains("pty-probe-123"),
            "pty did not echo marker: {seen:?}"
        );
        session.resize(30, 100).unwrap();
        assert_eq!(session.size(), (30, 100));
        let seq = session.begin_block("echo pty-probe-123");
        assert_eq!(seq, 1);
        assert_eq!(session.blocks.len(), 1);
        assert!(!session.screen_text().is_empty());
        session.kill();
    }

    #[test]
    fn pty_session_shell_integration_records_exit_and_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        let mut session = PtySession::spawn(dir.path(), 24, 80).unwrap();

        // Do not submit until the initial prompt and its cwd marker have
        // arrived; otherwise the initial prompt can race the first block.
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            session.drain();
            if session.screen_text().contains('%') || session.screen_text().contains('$') {
                break;
            }
        }

        session.begin_block("printf marker");
        session.write(b"printf marker\n").unwrap();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(20));
            session.drain();
            if session
                .blocks
                .last()
                .is_some_and(|block| block.ended.is_some())
            {
                break;
            }
        }
        let block = session.blocks.last().unwrap();
        assert_eq!(block.exit_code, Some(0), "block: {block:?}");
        assert!(block.output_tail.contains("marker"), "block: {block:?}");

        session.begin_block("cd child");
        session.write(b"cd child\n").unwrap();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(20));
            session.drain();
            if session
                .blocks
                .last()
                .is_some_and(|block| block.ended.is_some())
            {
                break;
            }
        }
        assert_eq!(session.blocks.last().unwrap().exit_code, Some(0));
        assert_eq!(
            std::fs::canonicalize(&session.cwd).unwrap(),
            std::fs::canonicalize(&child_dir).unwrap()
        );

        session.begin_block("false");
        session.write(b"false\n").unwrap();
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(20));
            session.drain();
            if session
                .blocks
                .last()
                .is_some_and(|block| block.ended.is_some())
            {
                break;
            }
        }
        assert_eq!(session.blocks.last().unwrap().exit_code, Some(1));
        session.kill();
    }

    #[test]
    fn vt100_parses_colored_output() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[31mred\x1b[0m plain");
        let contents = parser.screen().contents();
        assert!(contents.contains("red"));
        assert!(contents.contains("plain"));
    }
}
