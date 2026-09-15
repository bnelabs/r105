//! Shell-history completion cascade: history frequency first, path
//! top-hit second, no weights anywhere. Both layers resolve in
//! microseconds, so the ghost tick stays synchronous — the debounce,
//! dim render, and Tab/Esc shell from the sidecar era are unchanged,
//! only the source got smaller.

use std::collections::{HashMap, VecDeque};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Whether the composer text qualifies for ghost completion; returns
/// the model prefix (`!` and `/sh ` markers strip to the raw command).
/// Single-line shell drafts only.
pub fn ghost_prefix(input: &str) -> Option<String> {
    if input.len() < 3 {
        return None;
    }
    if let Some(rest) = input.strip_prefix("/sh ") {
        return (!rest.trim().is_empty()).then(|| rest.to_string());
    }
    if let Some(rest) = input.strip_prefix('!') {
        return (!rest.trim().is_empty()).then(|| rest.to_string());
    }
    None
}

/// One executed shell line with the workspace it ran in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub cmd: String,
    pub cwd: String,
}

/// Capped oldest→newest ring of executed shell lines. Duplicates stay:
/// repetition is the frequency signal.
#[derive(Debug, Clone, Default)]
pub struct ShellHistory {
    entries: VecDeque<HistoryEntry>,
    max: usize,
}

impl ShellHistory {
    pub fn new(max: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max: max.max(1),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Record a run; blanks never enter, overflow drops the oldest.
    pub fn record(&mut self, cmd: &str, cwd: &str) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return;
        }
        self.entries.push_back(HistoryEntry {
            cmd: cmd.to_string(),
            cwd: cwd.to_string(),
        });
        while self.entries.len() > self.max {
            self.entries.pop_front();
        }
    }

    /// Best continuation suffix for `prefix`, or `None`. Score per
    /// distinct command: `10×runs + 25×same-dir runs + newest_index`
    /// (later index = more recent). Prefix match is case-sensitive:
    /// shells are.
    pub fn suggest(&self, prefix: &str, cwd: &str) -> Option<String> {
        if prefix.is_empty() {
            return None;
        }
        let mut counts: HashMap<&str, (usize, usize, usize)> = HashMap::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if !entry.cmd.starts_with(prefix) || entry.cmd.len() == prefix.len() {
                continue;
            }
            let stats = counts.entry(entry.cmd.as_str()).or_insert((0, 0, 0));
            stats.0 += 1;
            if entry.cwd == cwd {
                stats.1 += 1;
            }
            stats.2 = index;
        }
        let (best, _) = counts
            .into_iter()
            .max_by_key(|(_, (count, cwd_hits, newest))| 10 * count + 25 * cwd_hits + newest)?;
        best.strip_prefix(prefix).map(str::to_string)
    }

    pub fn load(path: &std::path::Path, max: usize) -> Self {
        let mut history = Self::new(max);
        let Ok(raw) = std::fs::read_to_string(path) else {
            return history;
        };
        if let Ok(entries) = serde_json::from_str::<Vec<HistoryEntry>>(&raw) {
            for entry in entries.into_iter().take(history.max) {
                history.entries.push_back(entry);
            }
        }
        history
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let raw = serde_json::to_string(&self.entries.clone().into_iter().collect::<Vec<_>>())?;
        std::fs::write(path, raw).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Path top-hit for the last whitespace-separated token: `src/ma`
/// completes from the directory listing, scored on the basename.
/// `score` mirrors the palette file scorer (exact > prefix > fuzzy).
pub fn path_guess(
    prefix: &str,
    cwd: &std::path::Path,
    score: impl Fn(&str, &str) -> Option<i32>,
) -> Option<String> {
    const MAX_ENTRIES: usize = 200;
    let fragment = prefix.split_whitespace().next_back()?;
    if fragment.len() < 2 || (!fragment.contains('/') && !fragment.starts_with('.')) {
        return None;
    }
    let (dir_part, file_part) = match fragment.rsplit_once('/') {
        Some((dir, file)) => (dir, file),
        None => (".", fragment),
    };
    if file_part.is_empty() {
        return None;
    }
    let dir = cwd.join(dir_part);
    let listing = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<(i32, String)> = None;
    for (count, entry) in listing.enumerate() {
        if count >= MAX_ENTRIES {
            break;
        }
        let name = entry.ok()?.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && !file_part.starts_with('.') {
            continue;
        }
        if let Some(rank) = score(&name, file_part) {
            let better = best.as_ref().is_none_or(|(top, _)| rank > *top);
            if better {
                best = Some((rank, name));
            }
        }
    }
    let (_, name) = best?;
    name.strip_prefix(file_part).map(str::to_string)
}

/// The cascade: history frequency, then command-name (builtins + PATH
/// executables), then path top-hit. Returns the suffix to render dimmed
/// after the composer text.
pub fn suggest(
    input: &str,
    history: &ShellHistory,
    cwd: &std::path::Path,
    bins: &[String],
    score: impl Fn(&str, &str) -> Option<i32>,
) -> Option<String> {
    let prefix = ghost_prefix(input)?;
    if let Some(suffix) = history.suggest(&prefix, &cwd.to_string_lossy()) {
        return Some(suffix);
    }
    if let Some(suffix) = command_guess(&prefix, bins) {
        return Some(suffix);
    }
    path_guess(&prefix, cwd, score)
}

/// POSIX-ish shell builtins worth a ghost (the rest come from PATH).
pub const SHELL_BUILTINS: &[&str] = &[
    "alias", "bg", "break", "cd", "continue", "echo", "eval", "exec", "exit", "export", "fg",
    "history", "jobs", "kill", "local", "printf", "pwd", "read", "readonly", "return", "set",
    "shift", "source", "test", "time", "type", "ulimit", "umask", "unalias", "unset", "wait",
];

/// Best continuation for the first word of a shell line: builtins plus
/// PATH executables. Append-only ghost, so only prefix matches qualify
/// (the `contains` tier of the file scorer cannot produce a suffix);
/// shorter names win, ties keep the builtin (listed first).
pub fn command_guess(prefix: &str, bins: &[String]) -> Option<String> {
    if prefix.is_empty() || prefix.chars().any(char::is_whitespace) {
        return None;
    }
    let mut best: Option<(i32, &str)> = None;
    for name in SHELL_BUILTINS
        .iter()
        .copied()
        .chain(bins.iter().map(String::as_str))
    {
        if name.len() <= prefix.len() || !name.starts_with(prefix) {
            continue;
        }
        let rank = 10 - name.len().min(9) as i32;
        if best.as_ref().is_none_or(|(top, _)| rank > *top) {
            best = Some((rank, name));
        }
    }
    let name = best?.1;
    name.strip_prefix(prefix).map(str::to_string)
}

/// PATH executables for the command layer, from the live environment.
pub fn scan_path_bins() -> Vec<String> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    scan_bins(std::env::split_paths(&path))
}

