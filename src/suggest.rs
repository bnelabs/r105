//! Shell-history completion cascade: history frequency first, path
//! top-hit second, no weights anywhere. Both layers resolve in
//! microseconds, so the ghost tick stays synchronous — the debounce,
//! dim render, and Tab/Esc shell from the sidecar era are unchanged,
//! only the source got smaller.

use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Whether the composer text qualifies for ghost completion; returns
/// the model prefix (the `!` marker strips to the raw command).
/// Single-line shell drafts only. The UI resolves markers through
/// `shell_line` now; this stays as the tested marker contract.
#[cfg(test)]
pub fn ghost_prefix(input: &str) -> Option<String> {
    if input.len() < 3 {
        return None;
    }
    if let Some(rest) = input.strip_prefix('!') {
        return (!rest.trim().is_empty()).then(|| rest.to_string());
    }
    None
}

/// Bare-line shell detection: does this composer line read as a shell
/// command without any `!` marker? A fully typed command
/// word (spec, builtin, curated one-off, alias) qualifies — with or
/// without arguments. A lone partial word qualifies only for the
/// curated sets at 3+ characters, so ordinary prose never flashes
/// command ghosts (`helm` from "hel" is intended, `lsof` from "ls"
/// is not). PATH binaries are deliberately excluded from multi-word
/// detection: "write a test" must stay a prompt even though
/// /usr/bin/write exists. Questions stay prompts too.
pub fn looks_like_shell(line: &str, aliases: &[(String, String)]) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.ends_with('?') {
        return false;
    }
    if trimmed.starts_with(['/', '#', '!', '@']) {
        return false;
    }
    let tokens = split_shell_tokens(trimmed);
    if tokens.is_empty() {
        return false;
    }
    // Transparent wrappers imply shell on their own (`sudo …`).
    if WRAPPERS.contains(&tokens[0].as_str()) {
        return true;
    }
    let first = tokens[0].as_str();
    if first.starts_with("./") || first.starts_with('/') || first.starts_with("~/") {
        return true;
    }
    if crate::command::has_shell_syntax(trimmed) {
        return true;
    }
    if shell_spec(first).is_some()
        || SHELL_BUILTINS.contains(&first)
        || crate::command::SHELL_ONE_OFFS.contains(&first.to_ascii_lowercase().as_str())
        || aliases.iter().any(|(name, _)| name == first)
    {
        return true;
    }
    // A lone partial word leans shell only for curated prefixes.
    tokens.len() == 1
        && first.len() >= 3
        && (SHELL_SPECS
            .iter()
            .any(|(name, _)| name.starts_with(first) && name.len() > first.len())
            || SHELL_BUILTINS
                .iter()
                .any(|name| name.starts_with(first) && name.len() > first.len())
            || crate::command::SHELL_ONE_OFFS
                .iter()
                .any(|name| name.starts_with(first) && name.len() > first.len()))
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

    /// Newest-first full lines starting with `prefix` (strictly
    /// longer, deduped, capped): the history half of Tab menus.
    pub fn recent_matches(&self, prefix: &str, cap: usize) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for entry in self.entries.iter().rev() {
            if out.len() >= cap {
                break;
            }
            if entry.cmd.len() > prefix.len()
                && entry.cmd.starts_with(prefix)
                && seen.insert(entry.cmd.clone())
            {
                out.push(entry.cmd.clone());
            }
        }
        out
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

/// The cascade: history frequency, then context-aware argument
/// completion (subcommands, flags, branches, scripts, files), then
/// command-name (builtins + PATH executables), then path top-hit.
/// Returns the suffix to render dimmed after the composer text.
/// `line` is bare shell text — markers are resolved by the caller.
pub fn suggest_shell(
    line: &str,
    history: &ShellHistory,
    cwd: &std::path::Path,
    bins: &[String],
    ctx: &ContextCache,
    score: impl Fn(&str, &str) -> Option<i32>,
) -> Option<String> {
    if let Some(suffix) = history.suggest(line, &cwd.to_string_lossy()) {
        return Some(suffix);
    }
    if let Some(suffix) = context_suggest(line, cwd, ctx) {
        return Some(suffix);
    }
    if let Some(suffix) = command_guess(line, bins) {
        return Some(suffix);
    }
    path_guess(line, cwd, score)
}

/// Marker-gated cascade (`!`): the historical entry point, kept as the
/// tested wrapper over `suggest_shell`.
#[cfg(test)]
pub fn suggest(
    input: &str,
    history: &ShellHistory,
    cwd: &std::path::Path,
    bins: &[String],
    ctx: &ContextCache,
    score: impl Fn(&str, &str) -> Option<i32>,
) -> Option<String> {
    let prefix = ghost_prefix(input)?;
    suggest_shell(&prefix, history, cwd, bins, ctx, score)
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

/// Context-aware shell completion: per-command specs (subcommands,
/// flags) plus filesystem-derived value candidates (git refs, npm
/// scripts, make targets, ssh hosts, files). Everything resolves from
/// local files — no subprocess — so the ghost tick stays synchronous.
///
/// Ghost text appends to the composer, so this layer only completes
/// prefixes; fuzzy matching lives in the menus (palette, `@`, args).
/// Single-token lines belong to `command_guess`; empty tokens complete
/// nothing (history already covers the common `cmd ` continuation).
///
/// Split a shell line into tokens, honoring single/double quotes and
/// backslash escapes. Quote characters are consumed, so
/// `git checkout "my br` yields `["git", "checkout", "my br"]`.
pub fn split_shell_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut in_token = false;
    let mut chars = line.chars();
    while let Some(char) = chars.next() {
        if let Some(mark) = quote {
            if char == mark {
                quote = None;
            } else if char == '\\' {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            } else {
                current.push(char);
            }
            in_token = true;
        } else if char == '\'' || char == '"' {
            quote = Some(char);
            in_token = true;
        } else if char == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
            }
            in_token = true;
        } else if char.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
        } else {
            current.push(char);
            in_token = true;
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

/// Levenshtein distance over chars. Inputs are command names and file
/// stems — short — so the plain two-row DP is plenty.
pub fn edit_distance(first: &str, second: &str) -> usize {
    let first: Vec<char> = first.chars().collect();
    let second: Vec<char> = second.chars().collect();
    if first.is_empty() {
        return second.len();
    }
    if second.is_empty() {
        return first.len();
    }
    let mut prev: Vec<usize> = (0..=second.len()).collect();
    let mut next = vec![0; second.len() + 1];
    for (row, &left) in first.iter().enumerate() {
        next[0] = row + 1;
        for (col, &right) in second.iter().enumerate() {
            let cost = usize::from(left != right);
            next[col + 1] = (prev[col] + cost).min(prev[col + 1] + 1).min(next[col] + 1);
        }
        std::mem::swap(&mut prev, &mut next);
    }
    prev[second.len()]
}

/// Nearest candidate for a possibly mistyped `token`: the first of
/// `nearest_all`. See it for the ranking contract.
pub fn nearest<'candidate>(token: &str, candidates: &[&'candidate str]) -> Option<&'candidate str> {
    nearest_all(token, candidates, 1).into_iter().next()
}

/// First prefix completion strictly longer than `token`, or `None`.
/// Deterministic: candidate order is the priority (frequency-ordered
/// tables, alphabetical listings, makefile order).
pub fn best_prefix_match<'candidate>(
    candidates: impl IntoIterator<Item = &'candidate str>,
    token: &str,
) -> Option<&'candidate str> {
    if token.is_empty() {
        return None;
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.len() > token.len() && candidate.starts_with(token))
}

/// Value kind for the token after a subcommand: where candidates come
/// from. All readers are pure filesystem — no subprocess, no hangs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Branches,
    Tags,
    Remotes,
    Files,
    Dirs,
    NpmScripts,
    MakeTargets,
    SshHosts,
    K8sResources,
    /// Kubeconfig contexts (file-local, synchronous).
    KubeContexts,
    /// Cluster namespaces (daemon, background cache).
    KubeNamespaces,
    /// Pods in scope (daemon, background cache).
    KubePods,
    /// Running container names (daemon, background cache).
    DockerContainers,
    /// Local image refs (daemon, background cache).
    DockerImages,
    /// Installed unit names (unit dirs, synchronous).
    SystemdUnits,
}

/// Kubernetes resource types for `kubectl get <Tab>` and friends.
/// Static names only — live objects need the cluster API, which never
/// runs on the keystroke path.
pub const K8S_RESOURCES: &[&str] = &[
    "pods",
    "nodes",
    "services",
    "deployments",
    "replicasets",
    "statefulsets",
    "daemonsets",
    "jobs",
    "cronjobs",
    "configmaps",
    "secrets",
    "ingresses",
    "namespaces",
    "persistentvolumes",
    "persistentvolumeclaims",
    "serviceaccounts",
    "roles",
    "rolebindings",
    "clusterroles",
    "storageclasses",
    "endpoints",
    "events",
    "networkpolicies",
    "horizontalpodautoscalers",
];

/// One command's completion data: subcommands, flags, per-subcommand
/// value kinds, and first-argument kinds for commands like `ssh host`
/// or `make target` that take values instead of subcommands.
pub struct ShellSpec {
    pub subcommands: &'static [&'static str],
    pub flags: &'static [&'static str],
    pub values: &'static [(&'static str, &'static [ValueKind])],
    pub first_arg: &'static [ValueKind],
    /// Second-level subcommands (`gh issue list`, `docker container ls`).
    pub nested: &'static [(&'static str, &'static [&'static str])],
}

/// Transparent wrappers: completion looks past them at the real command.
const WRAPPERS: &[&str] = &["sudo", "doas", "env", "nice", "time", "nohup"];

pub fn shell_spec(command: &str) -> Option<&'static ShellSpec> {
    // Versioned spellings share one table.
    let command = match command {
        "python3" => "python",
        "pip3" => "pip",
        _ => command,
    };
    SHELL_SPECS
        .iter()
        .find(|spec| spec.0 == command)
        .map(|spec| spec.1)
}

/// Roughly by frequency: `nearest` returns the first prefix match,
/// so everyday subcommands outrank rare ones sharing a prefix
/// (`st` → `status`, not `stash`).
const GIT_SUBCOMMANDS: &[&str] = &[
    "status",
    "checkout",
    "branch",
    "commit",
    "log",
    "diff",
    "push",
    "pull",
    "add",
    "fetch",
    "merge",
    "clone",
    "show",
    "stash",
    "restore",
    "switch",
    "reset",
    "rebase",
    "remote",
    "tag",
    "revert",
    "cherry-pick",
    "clean",
    "mv",
    "rm",
    "grep",
    "describe",
    "init",
    "blame",
    "bisect",
    "archive",
    "am",
    "worktree",
];

const GIT_FLAGS: &[&str] = &[
    "--help",
    "--version",
    "--verbose",
    "--quiet",
    "--all",
    "--force",
    "--dry-run",
    "--porcelain",
    "--oneline",
    "--patch",
    "--cached",
    "--amend",
    "--no-verify",
    "--set-upstream",
    "--rebase",
    "--no-rebase",
    "--depth",
    "--branch",
    "--single-branch",
    "--prune",
    "--tags",
    "--follow",
    "--stat",
    "--name-only",
    "--decorate",
    "--graph",
    "--author",
    "--grep",
    "--invert-grep",
    "--before",
    "--after",
];

const GIT_VALUES: &[(&str, &[ValueKind])] = &[
    ("checkout", &[ValueKind::Branches, ValueKind::Files]),
    ("switch", &[ValueKind::Branches]),
    ("restore", &[ValueKind::Branches, ValueKind::Files]),
    ("merge", &[ValueKind::Branches, ValueKind::Tags]),
    ("rebase", &[ValueKind::Branches, ValueKind::Tags]),
    ("cherry-pick", &[ValueKind::Branches, ValueKind::Tags]),
    ("revert", &[ValueKind::Branches, ValueKind::Tags]),
    ("add", &[ValueKind::Files]),
    (
        "show",
        &[ValueKind::Branches, ValueKind::Tags, ValueKind::Files],
    ),
    (
        "log",
        &[ValueKind::Branches, ValueKind::Tags, ValueKind::Files],
    ),
    (
        "diff",
        &[ValueKind::Branches, ValueKind::Tags, ValueKind::Files],
    ),
    ("push", &[ValueKind::Remotes, ValueKind::Branches]),
    ("pull", &[ValueKind::Remotes, ValueKind::Branches]),
    ("fetch", &[ValueKind::Remotes, ValueKind::Branches]),
    ("tag", &[ValueKind::Tags]),
    ("branch", &[ValueKind::Branches]),
    ("clone", &[ValueKind::Dirs]),
];

const GIT_NESTED: &[(&str, &[&str])] = &[
    (
        "stash",
        &[
            "push", "pop", "apply", "list", "drop", "show", "clear", "create",
        ],
    ),
    (
        "remote",
        &["add", "remove", "rename", "set-url", "show", "prune"],
    ),
    (
        "worktree",
        &["add", "list", "remove", "move", "prune", "lock", "unlock"],
    ),
    (
        "submodule",
        &["add", "update", "init", "status", "foreach", "sync"],
    ),
    (
        "bisect",
        &["start", "bad", "good", "skip", "reset", "run", "log"],
    ),
];

const GH_NESTED: &[(&str, &[&str])] = &[
    (
        "issue",
        &[
            "list", "create", "view", "close", "reopen", "delete", "edit", "lock", "comment",
        ],
    ),
    (
        "pr",
        &[
            "list", "create", "view", "merge", "close", "reopen", "checkout", "diff", "review",
            "comment",
        ],
    ),
    (
        "repo",
        &[
            "clone", "create", "view", "fork", "list", "delete", "edit", "sync", "archive",
        ],
    ),
    (
        "run",
        &[
            "list", "view", "watch", "rerun", "cancel", "download", "delete",
        ],
    ),
    (
        "release",
        &[
            "list", "create", "view", "delete", "edit", "upload", "download",
        ],
    ),
    ("workflow", &["list", "view", "run", "enable", "disable"]),
    ("secret", &["list", "set", "delete"]),
    ("variable", &["list", "set", "delete", "get"]),
    ("cache", &["list", "delete"]),
    (
        "gist",
        &["list", "create", "view", "delete", "edit", "clone", "fork"],
    ),
    ("search", &["issues", "prs", "repos", "code", "commits"]),
    (
        "auth",
        &["login", "logout", "status", "refresh", "setup-git"],
    ),
    (
        "extension",
        &["list", "install", "remove", "search", "upgrade"],
    ),
];

