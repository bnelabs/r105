//! Project instruction chain: global file plus workspace files.
//!
//! Files are small Markdown documents loaded fresh for every request so
//! edits apply without a restart. Each file caps at 32 KiB; missing files
//! are skipped silently.

use std::path::{Path, PathBuf};

pub const MAX_INSTRUCTION_BYTES: u64 = 32 * 1024;

/// Ordered instruction sources: global first, workspace files after, so
/// the most specific file lands closest to the history.
pub fn sources(workspace: &Path, config_dir: &Path) -> Vec<PathBuf> {
    vec![
        config_dir.join("AGENTS.md"),
        workspace.join("AGENTS.md"),
        workspace.join("AGENTS.override.md"),
        workspace.join(".r105").join("AGENTS.md"),
    ]
}

/// Load available instruction files, capped and labeled by source.
pub fn load(workspace: &Path, config_dir: &Path) -> Vec<(String, String)> {
    let mut loaded = Vec::new();
    for path in sources(workspace, config_dir) {
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_INSTRUCTION_BYTES || !metadata.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                continue;
            }
            let capped: String = trimmed
                .chars()
                .take(MAX_INSTRUCTION_BYTES as usize)
                .collect();
            loaded.push((path.display().to_string(), capped));
        }
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_orders_global_before_workspace() {
        let workspace = Path::new("/work/project");
        let config = Path::new("/home/user/.config/r105");
        let ordered = sources(workspace, config);
        assert_eq!(ordered[0], config.join("AGENTS.md"));
        assert!(ordered[1].starts_with(workspace));
    }

    #[test]
    fn missing_files_are_skipped() {
        let workspace = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        assert!(load(workspace.path(), config.path()).is_empty());
        std::fs::write(workspace.path().join("AGENTS.md"), "Be concise.").unwrap();
        let loaded = load(workspace.path(), config.path());
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].1.contains("concise"));
    }
}