/// Sorted, deduped file names across `dirs` (regular files only, capped
/// so a pathological PATH cannot stall a keystroke).
pub fn scan_bins(dirs: impl IntoIterator<Item = std::path::PathBuf>) -> Vec<String> {
    const MAX_BINS: usize = 2000;
    let mut bins = std::collections::BTreeSet::new();
    for dir in dirs {
        let Ok(listing) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in listing.flatten() {
            if bins.len() >= MAX_BINS {
                break;
            }
            if !entry.metadata().is_ok_and(|meta| meta.is_file()) {
                continue;
            }
            if let Ok(name) = entry.file_name().into_string()
                && !name.is_empty()
            {
                bins.insert(name);
            }
        }
        if bins.len() >= MAX_BINS {
            break;
        }
    }
    bins.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(cmds: &[(&str, &str)]) -> ShellHistory {
        let mut history = ShellHistory::new(500);
        for (cmd, cwd) in cmds {
            history.record(cmd, cwd);
        }
        history
    }

    /// Frequency wins: the workhorse command beats one-offs.
    #[test]
    fn suggest_history_prefers_frequent() {
        let history = history(&[
            ("git status", "/repo"),
            ("git stash", "/repo"),
            ("git status", "/repo"),
            ("git status", "/repo"),
        ]);
        assert_eq!(history.suggest("git sta", "/repo"), Some("tus".to_string()));
    }

    /// Same-directory runs outrank equally frequent ones elsewhere.
    #[test]
    fn suggest_cwd_boosts_same_dir() {
        let history = history(&[
            ("docker ps -a", "/elsewhere"),
            ("docker ps -a", "/elsewhere"),
            ("docker ps --format '{{.ID}}'", "/repo"),
        ]);
        assert_eq!(
            history.suggest("docker ps", "/repo"),
            Some(" --format '{{.ID}}'".to_string())
        );
    }

    /// Ties break toward the most recent run.
    #[test]
    fn suggest_recency_breaks_ties() {
        let history = history(&[("make old", "/repo"), ("make new", "/repo")]);
        assert_eq!(history.suggest("make ", "/repo"), Some("new".to_string()));
    }

    /// Prefix is required and case-sensitive; exact input never echoes.
    #[test]
    fn suggest_prefix_guards() {
        let history = history(&[("Git status", "/repo"), ("git status", "/repo")]);
        assert_eq!(history.suggest("git sta", "/repo"), Some("tus".to_string()));
        assert_eq!(history.suggest("git status", "/repo"), None);
        assert_eq!(history.suggest("", "/repo"), None);
    }

    /// The ring drops the oldest runs past capacity.
    #[test]
    fn suggest_history_cap_truncates() {
        let mut history = ShellHistory::new(2);
        history.record("first", "/repo");
        history.record("second", "/repo");
        history.record("third", "/repo");
        assert_eq!(history.len(), 2);
        assert_eq!(history.suggest("fi", "/repo"), None);
        assert_eq!(history.suggest("thi", "/repo"), Some("rd".to_string()));
    }

    /// Save/load round-trips; a corrupt file starts empty, not broken.
    #[test]
    fn suggest_history_persists() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("shell_history.json");
        let history = history(&[("git status", "/repo")]);
        history.save(&path).unwrap();
        let loaded = ShellHistory::load(&path, 500);
        assert_eq!(loaded.suggest("git sta", "/repo"), Some("tus".to_string()));
        std::fs::write(&path, "not json{{{").unwrap();
        assert_eq!(ShellHistory::load(&path, 500).len(), 0);
    }

    /// Basename completion inside a real directory listing.
    #[test]
    fn suggest_path_completes_basename() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(directory.path().join("src/modem.rs"), "").unwrap();
        let score = |relative: &str, query: &str| {
            if relative == query {
                Some(3)
            } else if relative.starts_with(query) {
                Some(2)
            } else if relative.contains(query) {
                Some(1)
            } else {
                None
            }
        };
        assert_eq!(
            path_guess("cat src/mai", directory.path(), score),
            Some("n.rs".to_string())
        );
        assert_eq!(path_guess("cat README", directory.path(), score), None);
        assert_eq!(path_guess("ls", directory.path(), score), None);
    }

    /// Trigger shape: `!` and `/sh ` shell lines of length ≥ 3 only.
    #[test]
    fn ghost_trigger_shape() {
        assert_eq!(ghost_prefix("!git sta"), Some("git sta".to_string()));
        assert_eq!(ghost_prefix("/sh git sta"), Some("git sta".to_string()));
        assert_eq!(ghost_prefix("!ls"), Some("ls".to_string()));
        assert_eq!(ghost_prefix("!l"), None);
        assert_eq!(ghost_prefix("/sh"), None);
        assert_eq!(ghost_prefix("hello world"), None);
        assert_eq!(ghost_prefix(""), None);
    }

    /// Command layer: builtins and PATH bins complete the first token,
    /// argument territory and exact matches do not.
    #[test]
    fn command_guess_covers_builtins_and_bins() {
        let bins = vec!["cargo".to_string(), "git".to_string()];
        assert_eq!(command_guess("ec", &[]), Some("ho".to_string()));
        assert_eq!(command_guess("gi", &bins), Some("t".to_string()));
        assert_eq!(command_guess("car", &bins), Some("go".to_string()));
        assert_eq!(command_guess("git sta", &bins), None);
        assert_eq!(command_guess("git", &bins), None);
        assert_eq!(command_guess("", &bins), None);
    }

    /// PATH executables beat path guesses; history beats both; the path
    /// layer still handles tokens no binary matches.
    #[test]
    fn suggest_cascade_order_prefers_history_then_bins() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(directory.path().join("cargo.toml"), "").unwrap();
        let score = |relative: &str, query: &str| relative.starts_with(query).then_some(100);
        let bins = vec!["cargo".to_string()];
        let empty = ShellHistory::new(10);

        // History frequency wins over the command layer.
        let mut history = ShellHistory::new(10);
        history.record("cargo build", "/repo");
        assert_eq!(
            suggest("!car", &history, directory.path(), &bins, score),
            Some("go build".to_string())
        );
        // No history for the prefix: the binary completes `car` → `go`,
        // not the `cargo.toml` path top-hit.
        assert_eq!(
            suggest("!car", &empty, directory.path(), &bins, score),
            Some("go".to_string())
        );
        // No binary for the token: the path layer still serves.
        assert_eq!(
            suggest("!./car", &empty, directory.path(), &bins, score),
            Some("go.toml".to_string())
        );
    }

    /// The PATH scan collects files only, sorted and deduped.
    #[test]
    fn scan_bins_sorts_and_skips_directories() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        std::fs::write(first.path().join("cargo"), "").unwrap();
        std::fs::write(first.path().join("git"), "").unwrap();
        std::fs::create_dir(first.path().join("nested")).unwrap();
        std::fs::write(second.path().join("git"), "").unwrap();
        std::fs::write(second.path().join("aria2c"), "").unwrap();
        let bins = scan_bins(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);
        assert_eq!(bins, vec!["aria2c", "cargo", "git"]);
    }
}