const DOCKER_NESTED: &[(&str, &[&str])] = &[
    (
        "container",
        &[
            "ls", "run", "exec", "logs", "inspect", "start", "stop", "restart", "rm", "prune",
            "stats", "top",
        ],
    ),
    (
        "image",
        &[
            "ls", "build", "pull", "push", "tag", "rm", "inspect", "prune", "history",
        ],
    ),
    (
        "network",
        &[
            "ls",
            "create",
            "inspect",
            "rm",
            "prune",
            "connect",
            "disconnect",
        ],
    ),
    ("volume", &["ls", "create", "inspect", "rm", "prune"]),
    ("system", &["df", "prune", "info", "events"]),
    (
        "compose",
        &[
            "up", "down", "ps", "logs", "build", "exec", "restart", "pull", "config",
        ],
    ),
];

const KUBECTL_NESTED: &[(&str, &[&str])] = &[
    (
        "rollout",
        &["history", "pause", "resume", "restart", "status", "undo"],
    ),
    (
        "config",
        &[
            "get-contexts",
            "use-context",
            "current-context",
            "set-context",
            "view",
            "get-clusters",
        ],
    ),
    (
        "create",
        &[
            "deployment",
            "service",
            "configmap",
            "secret",
            "namespace",
            "job",
            "ingress",
        ],
    ),
];

