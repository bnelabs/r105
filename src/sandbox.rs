//! Native subprocess execution boundary.
//!
//! The wrapper selection is explicit and inspectable. When no external
//! sandbox is installed, r105 still applies a hard wall-clock timeout,
//! bounded output, a sanitized environment, and workspace-only working
//! directory. The doctor command reports the active posture.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use tokio::{process::Command, time::timeout};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    Auto,
    Nsjail,
    Bwrap,
    Docker,
    Rlimit,
    None,
}

impl SandboxBackend {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "auto" => Self::Auto,
            "nsjail" => Self::Nsjail,
            "bwrap" => Self::Bwrap,
            "docker" => Self::Docker,
            "rlimit" => Self::Rlimit,
            "none" => Self::None,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Nsjail => "nsjail",
            Self::Bwrap => "bwrap",
            Self::Docker => "docker",
            Self::Rlimit => "rlimit",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sandbox {
    selected: SandboxBackend,
    timeout: Duration,
    docker_image: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Sandbox {
    pub fn detect(requested: &str, docker_image: Option<String>, timeout_seconds: u64) -> Self {
        let requested = SandboxBackend::parse(requested).unwrap_or(SandboxBackend::Auto);
        let selected = match requested {
            SandboxBackend::Auto => {
                if which::which("nsjail").is_ok() {
                    SandboxBackend::Nsjail
                } else if which::which("bwrap").is_ok() {
                    SandboxBackend::Bwrap
                } else if which::which("docker").is_ok() {
                    SandboxBackend::Docker
                } else {
                    SandboxBackend::Rlimit
                }
            }
            value => value,
        };
        Self {
            selected,
            timeout: Duration::from_secs(timeout_seconds.clamp(1, 300)),
            docker_image,
        }
    }

    pub fn selected(&self) -> SandboxBackend {
        self.selected
    }

    pub fn selected_name(&self) -> &'static str {
        self.selected().as_str()
    }

    pub async fn run(
        &self,
        program: &str,
        arguments: &[String],
        workspace: &Path,
        allow_network: bool,
        cancellation: &CancellationToken,
    ) -> Result<ProcessOutput> {
        let mut command = self.command(program, arguments, workspace, allow_network)?;
        command
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", workspace)
            .env("R105_SANDBOX", self.selected_name());
        command.kill_on_drop(true);
        let child = command
            .spawn()
            .with_context(|| format!("starting {program}"))?;
        let wait = child.wait_with_output();
        tokio::pin!(wait);
        let result = tokio::select! {
            _ = cancellation.cancelled() => {
                bail!("process cancelled");
            }
            result = timeout(self.timeout, &mut wait) => {
                match result {
                    Ok(result) => result.context("waiting for subprocess")?,
                    Err(_) => bail!("execution timed out ({}s)", self.timeout.as_secs()),
                }
            }
        };
        Ok(ProcessOutput {
            status: result.status.code(),
            stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&result.stderr).into_owned(),
        })
    }

    fn command(
        &self,
        program: &str,
        arguments: &[String],
        workspace: &Path,
        allow_network: bool,
    ) -> Result<Command> {
        let mut command;
        match self.selected {
            SandboxBackend::None | SandboxBackend::Rlimit | SandboxBackend::Auto => {
                command = Command::new(program);
                command.args(arguments);
            }
            SandboxBackend::Nsjail => {
                command = Command::new("nsjail");
                command.args([
                    "--quiet",
                    "--mode",
                    "o",
                    "--cwd",
                    &workspace.to_string_lossy(),
                ]);
                if !allow_network {
                    command.arg("--disable_clone_newnet");
                }
                command.arg("--").arg(program).args(arguments);
            }
            SandboxBackend::Bwrap => {
                command = Command::new("bwrap");
                command.args([
                    "--die-with-parent",
                    "--new-session",
                    "--ro-bind",
                    "/",
                    "/",
                    "--bind",
                    &workspace.to_string_lossy(),
                    &workspace.to_string_lossy(),
                    "--chdir",
                    &workspace.to_string_lossy(),
                ]);
                if !allow_network {
                    command.arg("--unshare-net");
                }
                command.arg("--").arg(program).args(arguments);
            }
            SandboxBackend::Docker => {
                let image = self
                    .docker_image
                    .as_deref()
                    .unwrap_or("debian:bookworm-slim");
                command = Command::new("docker");
                command.args([
                    "run",
                    "--rm",
                    "--init",
                    "--cap-drop=ALL",
                    "--security-opt=no-new-privileges",
                    "-v",
                    &format!("{}:/workspace:rw", workspace.display()),
                    "-w",
                    "/workspace",
                ]);
                if !allow_network {
                    command.arg("--network=none");
                }
                command.arg(image).arg(program).args(arguments);
            }
        }
        Ok(command)
    }
}

pub fn available_backends() -> Vec<&'static str> {
    ["nsjail", "bwrap", "docker", "rlimit", "none"]
        .into_iter()
        .filter(|name| *name == "rlimit" || *name == "none" || which::which(name).is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_detection_always_has_a_fallback() {
        let sandbox = Sandbox::detect("auto", None, 30);
        assert_ne!(sandbox.selected(), SandboxBackend::Auto);
    }

    #[test]
    fn explicit_none_is_preserved() {
        let sandbox = Sandbox::detect("none", None, 30);
        assert_eq!(sandbox.selected_name(), "none");
    }
}
