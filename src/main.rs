#![forbid(unsafe_code)]

mod app;
mod backend;
mod command;
mod config;
mod custom;
mod export;
mod mcp;
mod model;
mod plugin;
mod provider;
mod python_bridge;
mod sandbox;
mod security;
mod session;
mod sse;
mod tool;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::{Config, ConfigPaths};
use model::ChatState;
use provider::resolve_connection;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    /// Model name to use for chat requests.
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
    /// Compatibility flag: select full access and approve Python execution for this UI run.
    #[arg(long)]
    yes: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send one prompt and exit.
    Send { message: Vec<String> },
    /// Start the interactive native Rust TUI.
    Chat,
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
    /// Show the optional external Python compatibility bridge.
    Bridge {
        /// Override the configured bridge command for this check.
        #[arg(long)]
        command: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "r105=warn".into()),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
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

    let paths = ConfigPaths::discover();
    let mut config = Config::load(&paths)?;
    let python_approved = cli.yes || config.auto_approve_execute_python;
    if let Some(Command::Bridge { command }) = &cli.command {
        println!(
            "{}",
            python_bridge::status(
                command
                    .as_deref()
                    .or(config.python_bridge_command.as_deref())
            )
        );
        return Ok(());
    }
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

    match cli.command.unwrap_or(Command::Chat) {
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
        Command::Chat => ui::run(backend, state, paths, config, python_approved).await?,
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
        Command::Bridge { .. } => unreachable!(),
    }
    Ok(())
}