static SHELL_SPECS: &[(&str, &ShellSpec)] = &[
    (
        "git",
        &ShellSpec {
            subcommands: GIT_SUBCOMMANDS,
            flags: GIT_FLAGS,
            values: GIT_VALUES,
            first_arg: &[],
            nested: GIT_NESTED,
        },
    ),
    (
        "cargo",
        &ShellSpec {
            subcommands: &[
                "add",
                "bench",
                "build",
                "check",
                "clean",
                "clippy",
                "doc",
                "fetch",
                "fix",
                "fmt",
                "init",
                "install",
                "locate-project",
                "login",
                "logout",
                "metadata",
                "new",
                "owner",
                "package",
                "publish",
                "read-manifest",
                "remove",
                "run",
                "rustc",
                "search",
                "test",
                "tree",
                "uninstall",
                "update",
                "verify-project",
                "version",
                "yank",
            ],
            flags: &[
                "--help",
                "--version",
                "--release",
                "--verbose",
                "--quiet",
                "--frozen",
                "--locked",
                "--offline",
                "--lib",
                "--bins",
                "--examples",
                "--tests",
                "--benches",
                "--all-targets",
                "--workspace",
                "--all-features",
                "--no-default-features",
                "--features",
                "--manifest-path",
                "--message-format",
                "--target",
                "--target-dir",
                "--jobs",
                "--keep-going",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "npm",
        &ShellSpec {
            subcommands: &[
                "install", "add", "remove", "run", "test", "start", "build", "publish", "pack",
                "version", "audit", "fund", "ls", "outdated", "prune", "dedupe", "exec", "init",
                "login", "logout", "link", "unlink", "config", "get", "set", "cache", "ping",
            ],
            flags: &[
                "--help",
                "--version",
                "--save",
                "--save-dev",
                "--save-optional",
                "--global",
                "--dry-run",
                "--force",
                "--legacy-peer-deps",
                "--workspaces",
                "--workspace",
                "--include-workspace-root",
                "--if-present",
                "--silent",
                "--verbose",
            ],
            values: &[("run", &[ValueKind::NpmScripts])],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "docker",
        &ShellSpec {
            subcommands: &[
                "build",
                "run",
                "exec",
                "ps",
                "images",
                "pull",
                "push",
                "tag",
                "rmi",
                "rm",
                "start",
                "stop",
                "restart",
                "logs",
                "inspect",
                "network",
                "volume",
                "container",
                "image",
                "system",
                "compose",
                "cp",
                "stats",
                "top",
                "login",
                "logout",
            ],
            flags: &[
                "--help",
                "--version",
                "--detach",
                "--interactive",
                "--tty",
                "--rm",
                "--name",
                "--volume",
                "--publish",
                "--env",
                "--env-file",
                "--workdir",
                "--network",
                "--pull",
                "--quiet",
                "--all",
                "--filter",
                "--format",
                "--follow",
                "--tail",
                "--since",
                "--until",
            ],
            values: &[],
            first_arg: &[],
            nested: DOCKER_NESTED,
        },
    ),
    (
        "kubectl",
        &ShellSpec {
            subcommands: &[
                "get",
                "describe",
                "create",
                "apply",
                "delete",
                "edit",
                "logs",
                "exec",
                "port-forward",
                "cp",
                "rollout",
                "scale",
                "autoscale",
                "top",
                "config",
                "cluster-info",
                "api-resources",
                "api-versions",
                "label",
                "annotate",
                "taint",
                "cordon",
                "uncordon",
                "drain",
            ],
            flags: &[
                "--help",
                "--namespace",
                "--all-namespaces",
                "--output",
                "--selector",
                "--field-selector",
                "--filename",
                "--recursive",
                "--dry-run",
                "--force",
                "--grace-period",
                "--timeout",
                "--follow",
                "--previous",
                "--tail",
                "--since",
                "--container",
                "--stdin",
                "--tty",
                "--replicas",
                "--current-replicas",
                "--record",
            ],
            values: &[
                ("get", &[ValueKind::K8sResources]),
                ("describe", &[ValueKind::K8sResources]),
                ("delete", &[ValueKind::K8sResources]),
                ("edit", &[ValueKind::K8sResources]),
                ("label", &[ValueKind::K8sResources]),
                ("annotate", &[ValueKind::K8sResources]),
                ("logs", &[ValueKind::K8sResources]),
            ],
            first_arg: &[],
            nested: KUBECTL_NESTED,
        },
    ),
    (
        "gh",
        &ShellSpec {
            subcommands: &[
                "issue",
                "pr",
                "repo",
                "gist",
                "run",
                "search",
                "auth",
                "browse",
                "release",
                "workflow",
                "secret",
                "variable",
                "cache",
                "codespace",
                "api",
                "completion",
                "config",
                "extension",
                "alias",
                "attestation",
                "label",
                "project",
                "ruleset",
            ],
            flags: &[
                "--help",
                "--version",
                "--repo",
                "--limit",
                "--json",
                "--jq",
                "--template",
                "--paginate",
                "--silent",
                "--verbose",
                "--hostname",
                "--search",
                "--state",
                "--label",
                "--assignee",
                "--author",
                "--mention",
                "--milestone",
                "--body",
                "--title",
                "--web",
                "--draft",
                "--fill",
                "--merge",
                "--squash",
                "--rebase",
            ],
            values: &[],
            first_arg: &[],
            nested: GH_NESTED,
        },
    ),
    (
        "ssh",
        &ShellSpec {
            subcommands: &[],
            flags: &[
                "-v", "-p", "-i", "-l", "-o", "-L", "-R", "-D", "-N", "-f", "-T", "-X", "-Y", "-C",
                "-q", "-A", "-J", "-t", "-n", "-4", "-6",
            ],
            values: &[],
            first_arg: &[ValueKind::SshHosts],
            nested: &[],
        },
    ),
    (
        "make",
        &ShellSpec {
            subcommands: &[],
            flags: &[
                "--help",
                "--version",
                "--directory",
                "--file",
                "--jobs",
                "--keep-going",
                "--dry-run",
                "--silent",
                "--always-make",
                "--new-file",
                "--old-file",
                "--what-if",
                "--print-directory",
                "--no-print-directory",
            ],
            values: &[],
            first_arg: &[ValueKind::MakeTargets],
            nested: &[],
        },
    ),
    (
        "go",
        &ShellSpec {
            subcommands: &[
                "build", "run", "test", "vet", "fmt", "mod", "get", "install", "clean", "doc",
                "list", "version", "env", "tool", "generate",
            ],
            flags: &[
                "-run", "-v", "-count", "-race", "-cover", "-tags", "-ldflags", "-o", "-x", "-n",
                "-a", "-m", "-u", "-d", "-e", "-json", "-short", "-timeout", "--help",
            ],
            values: &[("run", &[ValueKind::Files])],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "pip",
        &ShellSpec {
            subcommands: &[
                "install",
                "uninstall",
                "download",
                "freeze",
                "list",
                "show",
                "check",
                "config",
                "cache",
                "wheel",
                "hash",
                "debug",
            ],
            flags: &[
                "--help",
                "--version",
                "--verbose",
                "--quiet",
                "--requirement",
                "--constraint",
                "--no-deps",
                "--upgrade",
                "--force-reinstall",
                "--user",
                "--target",
                "--index-url",
                "--extra-index-url",
                "--dry-run",
                "--editable",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "brew",
        &ShellSpec {
            subcommands: &[
                "install",
                "uninstall",
                "reinstall",
                "upgrade",
                "update",
                "list",
                "info",
                "search",
                "services",
                "tap",
                "untap",
                "doctor",
                "cleanup",
                "autoremove",
                "deps",
                "leaves",
            ],
            flags: &[
                "--help",
                "--version",
                "--verbose",
                "--quiet",
                "--force",
                "--dry-run",
                "--cask",
                "--formula",
                "--build-from-source",
                "--adopt",
                "--skip-cask-deps",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "tmux",
        &ShellSpec {
            subcommands: &[
                "new",
                "new-session",
                "attach",
                "attach-session",
                "detach",
                "kill-server",
                "kill-session",
                "kill-window",
                "list-sessions",
                "list-windows",
                "rename-session",
                "rename-window",
                "split-window",
                "new-window",
                "select-window",
                "select-pane",
                "resize-pane",
                "send-keys",
                "capture-pane",
                "show-options",
                "set-option",
                "source-file",
            ],
            flags: &[
                "-d", "-s", "-t", "-n", "-c", "-e", "-v", "-2", "-u", "-C", "-L", "-S", "-f",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "terraform",
        &ShellSpec {
            subcommands: &[
                "init",
                "plan",
                "apply",
                "destroy",
                "validate",
                "fmt",
                "output",
                "show",
                "import",
                "state",
                "workspace",
                "providers",
                "taint",
                "untaint",
                "refresh",
                "console",
                "graph",
                "version",
            ],
            flags: &[
                "--help",
                "--auto-approve",
                "--var",
                "--var-file",
                "--target",
                "--out",
                "--input",
                "--refresh",
                "--parallelism",
                "--state",
                "--backup",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "node",
        &ShellSpec {
            subcommands: &[],
            flags: &[
                "--help",
                "--version",
                "--eval",
                "--print",
                "--check",
                "--inspect",
                "--inspect-brk",
                "--watch",
                "--watch-path",
                "--env-file",
                "--test",
                "--test-reporter",
                "--run",
                "--experimental-strip-types",
                "--no-warnings",
                "--trace-warnings",
                "--pending-deprecation",
                "--max-old-space-size",
            ],
            values: &[],
            first_arg: &[ValueKind::Files],
            nested: &[],
        },
    ),
    (
        "yarn",
        &ShellSpec {
            subcommands: &[
                "install",
                "add",
                "remove",
                "run",
                "test",
                "build",
                "start",
                "publish",
                "pack",
                "version",
                "audit",
                "list",
                "outdated",
                "upgrade",
                "init",
                "login",
                "logout",
                "link",
                "unlink",
                "config",
                "cache",
                "workspaces",
                "dlx",
                "exec",
            ],
            flags: &[
                "--help",
                "--version",
                "--dev",
                "--peer",
                "--optional",
                "--exact",
                "--tilde",
                "--latest",
                "--silent",
                "--verbose",
                "--non-interactive",
                "--ignore-scripts",
                "--frozen-lockfile",
                "--top-level",
            ],
            values: &[("run", &[ValueKind::NpmScripts])],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "pnpm",
        &ShellSpec {
            subcommands: &[
                "install", "add", "remove", "run", "test", "start", "build", "publish", "pack",
                "version", "audit", "list", "outdated", "update", "init", "login", "logout",
                "link", "unlink", "store", "dlx", "exec", "env",
            ],
            flags: &[
                "--help",
                "--version",
                "--save-dev",
                "--save-optional",
                "--save-peer",
                "--global",
                "--recursive",
                "--filter",
                "--workspace-root",
                "--silent",
                "--reporter",
                "--frozen-lockfile",
                "--offline",
                "--prefer-offline",
            ],
            values: &[("run", &[ValueKind::NpmScripts])],
            first_arg: &[],
            nested: &[],
        },
    ),
    (
        "python",
        &ShellSpec {
            subcommands: &[],
            flags: &[
                "-h",
                "-V",
                "-c",
                "-m",
                "-u",
                "-b",
                "-B",
                "-E",
                "-O",
                "-q",
                "-i",
                "-s",
                "-S",
                "-v",
                "-W",
                "-X",
                "--help",
                "--version",
            ],
            values: &[],
            first_arg: &[ValueKind::Files],
            nested: &[],
        },
    ),
    (
        "scp",
        &ShellSpec {
            subcommands: &[],
            flags: &[
                "-r", "-P", "-i", "-l", "-o", "-C", "-q", "-v", "-p", "-c", "-F", "-J", "-S", "-3",
            ],
            values: &[],
            first_arg: &[ValueKind::SshHosts, ValueKind::Files],
            nested: &[],
        },
    ),
    (
        "helm",
        &ShellSpec {
            subcommands: &[
                "install",
                "upgrade",
                "uninstall",
                "list",
                "status",
                "rollback",
                "history",
                "repo",
                "search",
                "show",
                "template",
                "lint",
                "package",
                "push",
                "pull",
                "dependency",
                "plugin",
                "version",
                "env",
                "get",
            ],
            flags: &[
                "--help",
                "--namespace",
                "--create-namespace",
                "--values",
                "--set",
                "--set-string",
                "--set-file",
                "--version",
                "--repo",
                "--kube-context",
                "--kubeconfig",
                "--dry-run",
                "--debug",
                "--wait",
                "--timeout",
                "--atomic",
                "--cleanup-on-fail",
                "--generate-name",
            ],
            values: &[],
            first_arg: &[],
            nested: &[
                ("repo", &["add", "list", "remove", "update", "index"]),
                ("search", &["repo", "hub"]),
                ("show", &["chart", "values", "readme", "crds"]),
                ("get", &["manifest", "values", "notes", "hooks"]),
                ("dependency", &["build", "list", "update"]),
                ("plugin", &["list", "install", "uninstall", "update"]),
            ],
        },
    ),
    (
        "systemctl",
        &ShellSpec {
            subcommands: &[
                "start",
                "stop",
                "restart",
                "reload",
                "status",
                "enable",
                "disable",
                "is-active",
                "is-enabled",
                "is-failed",
                "list-units",
                "list-unit-files",
                "show",
                "cat",
                "edit",
                "mask",
                "unmask",
                "daemon-reload",
                "reset-failed",
            ],
            flags: &[
                "--help",
                "--version",
                "--user",
                "--system",
                "--no-pager",
                "--full",
                "--all",
                "--failed",
                "--now",
                "--runtime",
                "--quiet",
                "--no-block",
            ],
            values: &[],
            first_arg: &[],
            nested: &[],
        },
    ),
];

/// One-line command summaries for completion rows. Curated for the
/// commands people actually pause on; unknown commands show the kind
/// tag alone rather than a guessed description.
pub fn command_desc(command: &str) -> Option<&'static str> {
    Some(match command {
        "git" => "distributed version control",
        "cargo" => "Rust package manager and build tool",
        "npm" => "Node package manager",
        "node" => "run JavaScript with Node.js",
        "yarn" => "Node package manager (Yarn)",
        "pnpm" => "fast Node package manager",
        "python" => "run Python scripts",
        "pip" => "Python package installer",
        "go" => "Go toolchain",
        "docker" => "containers: build, run, manage",
        "kubectl" => "talk to a Kubernetes cluster",
        "helm" => "Kubernetes package manager",
        "gh" => "GitHub CLI: issues, PRs, releases",
        "ssh" => "remote shell over SSH",
        "scp" => "copy files over SSH",
        "make" => "run Makefile targets",
        "brew" => "macOS/Linux package manager",
        "tmux" => "terminal multiplexer",
        "terraform" => "infrastructure as code",
        "systemctl" => "control systemd services",
        "ls" => "list directory contents",
        "cd" => "change directory",
        "pwd" => "print working directory",
        "cat" => "print file contents",
        "cp" => "copy files",
        "mv" => "move or rename files",
        "rm" => "remove files",
        "mkdir" => "create directories",
        "touch" => "create files / update times",
        "chmod" => "change file permissions",
        "chown" => "change file ownership",
        "ln" => "create links",
        "find" => "search for files",
        "grep" => "search text with patterns",
        "rg" => "fast recursive search (ripgrep)",
        "sed" => "stream text editor",
        "awk" => "pattern scanning language",
        "head" => "first lines of a file",
        "tail" => "last lines of a file",
        "less" => "page through output",
        "echo" => "print arguments",
        "ps" => "list running processes",
        "kill" => "stop a process",
        "df" => "disk free space",
        "du" => "directory disk usage",
        "curl" => "transfer data over HTTP",
        "wget" => "download files",
        "tar" => "archive files",
        "vim" | "nvim" | "vi" => "edit files (modal editor)",
        "code" => "open in VS Code",
        "jq" => "query JSON",
        "fzf" => "fuzzy finder",
        "bat" => "cat with syntax highlighting",
        "fd" => "fast file finder",
        "ping" => "test network reachability",
        "ssh-keygen" => "create SSH keys",
        "man" => "read the manual",
        "which" => "locate a command",
        "env" => "run with a modified environment",
        "watch" => "repeat a command periodically",
        _ => return None,
    })
}

/// One-line subcommand summaries, keyed by command. Table order follows
/// the spec tables; missing pairs fall back to the kind tag.
pub fn sub_desc(command: &str, sub: &str) -> Option<&'static str> {
    let desc = match command {
        "git" => match sub {
            "status" => "show working tree state",
            "checkout" => "switch branches or restore files",
            "branch" => "list, create, or delete branches",
            "commit" => "record changes to the repository",
            "log" => "show commit history",
            "diff" => "show changes between commits",
            "push" => "upload commits to a remote",
            "pull" => "fetch and merge from a remote",
            "add" => "stage changes",
            "fetch" => "download objects from a remote",
            "merge" => "join branches together",
            "clone" => "copy a repository",
            "show" => "show an object in detail",
            "stash" => "shelve changes temporarily",
            "restore" => "restore working tree files",
            "switch" => "switch branches",
            "reset" => "reset HEAD and optionally files",
            "rebase" => "reapply commits on another base",
            "remote" => "manage remotes",
            "tag" => "list or create tags",
            "revert" => "undo a commit with a new commit",
            "cherry-pick" => "apply selected commits here",
            "clean" => "remove untracked files",
            "mv" => "move or rename tracked files",
            "rm" => "remove tracked files",
            "grep" => "search tracked files",
            "describe" => "name a commit from the nearest tag",
            "init" => "create an empty repository",
            "blame" => "show who changed each line",
            "bisect" => "binary-search a regression",
            "archive" => "export a tree snapshot",
            "am" => "apply mailbox patches",
            "worktree" => "manage linked working trees",
            _ => return None,
        },
        "cargo" => match sub {
            "build" => "compile the package",
            "run" => "build and run the binary",
            "test" => "run the test suite",
            "check" => "typecheck without codegen",
            "clippy" => "run the linter",
            "fmt" => "format the code",
            "add" => "add a dependency",
            "remove" => "remove a dependency",
            "update" => "update dependencies",
            "clean" => "remove build artifacts",
            "doc" => "build documentation",
            "new" => "create a new package",
            "init" => "init a package here",
            "install" => "install a binary crate",
            "publish" => "publish to crates.io",
            "tree" => "show the dependency tree",
            "fix" => "auto-fix warnings",
            "bench" => "run benchmarks",
            "search" => "search crates.io",
            "metadata" => "machine-readable package info",
            _ => return None,
        },
        "npm" | "yarn" | "pnpm" => match sub {
            "install" => "install dependencies",
            "add" => "add a dependency",
            "remove" => "remove a dependency",
            "run" => "run a package script",
            "test" => "run the test script",
            "start" => "run the start script",
            "build" => "run the build script",
            "publish" => "publish the package",
            "audit" => "audit for vulnerabilities",
            "outdated" => "list stale dependencies",
            "init" => "scaffold a package.json",
            "exec" => "run a binary from node_modules",
            "login" => "authenticate with the registry",
            _ => return None,
        },
        "docker" => match sub {
            "run" => "start a new container",
            "exec" => "run a command inside a container",
            "ps" => "list containers",
            "images" => "list images",
            "build" => "build an image",
            "pull" => "download an image",
            "push" => "upload an image",
            "logs" => "read container logs",
            "inspect" => "full JSON details",
            "stop" => "stop a container",
            "start" => "start a stopped container",
            "restart" => "restart a container",
            "rm" => "remove a container",
            "rmi" => "remove an image",
            "tag" => "retarget an image name",
            "cp" => "copy files in or out",
            "container" => "manage containers",
            "image" => "manage images",
            "network" => "manage networks",
            "volume" => "manage volumes",
            "system" => "disk usage and prune",
            "compose" => "multi-container apps",
            "login" => "authenticate with a registry",
            _ => return None,
        },
        "kubectl" => match sub {
            "get" => "list resources",
            "describe" => "detailed resource state",
            "create" => "create a resource",
            "apply" => "apply a manifest",
            "delete" => "delete resources",
            "edit" => "edit a resource live",
            "logs" => "read pod logs",
            "exec" => "run a command in a pod",
            "port-forward" => "forward a local port",
            "rollout" => "manage rollouts",
            "scale" => "set replica count",
            "top" => "resource usage",
            "config" => "manage kubeconfig",
            "cluster-info" => "cluster endpoints",
            "cordon" => "mark a node unschedulable",
            "drain" => "evict pods from a node",
            _ => return None,
        },
        "gh" => match sub {
            "issue" => "manage issues",
            "pr" => "manage pull requests",
            "repo" => "manage repositories",
            "run" => "manage workflow runs",
            "release" => "manage releases",
            "browse" => "open in the browser",
            "search" => "search GitHub",
            "auth" => "authenticate",
            "workflow" => "manage Actions workflows",
            "secret" => "manage secrets",
            "gist" => "manage gists",
            "api" => "raw API requests",
            _ => return None,
        },
        "go" => match sub {
            "build" => "compile packages",
            "run" => "compile and run",
            "test" => "run tests",
            "vet" => "check for mistakes",
            "fmt" => "format sources",
            "mod" => "manage go.mod",
            "get" => "add a dependency",
            "install" => "install a binary",
            "list" => "list packages",
            "env" => "print Go environment",
            _ => return None,
        },
        "pip" => match sub {
            "install" => "install packages",
            "uninstall" => "remove packages",
            "freeze" => "pin installed versions",
            "list" => "list installed packages",
            "show" => "package details",
            "download" => "fetch without installing",
            "config" => "manage configuration",
            _ => return None,
        },
        "brew" => match sub {
            "install" => "install a formula",
            "uninstall" => "remove a formula",
            "upgrade" => "upgrade packages",
            "update" => "refresh formulae",
            "list" => "list installed",
            "info" => "package details",
            "search" => "search formulae",
            "services" => "manage background services",
            "doctor" => "diagnose the install",
            "cleanup" => "remove old versions",
            "deps" => "show dependencies",
            _ => return None,
        },
        "tmux" => match sub {
            "new" | "new-session" => "start a session",
            "attach" | "attach-session" => "attach to a session",
            "detach" => "detach this client",
            "list-sessions" => "list sessions",
            "kill-session" => "kill a session",
            "kill-server" => "kill the server",
            "split-window" => "split the pane",
            "new-window" => "open a window",
            "send-keys" => "type into a pane",
            "capture-pane" => "grab pane contents",
            "rename-session" => "rename the session",
            _ => return None,
        },
        "terraform" => match sub {
            "init" => "init the working directory",
            "plan" => "preview changes",
            "apply" => "apply changes",
            "destroy" => "tear everything down",
            "validate" => "check configuration",
            "fmt" => "format configuration",
            "output" => "show output values",
            "import" => "adopt existing resources",
            "state" => "manage state",
            "workspace" => "manage workspaces",
            _ => return None,
        },
        "helm" => match sub {
            "install" => "install a chart",
            "upgrade" => "upgrade a release",
            "uninstall" => "remove a release",
            "list" => "list releases",
            "status" => "release status",
            "rollback" => "roll back a release",
            "repo" => "manage chart repos",
            "search" => "search charts",
            "template" => "render templates locally",
            "lint" => "lint a chart",
            _ => return None,
        },
        "systemctl" => match sub {
            "start" => "start a unit",
            "stop" => "stop a unit",
            "restart" => "restart a unit",
            "status" => "unit status",
            "enable" => "start at boot",
            "disable" => "drop from boot",
            "is-active" => "is it running?",
            "list-units" => "list loaded units",
            "daemon-reload" => "reload unit files",
            "mask" => "forbid a unit entirely",
            "edit" => "override a unit",
            "cat" => "show the unit file",
            _ => return None,
        },
        _ => return None,
    };
    Some(desc)
}

/// One-line flag summaries. Per-command entries first, then the generic
/// table shared by every command (`--help` means the same everywhere).
pub fn flag_desc(command: &str, flag: &str) -> Option<&'static str> {
    let specific = match command {
        "git" => match flag {
            "--porcelain" => "machine-readable output",
            "--oneline" => "one line per commit",
            "--amend" => "fold into the last commit",
            "--cached" => "staged changes only",
            "--set-upstream" => "remember the remote branch",
            "--single-branch" => "clone one branch only",
            "--no-verify" => "skip commit hooks",
            "--dry-run" => "show what would happen",
            "--decorate" => "annotate refs in log",
            "--graph" => "draw the branch graph",
            _ => return generic_flag_desc(flag),
        },
        "kubectl" => match flag {
            "--namespace" => "target namespace (-n)",
            "--all-namespaces" => "every namespace (-A)",
            "--output" => "output format (-o)",
            "--selector" => "label selector (-l)",
            "--field-selector" => "field selector",
            "--filename" => "manifest file (-f)",
            "--previous" => "previous container run",
            "--container" => "which container (-c)",
            _ => return generic_flag_desc(flag),
        },
        "docker" => match flag {
            "--detach" => "run in background (-d)",
            "--interactive" => "keep stdin open (-i)",
            "--tty" => "allocate a terminal (-t)",
            "--rm" => "remove when it exits",
            "--name" => "assign a name",
            "--volume" => "mount a volume (-v)",
            "--publish" => "publish a port (-p)",
            "--env" => "set an env var (-e)",
            "--env-file" => "env vars from a file",
            "--all" => "include stopped (-a)",
            "--filter" => "filter results",
            "--format" => "Go-template output",
            "--follow" => "stream new output (-f)",
            "--tail" => "last N lines",
            _ => return generic_flag_desc(flag),
        },
        "gh" => match flag {
            "--repo" => "pick a repository (-R)",
            "--limit" => "max items to fetch",
            "--json" => "JSON output fields",
            "--jq" => "filter JSON output",
            "--web" => "open in the browser (-w)",
            "--draft" => "mark as draft",
            _ => return generic_flag_desc(flag),
        },
        "cargo" => match flag {
            "--release" => "optimized build",
            "--features" => "enable features",
            "--all-features" => "every feature",
            "--no-default-features" => "drop the defaults",
            "--manifest-path" => "path to Cargo.toml",
            "--target" => "build for a triple",
            "--frozen" => "no network, lockfile exact",
            "--locked" => "assert the lockfile",
            "--offline" => "no network access",
            "--workspace" => "whole workspace",
            _ => return generic_flag_desc(flag),
        },
        _ => return generic_flag_desc(flag),
    };
    Some(specific)
}

/// Flags that mean the same thing in every tool.
fn generic_flag_desc(flag: &str) -> Option<&'static str> {
    Some(match flag {
        "--help" | "-h" => "show help",
        "--version" | "-V" => "show the version",
        "--verbose" | "-v" => "verbose output",
        "--quiet" | "-q" => "quiet output",
        "--force" | "-f" => "force it",
        "--dry-run" => "show what would happen",
        "--all" | "-a" => "everything",
        "--recursive" | "-r" => "recurse",
        "--output" | "-o" => "output format",
        "--format" => "output format",
        "--filter" => "filter results",
        "--follow" => "follow output",
        "--tail" => "last N lines",
        "--since" => "only newer than this",
        "--timeout" => "give up after this long",
        "--jobs" | "-j" => "parallel jobs",
        "--target" => "build target",
        "--features" => "enable features",
        "--namespace" | "-n" => "namespace",
        "--global" | "-g" => "global scope",
        "--silent" => "minimal output",
        "--debug" => "debug output",
        "--no-pager" => "no pager",
        "--user" => "user scope",
        "--system" => "system scope",
        "--now" => "apply immediately",
        _ => return None,
    })
}
#[derive(Debug, Clone, Default)]
pub struct ContextCache {
    git: GitRefs,
    git_dir: std::path::PathBuf,
    git_at: Option<std::time::Instant>,
    scripts: Vec<String>,
    npm_dir: std::path::PathBuf,
    npm_at: Option<std::time::Instant>,
    targets: Vec<String>,
    make_dir: std::path::PathBuf,
    make_at: Option<std::time::Instant>,
    hosts: Vec<String>,
    ssh_at: Option<std::time::Instant>,
    pub aliases: Vec<(String, String)>,
    aliases_at: Option<std::time::Instant>,
    pub git_alias_list: Vec<(String, String)>,
    pub kube: Vec<String>,
    kube_at: Option<std::time::Instant>,
    pub units: Vec<String>,
    units_at: Option<std::time::Instant>,
    pub live: LiveCache,
}

const CTX_TTL: std::time::Duration = std::time::Duration::from_secs(15);

impl ContextCache {
    /// Refresh whatever the current command needs. Unknown commands need
    /// nothing, so the cache stays cold until a spec'd command is typed.
    pub fn refresh_for(&mut self, line: &str, cwd: &std::path::Path) {
        let tokens = split_shell_tokens(line);
        let mut index = 0;
        while tokens
            .get(index)
            .is_some_and(|token| WRAPPERS.contains(&token.as_str()))
        {
            index += 1;
        }
        let command = tokens.get(index).map(String::as_str).unwrap_or("");
        let now = std::time::Instant::now();
        let stale = |at: Option<std::time::Instant>| at.is_none_or(|at| now - at >= CTX_TTL);
        match command {
            "git" if self.git_dir != cwd || stale(self.git_at) => {
                self.git = git_refs(cwd);
                self.git_alias_list = git_aliases(cwd);
                self.git_dir = cwd.to_path_buf();
                self.git_at = Some(now);
            }
            "npm" | "npx" if self.npm_dir != cwd || stale(self.npm_at) => {
                self.scripts = npm_scripts(cwd);
                self.npm_dir = cwd.to_path_buf();
                self.npm_at = Some(now);
            }
            "make" if self.make_dir != cwd || stale(self.make_at) => {
                self.targets = make_targets(cwd);
                self.make_dir = cwd.to_path_buf();
                self.make_at = Some(now);
            }
            "ssh" | "scp" if stale(self.ssh_at) => {
                self.hosts = ssh_hosts();
                self.ssh_at = Some(now);
            }
            "kubectl" if stale(self.kube_at) => {
                self.kube = kube_contexts();
                self.kube_at = Some(now);
            }
            "systemctl" if stale(self.units_at) => {
                self.units = systemd_units();
                self.units_at = Some(now);
            }
            _ => {}
        }
        // Aliases back every shell line (spec lookup and menus expand
        // through them); the files are tiny and TTL-gated.
        if stale(self.aliases_at) {
            self.aliases = shell_aliases();
            self.aliases_at = Some(now);
        }
    }
}

/// Branch, tag, and remote names plus the current branch, read straight
/// from `.git` (refs plus packed-refs, `HEAD` for current, `config` for
/// remotes). Handles worktree pointer files; anything unreadable yields
/// empty lists rather than an error.
#[derive(Debug, Clone, Default)]
pub struct GitRefs {
    pub branches: Vec<String>,
    pub tags: Vec<String>,
    pub remotes: Vec<String>,
    pub current: Option<String>,
}

pub fn git_refs(cwd: &std::path::Path) -> GitRefs {
    let dot = cwd.join(".git");
    let dir = if dot.is_dir() {
        dot
    } else if dot.is_file() {
        let content = std::fs::read_to_string(&dot).unwrap_or_default();
        let target = content.strip_prefix("gitdir:").map(str::trim).unwrap_or("");
        if target.is_empty() {
            return GitRefs::default();
        }
        let path = std::path::PathBuf::from(target);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    } else {
        return GitRefs::default();
    };
    let mut refs = GitRefs::default();
    collect_ref_dir(&dir.join("refs").join("heads"), &mut refs.branches);
    collect_ref_dir(&dir.join("refs").join("tags"), &mut refs.tags);
    if let Ok(packed) = std::fs::read_to_string(dir.join("packed-refs")) {
        for line in packed.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            let mut parts = line.split_whitespace();
            if parts.next().is_none() {
                continue;
            }
            match parts.next() {
                Some(name) => {
                    if let Some(branch) = name.strip_prefix("refs/heads/") {
                        refs.branches.push(branch.to_string());
                    } else if let Some(tag) = name.strip_prefix("refs/tags/") {
                        refs.tags.push(tag.trim_end_matches("^{}").to_string());
                    }
                }
                None => continue,
            }
        }
    }
    refs.branches.sort();
    refs.branches.dedup();
    refs.branches.truncate(300);
    refs.tags.sort();
    refs.tags.dedup();
    refs.tags.truncate(300);
    let mut configs = vec![dir.join("config")];
    if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
        let common = common.trim();
        if !common.is_empty() {
            let path = std::path::PathBuf::from(common);
            let resolved = if path.is_absolute() {
                path
            } else {
                dir.join(path)
            };
            configs.push(resolved.join("config"));
        }
    }
    for config in configs {
        if let Ok(content) = std::fs::read_to_string(&config) {
            for line in content.lines() {
                let line = line.trim();
                if let Some(rest) = line
                    .strip_prefix("[remote \"")
                    .and_then(|rest| rest.strip_suffix("\"]"))
                    && !rest.is_empty()
                {
                    refs.remotes.push(rest.to_string());
                }
            }
        }
    }
    refs.remotes.sort();
    refs.remotes.dedup();
    if let Ok(head) = std::fs::read_to_string(dir.join("HEAD")) {
        let head = head.trim();
        if let Some(branch) = head
            .strip_prefix("ref: refs/heads/")
            .filter(|branch| !branch.is_empty())
        {
            refs.current = Some(branch.to_string());
        }
    }
    refs
}

/// Recursively collect ref names under `dir` (slash-joined, so
/// `feature/x` branches survive), capped so a huge repo cannot stall a
/// keystroke.
fn collect_ref_dir(dir: &std::path::Path, into: &mut Vec<String>) {
    const MAX_REFS: usize = 400;
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((path, prefix)) = stack.pop() {
        let Ok(listing) = std::fs::read_dir(&path) else {
            continue;
        };
        let mut entries: Vec<_> = listing.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if into.len() >= MAX_REFS {
                return;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let qualified = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                stack.push((entry.path(), qualified));
            } else {
                into.push(qualified);
            }
        }
    }
}

/// Script names from the workspace `package.json`, for `npm run <Tab>`.
pub fn npm_scripts(cwd: &std::path::Path) -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string(cwd.join("package.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    let Some(scripts) = json.get("scripts").and_then(|value| value.as_object()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = scripts.keys().cloned().collect();
    names.sort();
    names.truncate(100);
    names
}

/// Target names from the workspace makefile, in file order (the default
/// target first — the most likely continuation). Skips pattern rules,
/// variable assignments, dot-targets, and indented recipe lines.
pub fn make_targets(cwd: &std::path::Path) -> Vec<String> {
    let path = ["GNUmakefile", "makefile", "Makefile"]
        .iter()
        .map(|name| cwd.join(name))
        .find(|path| path.is_file());
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    for line in content.lines() {
        if line.starts_with([' ', '\t']) || line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let (head, _) = line.split_at(colon);
        if head.contains('=') || head.contains('%') || head.trim().is_empty() {
            continue;
        }
        let head = head.trim();
        if head.starts_with('.') || head.chars().any(char::is_whitespace) {
            continue;
        }
        if !targets.contains(&head.to_string()) {
            targets.push(head.to_string());
        }
        if targets.len() >= 200 {
            break;
        }
    }
    targets
}

/// Host nicknames from `~/.ssh/config` (`Host` lines, first pattern,
/// wildcards skipped), for `ssh <Tab>`.
pub fn ssh_hosts() -> Vec<String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let Ok(content) =
        std::fs::read_to_string(std::path::Path::new(&home).join(".ssh").join("config"))
    else {
        return Vec::new();
    };
    let mut hosts = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        if !words
            .next()
            .is_some_and(|key| key.eq_ignore_ascii_case("host"))
        {
            continue;
        }
        if let Some(first) = words.next()
            && !first.contains(['*', '?', '!'])
            && !hosts.contains(&first.to_string())
        {
            hosts.push(first.to_string());
        }
        if hosts.len() >= 200 {
            break;
        }
    }
    hosts.sort();
    hosts
}

/// Kubernetes context names from kubeconfig files: the `name:` entries
/// under the top-level `contexts:` section, current context first.
/// Pure file reads — no cluster round-trip, so `kubectl config
/// use-context <Tab>` completes synchronously.
pub fn kube_contexts_from(files: &[std::path::PathBuf]) -> Vec<String> {
    let mut contexts = Vec::new();
    let mut current = Vec::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        let mut section = String::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            if indent == 0 && !trimmed.starts_with('-') {
                // Top-level entry: switch sections, or record the
                // current context; anything else ends the section.
                if let Some(name) = trimmed.strip_prefix("current-context:") {
                    let name = name.trim().trim_matches(['"', '\'']);
                    if !name.is_empty() {
                        current.push(name.to_string());
                    }
                } else if let Some(name) = trimmed.strip_suffix(':') {
                    section = name.trim().to_string();
                } else {
                    section.clear();
                }
                continue;
            }
            if section != "contexts" {
                continue;
            }
            let value = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            if let Some(name) = value.strip_prefix("name:") {
                let name = name.trim().trim_matches(['"', '\'']);
                if !name.is_empty() && !contexts.contains(&name.to_string()) {
                    contexts.push(name.to_string());
                }
            }
        }
    }
    let mut ordered = current;
    for name in contexts {
        if !ordered.contains(&name) {
            ordered.push(name);
        }
    }
    ordered.truncate(100);
    ordered
}

/// Kubeconfig search paths: `$KUBECONFIG` (colon-separated) or the
/// default `~/.kube/config`.
pub fn kube_contexts() -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(kubeconfig) = std::env::var("KUBECONFIG")
        && !kubeconfig.trim().is_empty()
    {
        files.extend(std::env::split_paths(&kubeconfig));
    } else if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
        && !home.is_empty()
    {
        files.push(std::path::Path::new(&home).join(".kube").join("config"));
    }
    kube_contexts_from(&files)
}

/// Installed systemd unit names from the unit directories (system plus
/// user): `systemctl status ngi<Tab>` completes without the daemon.
pub fn systemd_units_from(dirs: &[std::path::PathBuf]) -> Vec<String> {
    const SUFFIXES: &[&str] = &[".service", ".socket", ".timer", ".target", ".path"];
    let mut units = Vec::new();
    for dir in dirs {
        let Ok(listing) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in listing.flatten().take(400) {
            let name = entry.file_name().to_string_lossy().to_string();
            if SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) && !units.contains(&name) {
                units.push(name);
            }
        }
    }
    units.sort();
    units.truncate(300);
    units
}

/// Standard unit directories, system-wide plus the user's.
pub fn systemd_units() -> Vec<String> {
    let mut dirs = vec![
        std::path::PathBuf::from("/etc/systemd/system"),
        std::path::PathBuf::from("/run/systemd/system"),
        std::path::PathBuf::from("/usr/lib/systemd/system"),
        std::path::PathBuf::from("/usr/local/lib/systemd/system"),
    ];
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"))
        && !home.is_empty()
    {
        let home = std::path::Path::new(&home);
        dirs.push(home.join(".config").join("systemd").join("user"));
        dirs.push(
            home.join(".local")
                .join("share")
                .join("systemd")
                .join("user"),
        );
    }
    systemd_units_from(&dirs)
}

/// Daemon-sourced values (pods, namespaces, containers, images).
/// Written only by background refresh events; the keystroke path reads
/// whatever is cached, possibly nothing. `at`/`failed_at` gate refresh
/// pacing, `inflight` stops duplicate spawns.
#[derive(Debug, Clone, Default)]
pub struct LiveCache {
    pub namespaces: Vec<String>,
    pub pods: Vec<String>,
    pub containers: Vec<String>,
    pub images: Vec<String>,
    pub at: HashMap<String, std::time::Instant>,
    pub inflight: HashSet<String>,
    pub failed_at: HashMap<String, std::time::Instant>,
}

/// How long live values stay fresh, and how long a failed daemon stays
/// quiet before the next attempt.
pub const LIVE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
pub const LIVE_FAIL_QUIET: std::time::Duration = std::time::Duration::from_secs(300);

/// Shell aliases (`name=value`) from the usual rc files, for expanding
/// the command word before spec lookup (`g st` completes as git).
/// One level only, capped; unreadable files yield nothing.
pub fn shell_aliases() -> Vec<(String, String)> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    if home.is_empty() {
        return Vec::new();
    }
    let home = std::path::Path::new(&home);
    let mut out = Vec::new();
    for file in ["bashrc", "bash_aliases", "zshrc", "zsh_aliases"] {
        let name = if file.contains('.') {
            file.to_string()
        } else {
            format!(".{file}")
        };
        let Ok(content) = std::fs::read_to_string(home.join(name)) else {
            continue;
        };
        for line in content.lines() {
            if let Some((name, value)) = parse_shell_alias(line)
                && !out.iter().any(|(known, _)| known == &name)
            {
                out.push((name, value));
            }
            if out.len() >= 200 {
                return out;
            }
        }
    }
    out
}

