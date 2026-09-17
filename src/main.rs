#![forbid(unsafe_code)]
// The config-schema `json!` literal holds one entry per key; keep headroom.
#![recursion_limit = "256"]

mod app;
mod approve;
mod assistant;
mod backend;
mod command;
mod config;
mod custom;
mod edit;
mod export;
mod instructions;
mod mcp;
mod model;
mod plugin;
mod provider;
mod sandbox;
mod security;
mod session;
mod sse;
mod suggest;
mod terminal;
mod tool;
mod ui;
mod window;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::{Config, ConfigPaths};
use model::ChatState;
use provider::resolve_connection;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn launched_from_app_bundle() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    std::env::current_exe().ok().is_some_and(|path| {
        path.parent()
            .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "MacOS"))
            && path.ancestors().nth(3).is_some_and(|bundle| {
                bundle
                    .extension()
                    .is_some_and(|extension| extension == "app")
            })
    })
}

#[derive(Debug, Parser)]
#[command(
    name = "r105",
    version = VERSION,
    about = "Beyond the prompt. Local-first AI harness for OpenAI-compatible backends."
)]
struct Cli {
    /// OpenAI-compatible API base URL.
    #[arg(long)]
    url: Option<String>,
    /// Workspace for generated files.
    #[arg(long)]
    workspace: Option<std::path::PathBuf>,
    /// Model name to use for interactive requests.
    #[arg(long)]
    model: Option<String>,
    /// Backend type: direct or router.
    #[arg(long, value_parser = ["direct", "router"])]
    backend: Option<String>,
    /// Provider preset id.
    #[arg(long)]
    provider: Option<String>,
    /// Router profile.
    #[arg(long)]
    profile: Option<String>,
    /// Quality hint for router requests.
    #[arg(long)]
    quality: Option<String>,
    /// Maximum completion tokens.
    #[arg(long)]
    max_tokens: Option<u32>,
    /// Request JSON object responses.
    #[arg(long)]
    json: bool,
    /// Load a saved session on startup.
    #[arg(long)]
    session: Option<String>,
    /// Set the initial UI theme.
    #[arg(long)]
    theme: Option<String>,
    /// Directory containing Markdown skills.
    #[arg(long)]
    skills_dir: Option<std::path::PathBuf>,
    /// Directory containing native Rust plugin manifests.
    #[arg(long)]
    plugins_dir: Option<std::path::PathBuf>,
    /// Override the backend request timeout in seconds.
    #[arg(long)]
    timeout: Option<u64>,
    /// Select full access for this UI run without prompting.
    #[arg(long)]
    yes: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send one prompt and exit.
    Send { message: Vec<String> },
    /// Start r105 in the current terminal (the default).
    #[command(name = "run", alias = "harness")]
    Harness,
    /// Check the selected backend health.
    Health,
    /// Diagnose config, sandbox, backend, and workspace.
    Doctor,
    /// Print router profiles.
    Profiles {
        #[arg(long)]
        raw: bool,
    },
    /// Print or write the config schema.
    ConfigSchema {
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Run a command inside the sandbox boundary (dry-run tester).
    Sandbox {
        /// Sandbox backend to probe (default auto).
        #[arg(long)]
        backend: Option<String>,
        /// Command to run; empty prints the selected backend.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Open the PTY shell with block tracking.
    /// With a command, runs it once in a PTY and prints the block.
    Terminal {
        /// Command to run once in a PTY; empty opens the interactive shell.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Open the native r105 terminal and AI window.
    Window {
        /// Render this many frames then exit (smoke test).
        #[arg(long)]
        smoke: Option<u64>,
        /// Seed composer, AI panel, and approval bar for headless runs.
        #[arg(long)]
        smoke_chrome: bool,
        /// Save the final smoke frame as a PPM image for visual inspection.
        #[arg(long, requires = "smoke")]
        smoke_snapshot: Option<std::path::PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    if cli.command.is_none() && launched_from_app_bundle() {
        cli.command = Some(Command::Window {
            smoke: None,
            smoke_chrome: false,
            smoke_snapshot: None,
        });
    }
    let paths = ConfigPaths::discover();
    // The alternate-screen TUI owns every terminal cell: any stderr
    // write mid-run (a tracing warn, today from MCP) scribbles rows
    // ratatui never repaints, stranding "limbo" text. TUI runs log to
    // a file; headless subcommands keep stderr.
    let tui = matches!(
        &cli.command,
        None | Some(Command::Harness) | Some(Command::Terminal { .. })
    );
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "r105=warn".into());
    if tui {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .with_writer(tui_writer(&paths.config_dir.join("r105.log")))
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .init();
    }

    if let Some(Command::ConfigSchema { output }) = &cli.command {
        let schema = Config::schema();
        let rendered = serde_json::to_string_pretty(&schema)? + "\n";
        if let Some(path) = output {
            std::fs::write(path, rendered)?;
            println!("wrote config schema to {}", path.display());
        } else {
            print!("{rendered}");
        }
        return Ok(());
    }

    let mut config = Config::load(&paths)?;
    if let Some(plugins_dir) = cli.plugins_dir.clone() {
        config.plugins_dir = plugins_dir;
    }
    let connection = resolve_connection(
        cli.provider.as_deref().or(config.provider.as_deref()),
        cli.backend.as_deref().or(config.backend.as_deref()),
        cli.url.as_deref().or(config.url.as_deref()),
    );
    let workspace = cli
        .workspace
        .or(config.workspace.clone())
        .unwrap_or_else(|| paths.home.join("r105-workspace"));
    std::fs::create_dir_all(&workspace)?;

    let mut state = ChatState::from_config(&config, workspace.clone());
    state.config_dir = paths.config_dir.clone();
    if let Some(model) = cli.model {
        state.model = model;
    } else if let Some(model) = std::env::var_os("R105_MODEL") {
        state.model = model.to_string_lossy().into_owned();
    }
    if let Some(profile) = cli.profile {
        state.profile = Some(profile);
    }
    if let Some(quality) = cli.quality {
        state.quality = Some(quality);
    }
    if let Some(max_tokens) = cli.max_tokens {
        state.max_tokens = Some(max_tokens);
    }
    if cli.json {
        state.json_mode = true;
    }
    if let Some(theme) = cli.theme {
        state.theme = theme;
    }
    if let Some(skills_dir) = cli.skills_dir {
        state.skills_dir = skills_dir;
    }
    if cli.yes {
        state.permission_posture = "full-access".into();
    }

    if let Some(name) = cli.session.as_deref() {
        match session::load(&paths, name, &mut state) {
            Ok(count) => eprintln!("loaded session '{name}': {count} messages restored"),
            Err(error)
                if error
                    .root_cause()
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|cause| cause.kind() == std::io::ErrorKind::NotFound) =>
            {
                eprintln!("warning: session not found: {name}")
            }
            Err(error) => eprintln!("warning: failed to load session: {error}"),
        }
    }

    let backend = backend::Backend::new(connection, cli.timeout.unwrap_or(config.timeout_seconds))?;
    config.apply_runtime_connection(backend.connection());

    match cli.command.unwrap_or(Command::Harness) {
        Command::Send { message } => {
            let prompt = message.join(" ");
            let result = backend
                .chat(
                    &state,
                    &prompt,
                    &tool::definitions_from(&config.plugins_dir),
                )
                .await?;
            println!("{}", result.content);
            eprintln!("[wall={:.2}s]", result.wall_seconds);
        }
        Command::Harness => ui::run(backend, state, paths, config).await?,
        Command::Terminal { command } => {
            if command.is_empty() {
                terminal::run_interactive(&workspace)?;
                return Ok(());
            }
            let (program, args) = command.split_first().unwrap();
            let args: Vec<String> = args.to_vec();
            let started = std::time::Instant::now();
            let output = terminal::run_command_in_pty(
                program,
                &args,
                &workspace,
                std::time::Duration::from_secs(config.timeout_seconds.max(1)),
            )?;
            let wall = started.elapsed().as_secs_f64();
            let mut blocks = terminal::BlockStore::new();
            let block = blocks.push(
                &command.join(" "),
                &output.cwd,
                Some(output.exit_code),
                output.output.clone(),
            );
            println!("$ {}", block.command);
            println!("cwd: {}", block.cwd);
            println!("exit: {}", output.exit_code);
            println!("wall: {wall:.2}s");
            if !block.output_tail.is_empty() {
                println!("--- output ---\n{}", block.output_tail);
            }
        }
        Command::Window {
            smoke,
            smoke_chrome,
            smoke_snapshot,
        } => {
            // The window owns the main thread (winit) and spawns
            // assistant tasks onto the runtime workers.
            let mut parts = assistant::AssistantParts::from_config(&config);
            if smoke.is_none() {
                let name = cli
                    .session
                    .clone()
                    .unwrap_or_else(|| format!("window-{}", uuid::Uuid::new_v4().simple()));
                if cli.session.is_none() {
                    eprintln!("Window AI session: {name} (resume with --session {name} window)");
                }
                parts.persistence = Some((paths.clone(), name));
            }
            let report = window::run_window(window::WindowOptions {
                workspace: workspace.clone(),
                smoke_frames: smoke,
                smoke_chrome,
                smoke_snapshot,
                backend: backend.clone(),
                state: state.clone(),
                parts,
            })?;
            eprintln!(
                "window: frames={} screen_bytes={} blocks={}",
                report.frames, report.screen_bytes, report.blocks
            );
        }
        Command::Sandbox {
            backend: requested,
            command,
        } => {
            use tokio_util::sync::CancellationToken;
            let sandbox = sandbox::Sandbox::detect(
                requested.as_deref().unwrap_or("auto"),
                config.docker_image.clone(),
                config.timeout_seconds,
            );
            if command.is_empty() {
                println!("sandbox backend: {}", sandbox.selected_name());
                println!("available: {}", sandbox::available_backends().join(", "));
                return Ok(());
            }
            let (program, args) = command.split_first().unwrap();
            let args: Vec<String> = args.to_vec();
            let output = sandbox
                .run(program, &args, &workspace, false, &CancellationToken::new())
                .await?;
            println!("backend: {}", sandbox.selected_name());
            println!("status: {:?}", output.status);
            if !output.stdout.is_empty() {
                println!("--- stdout ---\n{}", output.stdout);
            }
            if !output.stderr.is_empty() {
                println!("--- stderr ---\n{}", output.stderr);
            }
        }
        Command::Health => println!(
            "{}",
            serde_json::to_string_pretty(&backend.health().await?)?
        ),
        Command::Doctor => app::doctor(&backend, &state, &paths).await?,
        Command::Profiles { raw } => {
            let payload = backend.profiles().await?;
            if raw {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if let Some(profiles) = payload
                .get("profiles")
                .and_then(serde_json::Value::as_object)
            {
                for (name, value) in profiles {
                    println!(
                        "{name}: max_tokens={} reasoning={}",
                        value
                            .get("max_tokens")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0),
                        value
                            .get("reasoning")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown")
                    );
                }
            }
        }
        Command::ConfigSchema { .. } => unreachable!(),
    }
    Ok(())
}

/// Append-only log writer for TUI runs. An unopenable file degrades to
/// dropping logs — never to stderr, which would corrupt the screen.
fn tui_writer(path: &std::path::Path) -> tracing_subscriber::fmt::writer::BoxMakeWriter {
    use tracing_subscriber::fmt::writer::BoxMakeWriter;

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(file) => BoxMakeWriter::new(FileMakeWriter {
            file: std::sync::Arc::new(std::sync::Mutex::new(file)),
        }),
        Err(_) => BoxMakeWriter::new(std::io::sink),
    }
}

#[derive(Clone)]
struct FileMakeWriter {
    file: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
}

struct FileWriterGuard<'a> {
    guard: std::sync::MutexGuard<'a, std::fs::File>,
}

impl std::io::Write for FileWriterGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.guard.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.guard.flush()
    }
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for FileMakeWriter {
    type Writer = FileWriterGuard<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        FileWriterGuard {
            guard: self
                .file
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt::writer::MakeWriter as _;

    #[test]
    fn tui_log_writer_appends_to_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("r105.log");
        let writer = tui_writer(&path);
        writer.make_writer().write_all(b"hello log\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello log\n");
        // An unopenable path degrades to a silent sink, never panics.
        let sink = tui_writer(std::path::Path::new("/proc/nowhere/r105.log"));
        sink.make_writer().write_all(b"dropped\n").unwrap();
    }
}
