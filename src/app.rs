//! Application diagnostics and shared startup checks.

use anyhow::Result;
use serde_json::json;

use crate::{
    backend::Backend,
    config::ConfigPaths,
    model::ChatState,
    sandbox::{Sandbox, available_backends},
};

pub async fn doctor(backend: &Backend, state: &ChatState, paths: &ConfigPaths) -> Result<()> {
    let sandbox = Sandbox::detect("auto", None, 30);
    let health = backend
        .health()
        .await
        .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}));
    let workspace_writable = std::fs::metadata(&state.workspace).is_ok()
        && std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.workspace.join(".r105-doctor"))
            .map(|_| true)
            .unwrap_or(false);
    let report = json!({
        "config_file": paths.config_file,
        "workspace": state.workspace,
        "workspace_writable": workspace_writable,
        "sandbox": sandbox.selected_name(),
        "available_sandboxes": available_backends(),
        "backend": health,
        "rust_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    let _ = std::fs::remove_file(state.workspace.join(".r105-doctor"));
    Ok(())
}