/// One `alias ...` line: bash/zsh `name=value` (quotes stripped) and
/// fish `name value` forms. Comments and flags never qualify.
fn parse_shell_alias(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let rest = line.strip_prefix("alias")?;
    let rest = rest.strip_prefix([' ', '\t'])?;
    if rest.starts_with('-') {
        return None;
    }
    let (name, value) = match rest.split_once('=') {
        Some((name, value)) => (name.trim(), value.trim()),
        // Fish form: `alias ll ls -l`.
        None => {
            let mut words = rest.split_whitespace();
            match (words.next(), words.next()) {
                (Some(name), Some(_)) => (name, rest[name.len()..].trim()),
                _ => return None,
            }
        }
    };
    if name.is_empty() || name.chars().any(char::is_whitespace) || value.is_empty() {
        return None;
    }
    finish_alias(name, value)
}

fn finish_alias(name: &str, value: &str) -> Option<(String, String)> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or(value);
    if unquoted.is_empty() {
        return None;
    }
    Some((name.to_string(), unquoted.to_string()))
}

/// Git aliases from the global and repo configs (`st = status`), so
/// `git co <Tab>` completes checkout's branches. Later files win.
pub fn git_aliases(cwd: &std::path::Path) -> Vec<(String, String)> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    let mut files = Vec::new();
    if !home.is_empty() {
        let home = std::path::Path::new(&home);
        files.push(home.join(".gitconfig"));
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                files.push(std::path::Path::new(&xdg).join("git").join("config"));
            }
        } else {
            files.push(home.join(".config").join("git").join("config"));
        }
    }
    files.push(cwd.join(".git").join("config"));
    let mut out: Vec<(String, String)> = Vec::new();
    for file in files {
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut in_alias = false;
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_alias = line.eq_ignore_ascii_case("[alias]");
                continue;
            }
            if !in_alias || line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some((name, value)) = line.split_once('=') {
                let name = name.trim().to_string();
                let value = value.trim().trim_matches('"').to_string();
                if name.is_empty() || value.is_empty() {
                    continue;
                }
                if let Some(slot) = out.iter_mut().find(|(known, _)| *known == name) {
                    slot.1 = value;
                } else {
                    out.push((name, value));
                }
            }
            if out.len() >= 100 {
                break;
            }
        }
    }
    out
}

