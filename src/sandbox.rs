//! Native subprocess execution boundary.
//!
//! The wrapper selection is explicit and inspectable. When no external
//! sandbox is installed, r105 still applies a hard wall-clock timeout,
//! bounded output, a sanitized environment, and workspace-only working
//! directory. The doctor command reports the active posture.

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

const MAX_PROCESS_OUTPUT: usize = 256 * 1024;

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

    pub fn timeout_seconds(&self) -> u64 {
        self.timeout.as_secs()
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
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {program}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("subprocess stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("subprocess stderr unavailable"))?;
        let wait = wait_for_output(&mut child, stdout, stderr);
        tokio::pin!(wait);
        tokio::select! {
            _ = cancellation.cancelled() => {
                bail!("process cancelled");
            }
            result = timeout(self.timeout, &mut wait) => {
                match result {
                    Ok(result) => Ok(result?),
                    Err(_) => bail!("execution timed out ({}s)", self.timeout.as_secs()),
                }
            }
        }
    }

    /// Run a subprocess with one bounded request written to stdin.
    ///
    /// This is used by the optional Python compatibility bridge and keeps its
    /// transport inside the same sandbox, timeout, cancellation, and
    /// environment boundary as native executable tools.
    pub async fn run_with_input(
        &self,
        program: &str,
        arguments: &[String],
        workspace: &Path,
        allow_network: bool,
        cancellation: &CancellationToken,
        input: &[u8],
    ) -> Result<ProcessOutput> {
        let mut command = self.command(program, arguments, workspace, allow_network)?;
        command
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", workspace)
            .env("R105_SANDBOX", self.selected_name());
        command.kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {program}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            tokio::select! {
                _ = cancellation.cancelled() => bail!("process cancelled"),
                result = timeout(self.timeout, stdin.write_all(input)) => {
                    result.context("writing subprocess input")??;
                }
            }
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("subprocess stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("subprocess stderr unavailable"))?;
        let wait = wait_for_output(&mut child, stdout, stderr);
        tokio::pin!(wait);
        tokio::select! {
            _ = cancellation.cancelled() => bail!("process cancelled"),
            result = timeout(self.timeout, &mut wait) => {
                match result {
                    Ok(result) => Ok(result?),
                    Err(_) => bail!("execution timed out ({}s)", self.timeout.as_secs()),
                }
            }
        }
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

struct BoundedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<BoundedOutput> {
    let mut bytes = Vec::with_capacity(MAX_PROCESS_OUTPUT.min(8192));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let remaining = MAX_PROCESS_OUTPUT.saturating_sub(bytes.len());
        if remaining > 0 {
            bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        if read > remaining {
            truncated = true;
        }
    }
    Ok(BoundedOutput { bytes, truncated })
}

async fn wait_for_output(
    child: &mut Child,
    stdout: impl AsyncRead + Unpin,
    stderr: impl AsyncRead + Unpin,
) -> Result<ProcessOutput> {
    let (stdout, stderr, status) =
        tokio::join!(read_bounded(stdout), read_bounded(stderr), child.wait());
    let stdout = stdout.context("reading subprocess stdout")?;
    let stderr = stderr.context("reading subprocess stderr")?;
    let status = status.context("waiting for subprocess")?;
    Ok(ProcessOutput {
        status: status.code(),
        stdout: output_text(stdout),
        stderr: output_text(stderr),
    })
}

fn output_text(output: BoundedOutput) -> String {
    let mut text = String::from_utf8_lossy(&output.bytes).into_owned();
    if output.truncated {
        text.push_str("\n[output truncated by r105 sandbox]");
    }
    text
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

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_output_is_bounded_while_reading() {
        let directory = tempfile::tempdir().unwrap();
        let sandbox = Sandbox::detect("none", None, 10);
        let output = sandbox
            .run(
                "sh",
                &["-c".into(), "head -c 300000 /dev/zero".into()],
                directory.path(),
                false,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(output.stdout.len() < 300_000);
        assert!(output.stdout.contains("output truncated by r105 sandbox"));
    }
}
