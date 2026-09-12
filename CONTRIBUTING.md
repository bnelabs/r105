# Contributing to r105

r105 is a Rust native terminal AI harness. The contribution path is intentionally small: one Cargo project, one native binary, and platform packaging driven by GitHub Actions.

## Development setup

Install a stable Rust toolchain with rustup or your operating system package manager:

```sh
git clone https://github.com/bnelabs/r105.git
cd r105
cargo check
```

An OpenAI compatible backend is needed for live model, streaming, and tool loop checks. Unit tests run without a backend.

## Local checks

Run the same checks used by CI:

```sh
cargo fmt --all -- --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

Build and smoke test the executable:

```sh
cargo build --release
target/release/r105 --version
target/release/r105 --help
target/release/r105 config-schema
```

A local mock that returns an OpenAI compatible JSON response can validate the send path. A live provider validates SSE and tool execution.

## Source layout

- src/backend.rs contains the authoritative backend interface and HTTP/SSE implementation.
- src/provider.rs contains connection presets and credential lookup.
- src/ui.rs contains TUI orchestration and focused overlays.
- src/command.rs contains the slash command registry and scrolling visibility helper.
- src/tool.rs contains native tool schemas, dispatch, and bounded arithmetic.
- src/security.rs and src/sandbox.rs define the execution boundaries.
- src/session.rs and src/config.rs own durable formats and atomic writes.
- src/plugin.rs and src/mcp.rs define local extension protocols.
- src/python_bridge.rs defines the optional external Python compatibility protocol;
  the reference implementation lives in bridge/ and is not bundled into releases.

Keep responsibilities in their module. Add a small helper when it improves a boundary, then add a focused test for the behavior it protects.

## Adding a slash command

1. Add a CommandSpec to COMMANDS in src/command.rs.
2. Add the handler branch in UiApp::handle_command or a focused helper in src/ui.rs.
3. Describe the command in the README.
4. Add parser or state tests when the command changes durable behavior.

The command palette must keep keyboard selection visible. Use command::ensure_visible for any new picker or list.

## Adding a tool

1. Add a schema to builtin_definitions in src/tool.rs.
2. Add a bounded handler to execute.
3. Route filesystem work through safe_path and output through truncate_output.
4. Route public web requests through validate_web_url and resolve_public_socket.
5. Use ToolContext cancellation for operations that can wait.
6. Add a meaningful unit test.

Native Rust execution must continue through Sandbox. Do not add eval, exec, or an unbounded subprocess path.

## Compatibility rules

The Rust loader continues to read the 0.8.x config and session JSON shape. Keep serde defaults for fields that may be absent, and add a migration path when changing the session version.

Credentials must stay out of config and session files. Provider metadata can be persisted, but API keys are read from the environment or held only in memory by the guided connection flow.

The shipped runtime and release packages are Rust only. Python plugins and
Python optional dependencies are not embedded or installed. Existing Python
workflows may use the explicitly configured, approval gated external bridge;
keep that compatibility layer process based and versioned.

## Packaging

The release workflow builds:

- Linux GNU x86_64 and aarch64 archives;
- macOS x86_64 and arm64 archives;
- Windows x86_64 and arm64 archives;
- Ubuntu/Debian deb, Arch pkg.tar.zst, Fedora rpm;
- FreeBSD amd64 pkg;
- Alpine x86_64 apk.

Package recipes are in packaging/. Release archives are target specific and include README, LICENSE, and SHA256SUMS at publication time.

## Pull requests

- Keep the change focused.
- Explain user visible behavior and compatibility impact.
- Update CHANGELOG.md under Unreleased.
- Run formatting, check, clippy, and tests.
- Do not commit target/, build/, dist/, caches, credentials, or local config files.

## Release process

Update the version in Cargo.toml and move the matching Unreleased notes into a dated section in CHANGELOG.md. Validate the metadata:

```sh
./packaging/check_release.sh --tag v1.0.0
```

Commit the release, push the branch, merge it to main, and push the tag:

```sh
git tag -a v1.0.0 -m "r105 v1.0.0"
git push origin main --follow-tags
```

The tag workflow builds and publishes the native matrix, package formats, checksums, and release notes. Homebrew and Scoop metadata are synchronized after the release assets are available.