/// Full-token file candidates for a value position: `src/ma` yields
/// `src/main.rs`, directories keep a trailing `/` so completion can
/// continue. Hidden entries stay hidden unless the fragment starts with
/// a dot. Absolute tokens resolve from `/`, relative ones from `cwd`.
pub fn file_completions(token: &str, cwd: &std::path::Path, dirs_only: bool) -> Vec<String> {
    if token.is_empty() {
        return Vec::new();
    }
    let (dir_part, file_part) = match token.rsplit_once('/') {
        Some((dir, file)) => (dir, file),
        None => (".", token),
    };
    let base = if token.starts_with('/') {
        std::path::PathBuf::from("/")
    } else {
        cwd.to_path_buf()
    };
    let dir = base.join(dir_part);
    let Ok(listing) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let prefix = match token.rsplit_once('/') {
        Some((head, _)) => format!("{head}/"),
        None => String::new(),
    };
    let mut candidates = Vec::new();
    for entry in listing.flatten().take(300) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && !file_part.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if dirs_only && !is_dir {
            continue;
        }
        let display = if is_dir { format!("{name}/") } else { name };
        candidates.push(format!("{prefix}{display}"));
    }
    candidates.sort();
    candidates.truncate(50);
    candidates
}

/// Whether the line ends inside an unclosed quote: any trailing token
/// is quoted text, however it is spaced.
fn in_unclosed_quote(line: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(char) = chars.next() {
        if let Some(mark) = quote {
            if char == mark {
                quote = None;
            } else if char == '\\' {
                chars.next();
            }
        } else if char == '\'' || char == '"' {
            quote = Some(char);
        } else if char == '\\' {
            chars.next();
        }
    }
    quote.is_some()
}

/// Whether the token under the cursor sits inside quotes: an unclosed
/// quote always counts; otherwise look at the raw line just before the
/// token's trailing position (the split token drops quote characters).
fn token_quoted(line: &str, token: &str) -> bool {
    if in_unclosed_quote(line) {
        return true;
    }
    let trimmed = line.trim_end();
    if token.is_empty() || !trimmed.ends_with(token) {
        return false;
    }
    let Some(start) = trimmed.len().checked_sub(token.len()) else {
        return false;
    };
    trimmed[..start]
        .chars()
        .last()
        .is_some_and(|mark| mark == '"' || mark == '\'')
}

/// All prefix matches for one value kind (capped): the menu half of
/// value completion. Candidates with whitespace need quotes, where a
/// plain token swap stays exact.
pub fn value_candidates(
    kind: ValueKind,
    token: &str,
    quoted: bool,
    cwd: &std::path::Path,
    cache: &ContextCache,
) -> Vec<String> {
    const CAP: usize = 8;
    let accept = |candidate: &str| quoted || !candidate.contains(char::is_whitespace);
    if matches!(kind, ValueKind::Files | ValueKind::Dirs) {
        return file_completions(token, cwd, kind == ValueKind::Dirs)
            .into_iter()
            .filter(|candidate| {
                candidate.len() > token.len() && candidate.starts_with(token) && accept(candidate)
            })
            .take(CAP)
            .collect();
    }
    if kind == ValueKind::K8sResources {
        return prefix_matches(K8S_RESOURCES.iter().copied(), token, quoted);
    }
    let cached: &[String] = match kind {
        ValueKind::Branches => &cache.git.branches,
        ValueKind::Tags => &cache.git.tags,
        ValueKind::Remotes => &cache.git.remotes,
        ValueKind::NpmScripts => &cache.scripts,
        ValueKind::MakeTargets => &cache.targets,
        ValueKind::SshHosts => &cache.hosts,
        ValueKind::KubeContexts => &cache.kube,
        ValueKind::KubeNamespaces => &cache.live.namespaces,
        ValueKind::KubePods => &cache.live.pods,
        ValueKind::DockerContainers => &cache.live.containers,
        ValueKind::DockerImages => &cache.live.images,
        ValueKind::SystemdUnits => &cache.units,
        ValueKind::K8sResources => unreachable!("handled above"),
        ValueKind::Files | ValueKind::Dirs => unreachable!("handled above"),
    };
    prefix_matches(cached.iter().map(String::as_str), token, quoted)
}

/// A line whose final token is an exact spec subcommand or flag reads
/// as complete; the model ghost has nothing to add (`git status`,
/// `docker ps`, `git --version`), saving an idle model round-trip.
pub fn line_looks_complete(line: &str, ctx: &ContextCache) -> bool {
    let Some(context) = shell_context(line, &ctx.aliases, &ctx.git_alias_list) else {
        return false;
    };
    if context.token.is_empty() {
        return false;
    }
    let Some(spec) = shell_spec(&context.command) else {
        return false;
    };
    let token = context.token.as_str();
    if token.starts_with('-') {
        return spec.flags.contains(&token);
    }
    context.position == 1 && spec.subcommands.contains(&token)
}

/// Ordered prefix matches (strictly longer, quote-aware), capped.
fn prefix_matches<'candidate>(
    candidates: impl IntoIterator<Item = &'candidate str>,
    token: &str,
    quoted: bool,
) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|candidate| {
            candidate.len() > token.len()
                && candidate.starts_with(token)
                && (quoted || !candidate.contains(char::is_whitespace))
        })
        .take(8)
        .map(str::to_string)
        .collect()
}

/// Complete one argument token against ordered value kinds: the first
/// kind with a prefix match wins.
fn complete_values(
    token: &str,
    line: &str,
    cwd: &std::path::Path,
    cache: &ContextCache,
    kinds: &[ValueKind],
) -> Option<String> {
    let quoted = token_quoted(line, token);
    for kind in kinds {
        // A kind with no match falls through to the next kind.
        if let Some(pick) = value_candidates(*kind, token, quoted, cwd, cache)
            .into_iter()
            .next()
        {
            return Some(pick[token.len()..].to_string());
        }
    }
    None
}

/// A shell line resolved for completion: transparent wrappers dropped,
/// one shell-alias level expanded, so `sudo g st` looks up git's
/// subcommands with token `st`. `position` 0 is the command word
/// itself; `subcommand` is the resolved first argument (git aliases
/// resolved, so `git co` values complete checkout's branches).
pub struct ShellContext {
    pub command: String,
    pub position: usize,
    pub token: String,
    pub subcommand: String,
    /// Second argument (`pods` in `kubectl get pods …`): selects live
    /// values at deeper positions.
    pub resource: String,
}

pub fn shell_context(
    line: &str,
    aliases: &[(String, String)],
    git_aliases: &[(String, String)],
) -> Option<ShellContext> {
    let tokens = split_shell_tokens(line);
    if tokens.is_empty() {
        return None;
    }
    let trailing = line.chars().last().is_some_and(|char| char.is_whitespace());
    let open_quote = in_unclosed_quote(line);
    let mut start = 0;
    while tokens
        .get(start)
        .is_some_and(|token| WRAPPERS.contains(&token.as_str()))
    {
        start += 1;
    }
    // One alias level: the typed tail survives verbatim after the
    // expansion, so token math below stays exact.
    let mut expanded: Vec<String> = tokens[start..].to_vec();
    if let Some(target) = expanded
        .first()
        .and_then(|first| aliases.iter().find(|(name, _)| name == first))
        .map(|(_, value)| value.clone())
    {
        let mut value_tokens = split_shell_tokens(&target);
        if !value_tokens.is_empty() && value_tokens[0] != expanded[0] {
            value_tokens.extend(expanded[1..].iter().cloned());
            expanded = value_tokens;
        }
    }
    let separator = trailing && !open_quote;
    let position = if separator {
        expanded.len()
    } else {
        expanded.len().saturating_sub(1)
    };
    let token = if separator {
        String::new()
    } else {
        expanded.last().cloned().unwrap_or_default()
    };
    let command = expanded.first()?.clone();
    let raw_sub = expanded.get(1).cloned().unwrap_or_default();
    let subcommand = if command == "git" {
        git_aliases
            .iter()
            .find(|(name, _)| *name == raw_sub)
            .and_then(|(_, value)| value.split_whitespace().next())
            .unwrap_or(&raw_sub)
            .to_string()
    } else {
        raw_sub
    };
    let resource = expanded.get(2).cloned().unwrap_or_default();
    Some(ShellContext {
        command,
        position,
        token,
        subcommand,
        resource,
    })
}

/// Context-aware ghost suffix for a shell line: flags after `-`,
/// subcommands (or first-arg values like hosts/targets) in first
/// position, nested subcommands then per-subcommand values after that.
/// `None` for command-word lines (the command-name layer owns those)
/// and empty tokens.
pub fn context_suggest(line: &str, cwd: &std::path::Path, cache: &ContextCache) -> Option<String> {
    let context = shell_context(line, &cache.aliases, &cache.git_alias_list)?;
    if context.token.is_empty() || context.position == 0 {
        return None;
    }
    let token = context.token.as_str();
    let spec = shell_spec(context.command.as_str());
    // Flag position: complete the flag name (`--ver` → `--version`).
    // Flag *values* (`--format x`) stay manual.
    if token.starts_with('-') && token.len() > 1 {
        let flags: &[&str] = spec.map(|spec| spec.flags).unwrap_or(&[]);
        let hit = best_prefix_match(flags.iter().copied(), token)?;
        return Some(hit[token.len()..].to_string());
    }
    // First-argument position: subcommands, or values for commands
    // that take them directly (`ssh host`, `make target`).
    if context.position == 1 {
        if let Some(spec) = spec {
            if !spec.subcommands.is_empty() {
                let hit = best_prefix_match(spec.subcommands.iter().copied(), token)?;
                return Some(hit[token.len()..].to_string());
            }
            if !spec.first_arg.is_empty() {
                return complete_values(token, line, cwd, cache, spec.first_arg);
            }
        }
        return None;
    }
    // Second position behind a nested verb (`gh issue li` → `list`).
    if context.position == 2
        && let Some(spec) = spec
        && let Some((_, subs)) = spec
            .nested
            .iter()
            .find(|(name, _)| *name == context.subcommand)
        && let Some(hit) = best_prefix_match(subs.iter().copied(), token)
    {
        return Some(hit[token.len()..].to_string());
    }
    // Live values at deeper positions (`docker logs ub` → `untu`).
    if context.position >= 2
        && let Some(kind) = live_kind(
            &context.command,
            &context.subcommand,
            &context.resource,
            context.position,
        )
        && let Some(suffix) = complete_values(token, line, cwd, cache, &[kind])
    {
        return Some(suffix);
    }
    // Value position: kinds keyed by the subcommand in first position.
    let kinds: &[ValueKind] = spec
        .and_then(|spec| {
            spec.values
                .iter()
                .find(|(name, _)| *name == context.subcommand)
                .map(|(_, kinds)| *kinds)
        })
        .unwrap_or(&[]);
    if kinds.is_empty() {
        return None;
    }
    complete_values(token, line, cwd, cache, kinds)
}

