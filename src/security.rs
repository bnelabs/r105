//! Security boundaries for native tools.

use std::{
    fs,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};

pub const MAX_CODE_SIZE: usize = 100 * 1024;
pub const MAX_FILE_CONTENT: usize = 10 * 1024 * 1024;
pub const MAX_FILE_READ: u64 = 50 * 1024 * 1024;
pub const MAX_SEARCH_QUERY: usize = 500;
pub const MAX_TOOL_OUTPUT: usize = 100_000;

pub fn validate_tool_text(value: &str, field: &str, max: usize) -> Result<()> {
    if value.len() > max {
        bail!("{field} is too large ({} bytes, max {max})", value.len());
    }
    Ok(())
}

pub fn workspace_root(workspace: &Path) -> Result<PathBuf> {
    fs::create_dir_all(workspace)
        .with_context(|| format!("creating workspace {}", workspace.display()))?;
    fs::canonicalize(workspace).with_context(|| format!("resolving {}", workspace.display()))
}

/// Resolve a user supplied path while retaining the workspace boundary.
pub fn safe_path(workspace: &Path, requested: &str) -> Result<PathBuf> {
    if requested.is_empty() {
        bail!("path is required");
    }
    let root = workspace_root(workspace)?;
    let raw = Path::new(requested);
    if raw.is_absolute() {
        bail!("absolute paths are not allowed; use a path under the workspace");
    }
    if raw
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("path traversal is not allowed");
    }
    let mut cursor = root.clone();
    for component in raw.components() {
        if let Component::Normal(part) = component {
            cursor.push(part);
            if cursor.is_symlink() {
                let target = fs::canonicalize(&cursor)
                    .with_context(|| format!("resolving symlink {}", cursor.display()))?;
                if !target.starts_with(&root) {
                    bail!("access denied: symlink escapes the workspace");
                }
            }
        }
    }
    let candidate = root.join(raw);
    let resolved = if candidate.exists() {
        fs::canonicalize(&candidate)?
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| anyhow::anyhow!("path has no parent"))?;
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        parent.join(candidate.file_name().unwrap_or_default())
    };
    if !resolved.starts_with(&root) {
        bail!("access denied: path escapes the workspace");
    }
    Ok(resolved)
}

pub fn truncate_output(value: impl Into<String>) -> String {
    let value = value.into();
    if value.len() <= MAX_TOOL_OUTPUT {
        return value;
    }
    let preview = value.chars().take(MAX_TOOL_OUTPUT).collect::<String>();
    format!(
        "{}\n\n[TRUNCATED: output exceeds {MAX_TOOL_OUTPUT} characters ({} total)]",
        preview,
        value.len()
    )
}

pub fn is_blocked_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.octets()[0] == 0
                || ip.octets()[0] == 169 && ip.octets()[1] == 254
                // Carrier-grade NAT and documentation ranges are not useful
                // fetch targets for a local web tool.
                || ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1])
        }
        IpAddr::V6(ip) => {
            ip.to_ipv4_mapped()
                .is_some_and(|mapped| is_blocked_ip(IpAddr::V4(mapped)))
                || ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.segments()[0] & 0xfe00 == 0xfc00 // fc00::/7
                || ip.segments()[0] & 0xffc0 == 0xfe80 // fe80::/10
        }
    }
}

pub fn validate_web_url(raw: &str) -> Result<url::Url> {
    let url = url::Url::parse(raw).context("invalid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http:// and https:// URLs are allowed");
    }
    if url.username() != "" || url.password().is_some() {
        bail!("URLs with embedded credentials are not allowed");
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host.eq_ignore_ascii_case("metadata.google.internal")
        || host.eq_ignore_ascii_case("instance-data.ec2.internal")
    {
        bail!("private hostname is blocked");
    }
    let _ = resolve_public_socket(&url)?;
    Ok(url)
}

/// Resolve and validate one public address. Callers should pin this address
/// in the HTTP client for the request, closing the DNS check/use gap.
pub fn resolve_public_socket(url: &url::Url) -> Result<SocketAddr> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let addresses: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolving {host}"))?
        .collect();
    if addresses.is_empty() {
        bail!("hostname has no addresses");
    }
    if let Some(blocked) = addresses.iter().find(|address| is_blocked_ip(address.ip())) {
        bail!(
            "hostname resolves to blocked address {}; refusing the request",
            blocked.ip()
        );
    }
    addresses
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("hostname has no usable address"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn blocks_private_addresses() {
        assert!(is_blocked_ip("127.0.0.1".parse().unwrap()));
        assert!(is_blocked_ip("10.1.2.3".parse().unwrap()));
        assert!(is_blocked_ip("::1".parse().unwrap()));
        assert!(is_blocked_ip("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_blocked_ip("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn path_stays_inside_workspace() {
        let directory = tempdir().unwrap();
        let path = safe_path(directory.path(), "nested/file.txt").unwrap();
        assert!(path.starts_with(workspace_root(directory.path()).unwrap()));
        assert!(safe_path(directory.path(), "../outside").is_err());
        assert!(safe_path(directory.path(), "/etc/passwd").is_err());
    }
}
