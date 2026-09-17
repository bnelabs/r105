//! Anchored file edits and structured patches.
//!
//! Both tools write directly (no shell) and fail closed on ambiguous or
//! missing anchors. All paths stay inside the workspace via `safe_path`.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

use crate::security::{MAX_FILE_CONTENT, MAX_FILE_READ, safe_path, validate_tool_text};

/// Preview an anchored replacement without touching the filesystem.
/// Returns a short unified-style diff.
pub fn preview_edit(
    old_content: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<String> {
    if old_text.is_empty() {
        bail!("old_text is required");
    }
    validate_tool_text(new_text, "new_text", MAX_FILE_CONTENT)?;
    let matches = old_content.matches(old_text).count();
    if matches == 0 {
        bail!("anchor not found");
    }
    if matches > 1 && !replace_all {
        bail!("anchor matches {matches} locations; set replace_all or narrow old_text");
    }
    Ok(render_diff(
        old_content,
        &old_content.replacen(old_text, new_text, 1),
        replace_all,
        matches,
    ))
}

fn render_diff(before: &str, after_first: &str, replace_all: bool, matches: usize) -> String {
    let before_lines = before.lines().count();
    let after_lines = if replace_all {
        // Applied everywhere; count from the full replacement.
        after_first.lines().count() + matches.saturating_sub(1)
    } else {
        after_first.lines().count()
    };
    format!(
        "diff: {before_lines} -> {after_lines} lines ({matches} match{})",
        if matches == 1 { "" } else { "es" }
    )
}

/// Apply an anchored replacement to a workspace file.
pub fn apply_edit(
    workspace: &Path,
    path: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<String> {
    if old_text.is_empty() {
        bail!("old_text is required");
    }
    validate_tool_text(new_text, "new_text", MAX_FILE_CONTENT)?;
    let resolved = safe_path(workspace, path)?;
    let metadata =
        fs::metadata(&resolved).with_context(|| format!("reading {}", resolved.display()))?;
    if metadata.len() > MAX_FILE_READ {
        bail!("file too large ({} bytes)", metadata.len());
    }
    let old_content = fs::read_to_string(&resolved)
        .with_context(|| format!("decoding {}", resolved.display()))?;
    let matches = old_content.matches(old_text).count();
    if matches == 0 {
        bail!("anchor not found in {}", resolved.display());
    }
    if matches > 1 && !replace_all {
        bail!(
            "anchor matches {matches} locations in {}; narrow old_text or set replace_all",
            resolved.display()
        );
    }
    let updated = if replace_all {
        old_content.replace(old_text, new_text)
    } else {
        old_content.replacen(old_text, new_text, 1)
    };
    if updated == old_content {
        return Ok(format!("no changes for {}", resolved.display()));
    }
    validate_tool_text(&updated, "content", MAX_FILE_CONTENT)?;
    fs::write(&resolved, updated.as_bytes())?;
    Ok(format!(
        "{} updated ({} match{}, {} bytes)",
        resolved.display(),
        matches,
        if matches == 1 { "" } else { "es" },
        updated.len()
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatchAction {
    Add,
    Update,
    Delete,
}

#[derive(Debug, Clone)]
pub(crate) struct PatchFile {
    action: PatchAction,
    path: String,
    body: Vec<String>,
}

/// Parse the structured patch format. Relative paths only; anything else
/// fails before any file is touched.
pub fn parse_patch(patch: &str) -> Result<Vec<PatchFile>> {
    let lines: Vec<&str> = patch.lines().collect();
    let begin = lines
        .iter()
        .position(|line| line.trim() == "*** Begin Patch")
        .ok_or_else(|| anyhow::anyhow!("patch must start with *** Begin Patch"))?;
    let end = lines
        .iter()
        .position(|line| line.trim() == "*** End Patch")
        .ok_or_else(|| anyhow::anyhow!("patch must end with *** End Patch"))?;
    if end <= begin {
        bail!("patch end precedes begin");
    }
    let mut files: Vec<PatchFile> = Vec::new();
    let mut current: Option<PatchFile> = None;
    for line in &lines[begin + 1..end] {
        let trimmed = line.trim();
        if let Some(path) = trimmed
            .strip_prefix("*** Add File:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            flush(&mut current, &mut files)?;
            current = Some(PatchFile {
                action: PatchAction::Add,
                path: path.to_string(),
                body: Vec::new(),
            });
        } else if let Some(path) = trimmed
            .strip_prefix("*** Update File:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            flush(&mut current, &mut files)?;
            current = Some(PatchFile {
                action: PatchAction::Update,
                path: path.to_string(),
                body: Vec::new(),
            });
        } else if let Some(path) = trimmed
            .strip_prefix("*** Delete File:")
            .map(str::trim)
            .filter(|path| !path.is_empty())
        {
            flush(&mut current, &mut files)?;
            current = Some(PatchFile {
                action: PatchAction::Delete,
                path: path.to_string(),
                body: Vec::new(),
            });
        } else if trimmed.starts_with("***") {
            bail!("unknown patch header: {trimmed}");
        } else if let Some(file) = current.as_mut() {
            file.body.push(line.to_string());
        } else if trimmed.is_empty() {
            continue;
        } else {
            bail!("content outside a file section: {trimmed}");
        }
    }
    flush(&mut current, &mut files)?;
    if files.is_empty() {
        bail!("patch contains no files");
    }
    for file in &files {
        reject_absolute(&file.path)?;
    }
    Ok(files)
}

fn flush(current: &mut Option<PatchFile>, files: &mut Vec<PatchFile>) -> Result<()> {
    if let Some(file) = current.take() {
        if matches!(file.action, PatchAction::Update) && file.body.is_empty() {
            bail!("update for {} has no hunks", file.path);
        }
        files.push(file);
    }
    Ok(())
}

fn reject_absolute(path: &str) -> Result<()> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        bail!("absolute paths are not allowed: {path}");
    }
    if raw
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        bail!("path traversal is not allowed: {path}");
    }
    if path.is_empty() {
        bail!("file path is required");
    }
    Ok(())
}

/// Preview a patch without writing. Returns one line per file.
pub fn preview_patch(workspace: &Path, patch: &str) -> Result<String> {
    let files = parse_patch(patch)?;
    let mut lines = Vec::with_capacity(files.len());
    for file in &files {
        match file.action {
            PatchAction::Add => {
                lines.push(format!("add {} ({} lines)", file.path, file.body.len()));
            }
            PatchAction::Delete => {
                let resolved = safe_path(workspace, &file.path)?;
                if !resolved.exists() {
                    bail!("delete target missing: {}", file.path);
                }
                lines.push(format!("delete {}", file.path));
            }
            PatchAction::Update => {
                let resolved = safe_path(workspace, &file.path)?;
                let content = fs::read_to_string(&resolved)
                    .with_context(|| format!("reading {}", resolved.display()))?;
                let (old_block, new_block) = split_hunk(&file.body)?;
                let matches = count_block(&content, &old_block);
                if matches == 0 {
                    bail!("hunk does not match {}", file.path);
                }
                lines.push(format!(
                    "update {} ({} -> {} lines, {matches} match{})",
                    file.path,
                    old_block.len(),
                    new_block.len(),
                    if matches == 1 { "" } else { "es" }
                ));
            }
        }
    }
    Ok(lines.join("\n"))
}

/// Apply a parsed patch atomically per file (all-or-nothing per file,
///
/// files applied in order; a later failure leaves earlier files written —
/// callers preview first via the approval card).
pub fn apply_patch(workspace: &Path, patch: &str) -> Result<String> {
    validate_tool_text(patch, "patch", MAX_FILE_CONTENT)?;
    let files = parse_patch(patch)?;
    let mut reports = Vec::with_capacity(files.len());
    for file in &files {
        match file.action {
            PatchAction::Add => {
                let resolved = safe_path(workspace, &file.path)?;
                if resolved.exists() {
                    bail!("add target exists: {}", file.path);
                }
                let content = join_body(&file.body);
                validate_tool_text(&content, "content", MAX_FILE_CONTENT)?;
                if let Some(parent) = resolved.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&resolved, content.as_bytes())?;
                reports.push(format!(
                    "added {} ({} bytes)",
                    resolved.display(),
                    content.len()
                ));
            }
            PatchAction::Delete => {
                let resolved = safe_path(workspace, &file.path)?;
                if !resolved.exists() {
                    bail!("delete target missing: {}", file.path);
                }
                fs::remove_file(&resolved)?;
                reports.push(format!("deleted {}", resolved.display()));
            }
            PatchAction::Update => {
                let resolved = safe_path(workspace, &file.path)?;
                let content = fs::read_to_string(&resolved)
                    .with_context(|| format!("reading {}", resolved.display()))?;
                let (old_block, new_block) = split_hunk(&file.body)?;
                let updated = apply_hunk(&content, &old_block, &new_block, &file.path)?;
                validate_tool_text(&updated, "content", MAX_FILE_CONTENT)?;
                fs::write(&resolved, updated.as_bytes())?;
                reports.push(format!("updated {}", resolved.display()));
            }
        }
    }
    Ok(reports.join("\n"))
}

fn join_body(body: &[String]) -> String {
    let mut content = body.join("\n");
    if !body.is_empty() {
        content.push('\n');
    }
    // Allow `+`-prefixed bodies for familiarity; strip one level when
    // every non-empty line carries it.
    if !body.is_empty()
        && body
            .iter()
            .filter(|line| !line.is_empty())
            .all(|line| line.starts_with('+'))
    {
        content = body
            .iter()
            .map(|line| line.strip_prefix('+').unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        content.push('\n');
    }
    content
}

fn split_hunk(body: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let mut old_block = Vec::new();
    let mut new_block = Vec::new();
    for line in body {
        if line.starts_with("@@") {
            continue;
        } else if let Some(rest) = line.strip_prefix(' ') {
            old_block.push(rest.to_string());
            new_block.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('-') {
            old_block.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('+') {
            new_block.push(rest.to_string());
        } else if line.is_empty() {
            old_block.push(String::new());
            new_block.push(String::new());
        } else {
            bail!("bad hunk line (want ' '/-/+/@@): {line}");
        }
    }
    if old_block.is_empty() {
        bail!("hunk has no context or removals");
    }
    Ok((old_block, new_block))
}

fn count_block(content: &str, block: &[String]) -> usize {
    if block.is_empty() {
        return 0;
    }
    let needle = block.join("\n");
    content.matches(needle.as_str()).count()
}

fn apply_hunk(
    content: &str,
    old_block: &[String],
    new_block: &[String],
    path: &str,
) -> Result<String> {
    let needle = old_block.join("\n");
    let replacement = new_block.join("\n");
    let matches = content.matches(needle.as_str()).count();
    if matches == 0 {
        bail!("hunk does not match {path}");
    }
    if matches > 1 {
        bail!("hunk matches {matches} locations in {path}; narrow context");
    }
    Ok(content.replacen(needle.as_str(), replacement.as_str(), 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn edit_applies_anchored_replacement() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("a.txt"), "hello world\n").unwrap();
        let report = apply_edit(directory.path(), "a.txt", "world", "there", false).unwrap();
        assert!(report.contains("updated"), "{report}");
        assert_eq!(
            fs::read_to_string(directory.path().join("a.txt")).unwrap(),
            "hello there\n"
        );
    }

    #[test]
    fn edit_rejects_ambiguous_anchor() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("a.txt"), "x x x").unwrap();
        assert!(apply_edit(directory.path(), "a.txt", "x", "y", false).is_err());
        assert!(apply_edit(directory.path(), "a.txt", "x", "y", true).is_ok());
    }

    #[test]
    fn edit_rejects_missing_anchor() {
        assert!(preview_edit("hello", "missing", "x", false).is_err());
    }

    #[test]
    fn patch_add_update_delete_roundtrip() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("keep.txt"), "one\ntwo\nthree\n").unwrap();
        let patch = "*** Begin Patch\n*** Add File: new.txt\nhello\n*** Update File: keep.txt\n@@\n one\n-two\n+TWO\n three\n*** End Patch\n";
        let preview = preview_patch(directory.path(), patch).unwrap();
        assert!(preview.contains("add new.txt"), "{preview}");
        let report = apply_patch(directory.path(), patch).unwrap();
        assert!(report.contains("added"), "{report}");
        assert_eq!(
            fs::read_to_string(directory.path().join("keep.txt")).unwrap(),
            "one\nTWO\nthree\n"
        );
        let delete = "*** Begin Patch\n*** Delete File: new.txt\n*** End Patch\n";
        assert!(apply_patch(directory.path(), delete).is_ok());
        assert!(!directory.path().join("new.txt").exists());
    }

    #[test]
    fn patch_rejects_absolute_and_ambiguous() {
        let directory = tempdir().unwrap();
        assert!(parse_patch("*** Begin Patch\n*** Add File: /etc/x\n*** End Patch\n").is_err());
        fs::write(directory.path().join("a.txt"), "x\nx\n").unwrap();
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-x\n+y\n*** End Patch\n";
        assert!(apply_patch(directory.path(), patch).is_err());
    }
}