/// One Tab-menu row for a shell line: the full replacement text, its
/// kind tag (`history`, `subcommand`, `flag`, `branch`, …), whether
/// accepting swaps the whole line (history) or just the token under
/// the cursor, and a one-line description (empty when unknown — the
/// kind tag alone beats a guessed doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCandidate {
    pub text: String,
    pub kind: &'static str,
    pub whole_line: bool,
    pub detail: String,
}

/// Swap a candidate into its line: whole-line picks replace everything,
/// token picks replace the token under the cursor (or append after a
/// separator space).
pub fn apply_shell_candidate(line: &str, candidate: &ShellCandidate) -> String {
    if candidate.whole_line {
        return candidate.text.clone();
    }
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return candidate.text.clone();
    }
    if line.chars().last().is_some_and(|char| char.is_whitespace()) && !in_unclosed_quote(line) {
        return format!("{trimmed} {}", candidate.text);
    }
    let start = trimmed
        .char_indices()
        .rev()
        .find(|(_, char)| char.is_whitespace())
        .map(|(index, char)| index + char.len_utf8())
        .unwrap_or(0);
    format!("{}{}", &trimmed[..start], candidate.text)
}

/// Tab-menu row cap: the menu stays a menu.
const MENU_CAP: usize = 8;

fn push_candidate(
    out: &mut Vec<ShellCandidate>,
    seen: &mut HashSet<(String, bool)>,
    text: String,
    kind: &'static str,
    whole_line: bool,
    detail: &str,
) {
    if out.len() < MENU_CAP && seen.insert((text.clone(), whole_line)) {
        out.push(ShellCandidate {
            text,
            kind,
            whole_line,
            detail: detail.to_string(),
        });
    }
}

/// One ordered prefix row unless the menu is full. Spaced names need
/// quotes, where a plain token swap stays exact.
fn push_token_row(
    out: &mut Vec<ShellCandidate>,
    seen: &mut HashSet<(String, bool)>,
    candidate: &str,
    token: &str,
    quoted: bool,
    kind: &'static str,
    detail: &str,
) {
    if out.len() < MENU_CAP
        && candidate.len() > token.len()
        && candidate.starts_with(token)
        && (quoted || !candidate.contains(char::is_whitespace))
    {
        push_candidate(out, seen, candidate.to_string(), kind, false, detail);
    }
}

/// Live value kind for deeper positions: `kubectl logs <pod>`,
/// `kubectl get pods <pod>`, `kubectl config use-context <ctx>`,
/// `docker logs <name>`, `systemctl status <unit>`. File-backed kinds
/// (contexts, units) resolve synchronously; daemon kinds (pods,
/// namespaces, containers, images) read the background cache, which is
/// empty until the first refresh lands — the keystroke path never
/// waits for a daemon.
pub fn live_kind(
    command: &str,
    subcommand: &str,
    resource: &str,
    position: usize,
) -> Option<ValueKind> {
    match command {
        "kubectl" => {
            if subcommand == "config"
                && matches!(
                    resource,
                    "use-context" | "delete-context" | "rename-context" | "set-context"
                )
            {
                return Some(ValueKind::KubeContexts);
            }
            if position == 2 && matches!(subcommand, "logs" | "exec") {
                return Some(ValueKind::KubePods);
            }
            if position == 3
                && matches!(
                    subcommand,
                    "get" | "describe" | "delete" | "edit" | "label" | "annotate"
                )
            {
                if resource == "namespaces" || resource == "namespace" || resource == "ns" {
                    return Some(ValueKind::KubeNamespaces);
                }
                if resource == "pods"
                    || resource == "pod"
                    || resource == "po"
                    || resource == "deployments"
                    || resource == "deploy"
                    || resource == "statefulsets"
                    || resource == "daemonsets"
                    || resource == "jobs"
                {
                    return Some(ValueKind::KubePods);
                }
            }
            None
        }
        "docker" => {
            if position != 2 {
                return None;
            }
            if matches!(
                subcommand,
                "exec"
                    | "logs"
                    | "start"
                    | "stop"
                    | "restart"
                    | "rm"
                    | "inspect"
                    | "top"
                    | "stats"
                    | "kill"
                    | "pause"
                    | "unpause"
                    | "rename"
                    | "wait"
                    | "attach"
                    | "cp"
            ) {
                return Some(ValueKind::DockerContainers);
            }
            if matches!(subcommand, "rmi" | "tag" | "push" | "save" | "history") {
                return Some(ValueKind::DockerImages);
            }
            None
        }
        "systemctl" => {
            if position == 2
                && matches!(
                    subcommand,
                    "start"
                        | "stop"
                        | "restart"
                        | "reload"
                        | "status"
                        | "enable"
                        | "disable"
                        | "reenable"
                        | "is-active"
                        | "is-enabled"
                        | "is-failed"
                        | "show"
                        | "cat"
                        | "edit"
                        | "mask"
                        | "unmask"
                        | "preset"
                        | "revert"
                )
            {
                return Some(ValueKind::SystemdUnits);
            }
            None
        }
        _ => None,
    }
}

/// Tab-menu candidates for a shell line, most useful first: recent
/// history, then the same context the ghost uses (subcommands, flags,
/// nested verbs, values), then command names, then path files. Capped
/// so the menu stays a menu.
pub fn shell_candidates(
    line: &str,
    history: &ShellHistory,
    cwd: &std::path::Path,
    ctx: &ContextCache,
    bins: &[String],
) -> Vec<ShellCandidate> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<ShellCandidate> = Vec::new();
    let mut seen: HashSet<(String, bool)> = HashSet::new();
    for cmd in history.recent_matches(line, 4) {
        push_candidate(&mut out, &mut seen, cmd, "history", true, "");
    }
    let Some(context) = shell_context(line, &ctx.aliases, &ctx.git_alias_list) else {
        return out;
    };
    // An empty token (`git ` with a trailing space) still has a menu:
    // subcommands, recent history, and values all list from the start.
    let token = context.token.as_str();
    let spec = shell_spec(context.command.as_str());
    let quoted = token_quoted(line, token);
    // Command word: builtins, PATH, and alias names (alias rows show
    // the expansion, command rows a summary when curated).
    if context.position == 0 {
        for builtin in SHELL_BUILTINS {
            let detail = command_desc(builtin).unwrap_or("");
            push_token_row(
                &mut out, &mut seen, builtin, token, quoted, "command", detail,
            );
        }
        for bin in bins {
            let detail = command_desc(bin).unwrap_or("");
            push_token_row(&mut out, &mut seen, bin, token, quoted, "command", detail);
        }
        for (name, value) in &ctx.aliases {
            push_token_row(&mut out, &mut seen, name, token, quoted, "alias", value);
        }
        // An exact spec hit (`git` with no trailing space yet) offers
        // its subcommands as whole-line rows — `git` + Tab lists verbs
        // instead of going quiet.
        if token == context.command.as_str()
            && let Some(spec) = spec
        {
            for sub in spec.subcommands {
                let row = format!("{token} {sub}");
                let detail = sub_desc(&context.command, sub).unwrap_or("");
                push_candidate(&mut out, &mut seen, row, "subcommand", true, detail);
            }
        }
        return out;
    }
    // Flag names with per-command summaries.
    if token.starts_with('-') && token.len() > 1 {
        if let Some(spec) = spec {
            for flag in spec.flags {
                let detail = flag_desc(&context.command, flag).unwrap_or("");
                push_token_row(&mut out, &mut seen, flag, token, quoted, "flag", detail);
            }
        }
        return out;
    }
    // First argument: subcommands (plus git aliases), or direct values.
    if context.position == 1 {
        if let Some(spec) = spec {
            if !spec.subcommands.is_empty() {
                for sub in spec.subcommands {
                    let detail = sub_desc(&context.command, sub).unwrap_or("");
                    push_token_row(
                        &mut out,
                        &mut seen,
                        sub,
                        token,
                        quoted,
                        "subcommand",
                        detail,
                    );
                }
                if context.command == "git" {
                    for (name, value) in &ctx.git_alias_list {
                        push_token_row(&mut out, &mut seen, name, token, quoted, "alias", value);
                    }
                }
                push_files(&mut out, &mut seen, token, quoted, cwd, ctx);
                return out;
            }
            if !spec.first_arg.is_empty() {
                push_value_kinds(&mut out, &mut seen, token, quoted, cwd, ctx, spec.first_arg);
                return out;
            }
        }
        push_files(&mut out, &mut seen, token, quoted, cwd, ctx);
        return out;
    }
    // Nested verbs, then live values (pods, containers, units), then
    // per-subcommand values, then path files.
    if context.position == 2
        && let Some(spec) = spec
        && let Some((_, subs)) = spec
            .nested
            .iter()
            .find(|(name, _)| *name == context.subcommand)
    {
        for sub in *subs {
            push_token_row(&mut out, &mut seen, sub, token, quoted, "subcommand", "");
        }
    }
    if context.position >= 2
        && let Some(kind) = live_kind(
            &context.command,
            &context.subcommand,
            &context.resource,
            context.position,
        )
    {
        push_value_kinds(&mut out, &mut seen, token, quoted, cwd, ctx, &[kind]);
    }
    let kinds: &[ValueKind] = spec
        .and_then(|spec| {
            spec.values
                .iter()
                .find(|(name, _)| *name == context.subcommand)
                .map(|(_, kinds)| *kinds)
        })
        .unwrap_or(&[]);
    if !kinds.is_empty() {
        push_value_kinds(&mut out, &mut seen, token, quoted, cwd, ctx, kinds);
    } else {
        push_files(&mut out, &mut seen, token, quoted, cwd, ctx);
    }
    out
}

fn push_value_kinds(
    out: &mut Vec<ShellCandidate>,
    seen: &mut HashSet<(String, bool)>,
    token: &str,
    quoted: bool,
    cwd: &std::path::Path,
    ctx: &ContextCache,
    kinds: &[ValueKind],
) {
    for kind in kinds {
        for candidate in value_candidates(*kind, token, quoted, cwd, ctx) {
            if out.len() >= MENU_CAP {
                return;
            }
            push_candidate(out, seen, candidate, value_kind_tag(*kind), false, "");
        }
    }
}

fn push_files(
    out: &mut Vec<ShellCandidate>,
    seen: &mut HashSet<(String, bool)>,
    token: &str,
    quoted: bool,
    cwd: &std::path::Path,
    ctx: &ContextCache,
) {
    for candidate in value_candidates(ValueKind::Files, token, quoted, cwd, ctx)
        .into_iter()
        .take(4)
    {
        if out.len() >= MENU_CAP {
            return;
        }
        push_candidate(out, seen, candidate, "file", false, "");
    }
}

/// Menu tag per value kind.
pub fn value_kind_tag(kind: ValueKind) -> &'static str {
    match kind {
        ValueKind::Branches => "branch",
        ValueKind::Tags => "tag",
        ValueKind::Remotes => "remote",
        ValueKind::Files => "file",
        ValueKind::Dirs => "dir",
        ValueKind::NpmScripts => "script",
        ValueKind::MakeTargets => "target",
        ValueKind::SshHosts => "host",
        ValueKind::K8sResources => "resource",
        ValueKind::KubeContexts => "context",
        ValueKind::KubeNamespaces => "namespace",
        ValueKind::KubePods => "pod",
        ValueKind::DockerContainers => "container",
        ValueKind::DockerImages => "image",
        ValueKind::SystemdUnits => "unit",
    }
}

/// Ranked near-matches: prefix completions first (table order), then
/// fuzzy within distance 2, capped. `nearest` is the first of these.
pub fn nearest_all<'candidate>(
    token: &str,
    candidates: &[&'candidate str],
    cap: usize,
) -> Vec<&'candidate str> {
    if token.is_empty() || cap == 0 || candidates.contains(&token) {
        return Vec::new();
    }
    let mut out: Vec<&'candidate str> = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.len() > token.len() && candidate.starts_with(token))
        .take(cap)
        .collect();
    if out.len() >= cap || token.len() < 3 {
        return out;
    }
    let mut fuzzy: Vec<(&'candidate str, usize)> = candidates
        .iter()
        .copied()
        .filter(|candidate| *candidate != token && !candidate.starts_with(token))
        .map(|candidate| (candidate, edit_distance(token, candidate)))
        .filter(|(_, distance)| *distance <= 2)
        .collect();
    fuzzy.sort_by_key(|(_, distance)| *distance);
    for (candidate, _) in fuzzy {
        if out.len() >= cap {
            break;
        }
        out.push(candidate);
    }
    out
}

/// A failed shell line plus its proposed fixes: the best first, up to
/// two alternates. Fixes are raw shell text (no `!` prefix); the UI
/// offers the best on `→` and lists the rest in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub failed: String,
    pub fixed: String,
    pub more: Vec<String>,
}

