# Security Policy

## Supported Versions

Only the latest released version of r105 is actively supported with security updates. Users are strongly encouraged to upgrade to the newest version.

## Reporting a Vulnerability

If you discover a security vulnerability, please report it privately rather than creating a public issue.

### How to Report

1. **Email**: Send a detailed report to security@bnelabs.com
2. **GitHub Security Advisory**: Use the [GitHub Security Advisory](https://github.com/bnelabs/r105/security/advisories) feature

### What to Include

Please include the following information in your report:

- A description of the vulnerability
- Steps to reproduce the issue
- Potential impact of the vulnerability
- Any suggested mitigation or fix

### Response Timeline

- **Initial Response**: Within 48 hours
- **Investigation**: Within 1 week
- **Fix Release**: As soon as feasible, based on severity

### Security Best Practices

r105 follows several security best practices:

- **Workspace Containment**: All file operations are restricted to the configured workspace directory
- **Network Security**: Web tools only allow HTTP/HTTPS, reject private addresses, and validate redirects
- **Sandboxing**: Rust code execution uses OS-level isolation (nsjail, bubblewrap, Docker) when available
- **Input Validation**: All tool inputs are validated and bounded
- **No Credential Storage**: API keys are never written to disk; they're held in memory only
- **Audit Trail**: Session files record all tool calls and responses

### Dependency Security

- Dependencies are audited using `cargo-audit` in CI
- Vulnerability reports are reviewed and addressed promptly
- Updates are only applied after thorough testing

### Threat Model

r105 is designed with the following threat model in mind:

- **Malicious Models**: The model may attempt to escape containment, access sensitive data, or execute arbitrary code
- **Compromised Backends**: The backend provider may return malicious content or attempt to exploit the client
- **Network Attacks**: Network requests may be intercepted, redirected, or manipulated
- **File System Attacks**: File operations may attempt path traversal, symlink escapes, or access sensitive files

The security measures in r105 are designed to mitigate these threats by:

- Enforcing strict workspace boundaries
- Validating all network requests and responses
- Using sandboxed execution for untrusted code
- Bounding all inputs and outputs
- Never persisting sensitive credentials

### Disclosure Policy

After a vulnerability is fixed:

- A security advisory will be published
- The fix will be included in the next release
- Users will be notified via GitHub releases and changelog
- For critical vulnerabilities, coordinated disclosure may be used to give users time to update

### Security Questions

For general security questions or concerns that don't involve a specific vulnerability, please open a GitHub issue with the `security` label.