/// Every applicable fix, best first, capped at 3: mistyped command
/// (builtins + PATH), mistyped git subcommand or branch, missing
/// upstream, non-executable script, mistyped path. Conservative: empty
/// unless a rule fires with a close match.
pub fn suggest_corrections(
    cmdline: &str,
    stderr: &str,
    cwd: &std::path::Path,
    bins: &[String],
) -> Vec<String> {
    const CAP: usize = 3;
    let tokens = split_shell_tokens(cmdline);
    let Some(argv0) = tokens.first().cloned() else {
        return Vec::new();
    };
    if argv0.is_empty() {
        return Vec::new();
    }
    let mut fixes: Vec<String> = Vec::new();
    let mut push = |fixed: String| {
        if fixes.len() < CAP && !fixes.contains(&fixed) && fixed != cmdline {
            fixes.push(fixed);
        }
    };
    let lower = stderr.to_ascii_lowercase();
    let argv0_lower = argv0.to_ascii_lowercase();
    // Mistyped executable: `gti` → `git`. The shell reports the missing
    // name, so only fire when stderr names argv0.
    if lower.contains("command not found")
        || (lower.contains("not found")
            && !lower.contains("no such file")
            && lower.contains(&argv0_lower))
    {
        let pool: Vec<&str> = SHELL_BUILTINS
            .iter()
            .copied()
            .chain(bins.iter().map(String::as_str))
            .collect();
        for hit in nearest_all(&argv0, &pool, CAP) {
            let mut fixed = tokens.clone();
            fixed[0] = hit.to_string();
            push(fixed.join(" "));
        }
    }
    if argv0 == "git" {
        // Mistyped subcommand: `git chekout` → `git checkout`.
        if lower.contains("is not a git command")
            && let Some(subcommand) = tokens.get(1)
        {
            for hit in nearest_all(subcommand, GIT_SUBCOMMANDS, CAP) {
                let mut fixed = tokens.clone();
                fixed[1] = hit.to_string();
                push(fixed.join(" "));
            }
        }
        // Unknown revision: `git checkout mian` → `git checkout main`.
        if (lower.contains("did not match") || lower.contains("unknown revision"))
            && let Some(target) = tokens.get(2)
        {
            let refs = git_refs(cwd);
            let pool: Vec<&str> = refs
                .branches
                .iter()
                .map(String::as_str)
                .chain(refs.tags.iter().map(String::as_str))
                .collect();
            if let Some(hit) = nearest(target, &pool) {
                let mut fixed = tokens.clone();
                fixed[2] = hit.to_string();
                push(fixed.join(" "));
            }
        }
        // Missing upstream: `git push` → `git push --set-upstream origin main`.
        if lower.contains("no upstream")
            && tokens.get(1).is_some_and(|sub| sub == "push")
            && !tokens.iter().any(|token| token == "--set-upstream")
            && let Some(branch) = git_refs(cwd).current
        {
            let remote = tokens
                .get(2)
                .cloned()
                .unwrap_or_else(|| "origin".to_string());
            push(format!("git push --set-upstream {remote} {branch}"));
        }
        // Mistyped flag against the known git flags.
        if (lower.contains("unknown option") || lower.contains("unknown flag"))
            && let Some(flag) = tokens.iter().find(|token| token.starts_with('-'))
        {
            let stripped = flag.trim_start_matches('-');
            let pool: Vec<&str> = GIT_FLAGS
                .iter()
                .map(|name| name.trim_start_matches('-'))
                .collect();
            if let Some(hit) = nearest(stripped, &pool) {
                let dashes = if flag.starts_with("--") { "--" } else { "-" };
                if let Some(position) = tokens.iter().position(|token| token == flag) {
                    let mut fixed = tokens.clone();
                    fixed[position] = format!("{dashes}{hit}");
                    push(fixed.join(" "));
                }
            }
        }
    }
    // Non-executable script: `./deploy` → `chmod +x ./deploy && ...`.
    if lower.contains("permission denied") {
        let target = argv0.trim_start_matches("./");
        if !target.is_empty() && !target.contains('/') && is_non_executable_file(&cwd.join(target))
        {
            push(format!("chmod +x {argv0} && {cmdline}"));
        }
    }
    // Mistyped path: nearest sibling in the same directory.
    if lower.contains("no such file or directory") {
        for (index, token) in tokens.iter().enumerate().skip(1) {
            if token.starts_with('-') || !lower.contains(&token.to_ascii_lowercase()) {
                continue;
            }
            if let Some(fix) = nearest_file(token, cwd) {
                let mut fixed = tokens.clone();
                fixed[index] = fix;
                push(fixed.join(" "));
                break;
            }
        }
    }
    fixes
}

/// True for a regular file without any execute bit (unix) or any
/// regular file (elsewhere — the execute bit is not portable).
fn is_non_executable_file(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 == 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Nearest sibling for a mistyped path, keeping the directory part:
/// `src/mian.rs` → `src/main.rs`.
fn nearest_file(token: &str, cwd: &std::path::Path) -> Option<String> {
    let (dir_part, file_part) = match token.rsplit_once('/') {
        Some((dir, file)) => (dir, file),
        None => (".", token),
    };
    if file_part.is_empty() {
        return None;
    }
    let dir = if token.starts_with('/') {
        std::path::PathBuf::from("/").join(dir_part)
    } else {
        cwd.join(dir_part)
    };
    let Ok(listing) = std::fs::read_dir(&dir) else {
        return None;
    };
    let mut names: Vec<String> = listing
        .flatten()
        .take(300)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !name.starts_with('.') || file_part.starts_with('.'))
        .collect();
    names.sort();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let hit = nearest(file_part, &borrowed)?;
    let prefix = match token.rsplit_once('/') {
        Some((head, _)) => format!("{head}/"),
        None => String::new(),
    };
    Some(format!("{prefix}{hit}"))
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

    /// Trigger shape: `!` shell lines of length ≥ 3 only.
    #[test]
    fn ghost_trigger_shape() {
        assert_eq!(ghost_prefix("!git sta"), Some("git sta".to_string()));
        assert_eq!(ghost_prefix("!ls"), Some("ls".to_string()));
        assert_eq!(ghost_prefix("!l"), None);
        assert_eq!(ghost_prefix("/sh git sta"), None);
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
            suggest(
                "!car",
                &history,
                directory.path(),
                &bins,
                &ContextCache::default(),
                score
            ),
            Some("go build".to_string())
        );
        // No history for the prefix: the binary completes `car` → `go`,
        // not the `cargo.toml` path top-hit.
        assert_eq!(
            suggest(
                "!car",
                &empty,
                directory.path(),
                &bins,
                &ContextCache::default(),
                score
            ),
            Some("go".to_string())
        );
        // No binary for the token: the path layer still serves.
        assert_eq!(
            suggest(
                "!./car",
                &empty,
                directory.path(),
                &bins,
                &ContextCache::default(),
                score
            ),
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

    /// Quotes group words; backslashes escape the next character.
    #[test]
    fn shell_tokens_respect_quotes() {
        assert_eq!(
            split_shell_tokens("git checkout main"),
            vec!["git", "checkout", "main"]
        );
        assert_eq!(
            split_shell_tokens("git checkout \"my branch\""),
            vec!["git", "checkout", "my branch"]
        );
        assert_eq!(
            split_shell_tokens("echo 'it\\'s' ok"),
            vec!["echo", "it's", "ok"]
        );
        assert_eq!(split_shell_tokens("  "), Vec::<String>::new());
    }

    /// Transpositions and single typos land within distance 2; exact
    /// matches and unrelated words do not.
    #[test]
    fn edit_distance_shapes() {
        assert_eq!(edit_distance("gti", "git"), 2);
        assert_eq!(edit_distance("chekout", "checkout"), 1);
        assert_eq!(edit_distance("git", "git"), 0);
        assert!(edit_distance("git", "kubectl") > 2);
    }

    /// Prefix completions beat fuzzy ones; exact matches need nothing.
    #[test]
    fn nearest_prefers_prefix_then_close() {
        let bins = ["git", "grep", "go"];
        assert_eq!(nearest("gi", &bins), Some("git"));
        assert_eq!(nearest("gti", &bins), Some("git"));
        assert_eq!(nearest("git", &bins), None);
        assert_eq!(nearest("zzz", &bins), None);
        assert_eq!(nearest("", &bins), None);
    }

    fn ctx_with_git(branches: &[&str]) -> ContextCache {
        ContextCache {
            git: GitRefs {
                branches: branches.iter().map(ToString::to_string).collect(),
                ..Default::default()
            },
            git_at: Some(std::time::Instant::now()),
            git_dir: std::path::PathBuf::from("/repo"),
            ..Default::default()
        }
    }

    /// Subcommands, flags, and branch values complete by prefix;
    /// single tokens and empty tokens stay with their own layers.
    #[test]
    fn context_suggests_subcommands_flags_branches() {
        let directory = tempfile::TempDir::new().unwrap();
        let ctx = ctx_with_git(&["main", "develop"]);
        assert_eq!(
            context_suggest("git check", directory.path(), &ctx),
            Some("out".to_string())
        );
        assert_eq!(
            context_suggest("git --ver", directory.path(), &ctx),
            Some("sion".to_string())
        );
        assert_eq!(
            context_suggest("git checkout ma", directory.path(), &ctx),
            Some("in".to_string())
        );
        assert_eq!(context_suggest("git", directory.path(), &ctx), None);
        assert_eq!(context_suggest("git ", directory.path(), &ctx), None);
        assert_eq!(
            context_suggest("cargo bui", directory.path(), &ctx),
            Some("ld".to_string())
        );
        assert_eq!(
            context_suggest("cargo build --rele", directory.path(), &ctx),
            Some("ase".to_string())
        );
        // Unknown commands and valueless positions stay silent.
        assert_eq!(
            context_suggest("frobnicate x", directory.path(), &ctx),
            None
        );
        assert_eq!(
            context_suggest("cargo build x", directory.path(), &ctx),
            None
        );
        // Wrappers are transparent.
        assert_eq!(
            context_suggest("sudo git check", directory.path(), &ctx),
            Some("out".to_string())
        );
    }

    /// Quoted tokens may complete values with spaces; unquoted ones
    /// must not (a suffix append cannot insert the missing quote).
    #[test]
    fn context_skips_spaced_values_unquoted() {
        let directory = tempfile::TempDir::new().unwrap();
        let ctx = ctx_with_git(&["my branch", "main"]);
        assert_eq!(
            context_suggest("git checkout \"my ", directory.path(), &ctx),
            Some("branch".to_string())
        );
        assert_eq!(
            context_suggest("git checkout my ", directory.path(), &ctx),
            None
        );
    }

    /// Value kinds resolve from the workspace: npm scripts and make
    /// targets complete without any git state.
    #[test]
    fn context_suggests_scripts_and_targets() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(
            directory.path().join("package.json"),
            r#"{"scripts": {"build": "tsc", "test": "vitest"}}"#,
        )
        .unwrap();
        std::fs::write(
            directory.path().join("Makefile"),
            "build:\n\tgo build ./...\n\ntest:\n\tgo test ./...\n",
        )
        .unwrap();
        let ctx = ContextCache {
            scripts: npm_scripts(directory.path()),
            targets: make_targets(directory.path()),
            ..Default::default()
        };
        assert_eq!(
            context_suggest("npm run bui", directory.path(), &ctx),
            Some("ld".to_string())
        );
        assert_eq!(
            context_suggest("make te", directory.path(), &ctx),
            Some("st".to_string())
        );
        // File values complete the basename after a directory prefix.
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/main.rs"), "").unwrap();
        assert_eq!(
            context_suggest("git add src/mai", directory.path(), &ctx),
            Some("n.rs".to_string())
        );
    }

    /// ssh completes host nicknames in first position.
    #[test]
    fn context_suggests_ssh_hosts() {
        let directory = tempfile::TempDir::new().unwrap();
        let ctx = ContextCache {
            hosts: vec!["prod-bastion".to_string(), "devbox".to_string()],
            ..Default::default()
        };
        assert_eq!(
            context_suggest("ssh prod-ba", directory.path(), &ctx),
            Some("stion".to_string())
        );
        assert_eq!(context_suggest("ssh", directory.path(), &ctx), None);
    }

    /// `.git` mining: loose refs, packed refs, HEAD, and remotes.
    #[test]
    fn git_refs_reads_loose_packed_head_config() {
        let directory = tempfile::TempDir::new().unwrap();
        let git = directory.path().join(".git");
        std::fs::create_dir_all(git.join("refs").join("heads").join("feature")).unwrap();
        std::fs::write(git.join("refs").join("heads").join("main"), "abc").unwrap();
        std::fs::write(
            git.join("refs").join("heads").join("feature").join("x"),
            "def",
        )
        .unwrap();
        std::fs::write(
            git.join("packed-refs"),
            "# pack-refs\n111 refs/heads/packed\n222 refs/tags/v1.0\n",
        )
        .unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("config"), "[remote \"origin\"]\n\turl = x\n").unwrap();
        let refs = git_refs(directory.path());
        assert_eq!(refs.branches, vec!["feature/x", "main", "packed"]);
        assert_eq!(refs.tags, vec!["v1.0"]);
        assert_eq!(refs.remotes, vec!["origin"]);
        assert_eq!(refs.current.as_deref(), Some("main"));
        // No repo: everything empty, no panic.
        let bare = tempfile::TempDir::new().unwrap();
        assert!(git_refs(bare.path()).branches.is_empty());
    }

    /// Makefile parsing keeps targets in order, skips recipes, pattern
    /// rules, assignments, and dot-targets.
    #[test]
    fn make_targets_parses() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(
            directory.path().join("Makefile"),
            "build: deps\n\tgo build\n\n%.o: %.c\n\tcc -c\n\nVAR = x\n\n.PHONY: build\ntest: build\n\tgo test\n",
        )
        .unwrap();
        assert_eq!(make_targets(directory.path()), vec!["build", "test"]);
    }

    /// Mistyped executables correct against builtins + PATH, best
    /// first with alternates after.
    #[test]
    fn correction_fixes_command_typos() {
        let directory = tempfile::TempDir::new().unwrap();
        let bins = vec!["git".to_string(), "cargo".to_string()];
        let fixes = suggest_corrections(
            "gti status",
            "sh: 1: gti: not found",
            directory.path(),
            &bins,
        );
        assert_eq!(fixes.first().map(String::as_str), Some("git status"));
        // An exact name with no close neighbor stays silent.
        assert!(
            suggest_corrections(
                "git status",
                "sh: 1: git: not found",
                directory.path(),
                &bins
            )
            .is_empty()
        );
        // Short names that truly miss stay silent.
        assert!(
            suggest_corrections("sl -l", "sh: 1: sl: not found", directory.path(), &bins)
                .is_empty()
        );
    }

    /// Ambiguous typos offer ranked alternates, capped at three.
    #[test]
    fn correction_lists_alternates() {
        let directory = tempfile::TempDir::new().unwrap();
        let bins = vec!["git".to_string(), "gimp".to_string(), "gist".to_string()];
        let fixes =
            suggest_corrections("gi status", "sh: 1: gi: not found", directory.path(), &bins);
        assert!(fixes.len() <= 3, "capped: {fixes:?}");
        assert!(
            fixes.iter().all(|fix| fix.ends_with(" status")),
            "argv0 only: {fixes:?}"
        );
        assert_eq!(fixes.first().map(String::as_str), Some("git status"));
    }

    /// Git subcommand, revision, upstream, and flag rules.
    #[test]
    fn correction_fixes_git_failures() {
        let directory = tempfile::TempDir::new().unwrap();
        let git = directory.path().join(".git");
        std::fs::create_dir_all(git.join("refs").join("heads")).unwrap();
        std::fs::write(git.join("refs").join("heads").join("main"), "abc").unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(git.join("config"), "[remote \"origin\"]\n").unwrap();
        let bins: Vec<String> = Vec::new();
        assert_eq!(
            suggest_corrections(
                "git chekout main",
                "error: 'chekout' is not a git command. See 'git --help'.",
                directory.path(),
                &bins
            )
            .first()
            .map(String::as_str),
            Some("git checkout main")
        );
        assert_eq!(
            suggest_corrections(
                "git checkout mian",
                "error: pathspec 'mian' did not match any file(s) known to git",
                directory.path(),
                &bins
            )
            .first()
            .map(String::as_str),
            Some("git checkout main")
        );
        assert_eq!(
            suggest_corrections(
                "git push",
                "fatal: The current branch main has no upstream branch.",
                directory.path(),
                &bins
            )
            .first()
            .map(String::as_str),
            Some("git push --set-upstream origin main")
        );
    }

    /// Mistyped paths correct to the nearest sibling named in stderr;
    /// non-executable scripts gain `chmod +x`.
    #[test]
    fn correction_fixes_paths_and_permissions() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(directory.path().join("main.rs"), "").unwrap();
        let bins: Vec<String> = Vec::new();
        assert_eq!(
            suggest_corrections(
                "cat mian.rs",
                "cat: mian.rs: No such file or directory",
                directory.path(),
                &bins
            )
            .first()
            .map(String::as_str),
            Some("cat main.rs")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = directory.path().join("deploy");
            std::fs::write(&script, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                suggest_corrections(
                    "./deploy",
                    "sh: 1: ./deploy: Permission denied",
                    directory.path(),
                    &bins
                )
                .first()
                .map(String::as_str),
                Some("chmod +x ./deploy && ./deploy")
            );
        }
    }

    /// Shell aliases parse from rc files; fish form included.
    #[test]
    fn shell_aliases_parse() {
        assert_eq!(
            parse_shell_alias("alias ll='ls -l'"),
            Some(("ll".to_string(), "ls -l".to_string()))
        );
        assert_eq!(
            parse_shell_alias("alias g=\"git\""),
            Some(("g".to_string(), "git".to_string()))
        );
        assert_eq!(
            parse_shell_alias("alias ll ls -l"),
            Some(("ll".to_string(), "ls -l".to_string()))
        );
        assert_eq!(parse_shell_alias("alias"), None);
        assert_eq!(parse_shell_alias("# alias x=y"), None);
        assert_eq!(parse_shell_alias("alias -p"), None);
        assert_eq!(parse_shell_alias("export FOO=1"), None);
    }

    /// Git aliases resolve through completion: `g st` behaves like
    /// `git status`, and checkout aliases complete branches.
    #[test]
    fn aliases_expand_through_completion() {
        let directory = tempfile::TempDir::new().unwrap();
        let ctx = ContextCache {
            aliases: vec![("g".to_string(), "git".to_string())],
            git_alias_list: vec![("co".to_string(), "checkout".to_string())],
            git: GitRefs {
                branches: vec!["main".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let context = shell_context("g st", &ctx.aliases, &ctx.git_alias_list).unwrap();
        assert_eq!(context.command, "git");
        assert_eq!(context.position, 1);
        assert_eq!(context.token, "st");
        assert_eq!(
            context_suggest("g st", directory.path(), &ctx),
            Some("atus".to_string())
        );
        // `git co ma` reads checkout's branches through the alias.
        assert_eq!(
            context_suggest("git co ma", directory.path(), &ctx),
            Some("in".to_string())
        );
    }

    /// Menu candidates mix history, specs, and files with kind tags;
    /// whole-line picks swap everything, token picks swap the token.
    #[test]
    fn shell_menu_candidates_mix_sources() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(directory.path().join("README.md"), "").unwrap();
        let mut history = ShellHistory::new(10);
        history.record("git status", "/repo");
        let ctx = ContextCache::default();
        let bins = vec!["git".to_string()];
        let items = shell_candidates("git sta", &history, directory.path(), &ctx, &bins);
        assert_eq!(
            items.first().map(|item| item.text.as_str()),
            Some("git status")
        );
        assert!(items.first().is_some_and(|item| item.whole_line));
        assert!(
            items
                .iter()
                .any(|item| item.text == "status" && item.kind == "subcommand" && !item.whole_line),
            "spec row missing: {items:?}"
        );
        let files = shell_candidates("cat READ", &history, directory.path(), &ctx, &bins);
        assert!(
            files
                .iter()
                .any(|item| item.text == "README.md" && item.kind == "file"),
            "file row missing: {files:?}"
        );
        // Nested verbs complete one level deeper.
        let nested = shell_candidates("gh issue li", &history, directory.path(), &ctx, &bins);
        assert!(
            nested.iter().any(|item| item.text == "list"),
            "nested row missing: {nested:?}"
        );
        // K8s resource types complete behind kubectl verbs.
        let k8s = shell_candidates("kubectl get po", &history, directory.path(), &ctx, &bins);
        assert!(
            k8s.iter()
                .any(|item| item.text == "pods" && item.kind == "resource"),
            "resource row missing: {k8s:?}"
        );
        // Candidate application: token swap vs whole-line swap.
        let token_pick = ShellCandidate {
            text: "status".to_string(),
            kind: "subcommand",
            whole_line: false,
            detail: String::new(),
        };
        assert_eq!(apply_shell_candidate("git sta", &token_pick), "git status");
        let line_pick = ShellCandidate {
            text: "git status".to_string(),
            kind: "history",
            whole_line: true,
            detail: String::new(),
        };
        assert_eq!(apply_shell_candidate("git sta", &line_pick), "git status");
        assert_eq!(apply_shell_candidate("git ", &token_pick), "git status");
    }

    /// Marker-free detection: typed commands and curated prefixes read
    /// as shell; prose, questions, and slash input never do.
    #[test]
    fn looks_like_shell_detects_commands_not_prose() {
        let aliases = vec![("g".to_string(), "git".to_string())];
        for shell in [
            "git status",
            "git",
            "kubectl get pods",
            "vim",
            "sudo apt update",
            "cargo build --release",
            "g st",
            "kub",
            "doc",
            "./script.sh",
            "ls -la | grep x",
        ] {
            assert!(looks_like_shell(shell, &aliases), "missed: {shell}");
        }
        for prose in [
            "hello world",
            "he",
            "write a test",
            "what is git status?",
            "/sh git status",
            "!git status",
            "@src/main.rs",
            "# explain git",
        ] {
            assert!(
                !looks_like_shell(prose, &aliases),
                "prose detected as shell: {prose}"
            );
        }
    }

    /// An empty token after a trailing space still lists the menu:
    /// `git ` shows subcommands, not just history.
    #[test]
    fn shell_menu_lists_subcommands_after_trailing_space() {
        let history = history(&[]);
        let ctx = ContextCache::default();
        let directory = tempfile::TempDir::new().unwrap();
        let items = shell_candidates("git ", &history, directory.path(), &ctx, &[]);
        assert!(
            items.iter().any(|item| item.text == "status"),
            "subcommand rows missing: {items:?}"
        );
        assert!(
            items
                .iter()
                .all(|item| !item.whole_line || item.kind == "history")
        );
    }

    /// Menu rows carry curated one-line docs where they exist and stay
    /// kind-only where they do not (a guessed doc is worse than none).
    #[test]
    fn menu_rows_carry_curated_descriptions() {
        let history = history(&[]);
        let ctx = ContextCache::default();
        let directory = tempfile::TempDir::new().unwrap();
        let items = shell_candidates("git sta", &history, directory.path(), &ctx, &[]);
        let status = items
            .iter()
            .find(|item| item.text == "status")
            .expect("status row");
        assert_eq!(status.detail, "show working tree state");
        let flags = shell_candidates("git --ver", &history, directory.path(), &ctx, &[]);
        let version = flags
            .iter()
            .find(|item| item.text == "--version")
            .expect("flag row");
        assert_eq!(version.detail, "show the version");
        assert!(command_desc("git").is_some_and(|desc| desc.contains("version control")));
        assert!(sub_desc("cargo", "build").is_some_and(|desc| desc.contains("compile")));
        assert!(flag_desc("kubectl", "--namespace").is_some_and(|desc| desc.contains("-n")));
    }

    /// Live kinds map to the verbs people pause on, at the right depth.
    #[test]
    fn live_kind_maps_daemon_verbs() {
        assert_eq!(
            live_kind("kubectl", "logs", "", 2),
            Some(ValueKind::KubePods)
        );
        assert_eq!(
            live_kind("kubectl", "get", "pods", 3),
            Some(ValueKind::KubePods)
        );
        assert_eq!(
            live_kind("kubectl", "get", "namespaces", 3),
            Some(ValueKind::KubeNamespaces)
        );
        assert_eq!(
            live_kind("kubectl", "config", "use-context", 2),
            Some(ValueKind::KubeContexts)
        );
        assert_eq!(
            live_kind("docker", "logs", "", 2),
            Some(ValueKind::DockerContainers)
        );
        assert_eq!(
            live_kind("docker", "rmi", "", 2),
            Some(ValueKind::DockerImages)
        );
        assert_eq!(
            live_kind("systemctl", "status", "", 2),
            Some(ValueKind::SystemdUnits)
        );
        assert_eq!(live_kind("git", "checkout", "", 2), None);
    }

    /// Kubeconfig contexts parse from YAML text without a cluster call;
    /// the current context leads.
    #[test]
    fn kube_contexts_parse_from_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let file = directory.path().join("config");
        std::fs::write(
            &file,
            "apiVersion: v1\ncurrent-context: prod\ncontexts:\n- name: prod\n  cluster: p\n- name: staging\n  cluster: s\n",
        )
        .unwrap();
        assert_eq!(
            kube_contexts_from(&[file]),
            vec!["prod".to_string(), "staging".to_string()]
        );
        assert!(kube_contexts_from(&[directory.path().join("absent")]).is_empty());
    }

    /// Unit directories list known suffixes, sorted and capped.
    #[test]
    fn systemd_units_parse_from_dir() {
        let directory = tempfile::TempDir::new().unwrap();
        std::fs::write(directory.path().join("nginx.service"), "").unwrap();
        std::fs::write(directory.path().join("cron.timer"), "").unwrap();
        std::fs::write(directory.path().join("notes.txt"), "").unwrap();
        assert_eq!(
            systemd_units_from(&[directory.path().to_path_buf()]),
            vec!["cron.timer".to_string(), "nginx.service".to_string()]
        );
    }

    /// Cached daemon values show up in Tab menus at live positions.
    #[test]
    fn live_values_complete_menu_rows() {
        let mut ctx = ContextCache::default();
        ctx.live.pods = vec!["api-7f9".to_string(), "web-2d1".to_string()];
        ctx.live.containers = vec!["db-1".to_string()];
        let directory = tempfile::TempDir::new().unwrap();
        let pods = shell_candidates(
            "kubectl logs api",
            &history(&[]),
            directory.path(),
            &ctx,
            &[],
        );
        assert!(
            pods.iter()
                .any(|item| item.text == "api-7f9" && item.kind == "pod"),
            "pod row missing: {pods:?}"
        );
        let containers =
            shell_candidates("docker logs db", &history(&[]), directory.path(), &ctx, &[]);
        assert!(
            containers
                .iter()
                .any(|item| item.text == "db-1" && item.kind == "container"),
            "container row missing: {containers:?}"
        );
    }

    /// Complete command lines skip the model ghost; partial ones do not.
    #[test]
    fn line_completeness_skips_model_ghost() {
        let ctx = ContextCache::default();
        assert!(line_looks_complete("git status", &ctx));
        assert!(line_looks_complete("git --version", &ctx));
        assert!(line_looks_complete("docker ps", &ctx));
        assert!(!line_looks_complete("git sta", &ctx));
        assert!(!line_looks_complete("git", &ctx));
        assert!(!line_looks_complete("docker ps -", &ctx));
        assert!(!line_looks_complete("git status --short", &ctx));
    }

    /// Marker-free lines resolve through the same cascade as `!` lines.
    #[test]
    fn bare_lines_resolve_through_the_cascade() {
        let history = history(&[("git status", "/repo")]);
        let cwd = std::path::PathBuf::from("/repo");
        let ctx = ContextCache::default();
        let bins = vec!["git".to_string(), "gzip".to_string()];
        assert_eq!(
            suggest_shell("git sta", &history, &cwd, &bins, &ctx, |_, _| None),
            Some("tus".to_string())
        );
        assert_eq!(
            suggest_shell("gzi", &history, &cwd, &bins, &ctx, |_, _| None),
            Some("p".to_string())
        );
    }
}
