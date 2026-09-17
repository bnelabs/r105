# Contributing to r105

r105 is a Rust-native terminal AI harness. The contribution path is
intentionally small: one Cargo project, one native binary, and platform
packaging driven by GitHub Actions. Keep user-facing documentation neutral:
describe behavior and interfaces directly, without comparative product or
style references.

## Development setup

Install a stable Rust toolchain with rustup or your operating system package manager:

```sh
git clone https://github.com/bnelabs/r105.git
cd r105
cargo check
```

An OpenAI-compatible backend is needed for live model, streaming, and tool-loop
checks. Unit tests, assistant lifecycle tests, and window smoke tests run
without a backend.

## Local checks

Run the same checks used by CI:

```sh
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
./packaging/check_release.sh
```

Build and smoke test the executable:

```sh
cargo build --release --locked
target/release/r105 --version
target/release/r105 --help
target/release/r105 config-schema
target/release/r105 window --smoke 60
target/release/r105 window --smoke 90 --smoke-chrome
```

A local mock that returns an OpenAI-compatible JSON/SSE response can validate
the send path and assistant lifecycle. A live provider validates real SSE,
tool execution, approval verdicts, and model-specific behavior.

## Source layout

- src/backend.rs contains the authoritative backend interface and HTTP/SSE implementation.
- src/provider.rs contains connection presets and credential lookup.
- src/ui/ contains TUI orchestration, panes, tabs, overlays, completion, and rendering.
- src/assistant.rs contains the headless native-window prompt, approval, tool, and checkpoint loop.
- src/window.rs and src/window/ contain the native GPU terminal, AI chrome, selection, clipboard, and IME paths.
- src/terminal.rs contains the shared PTY, vt100 screen, shell markers, and command blocks.
- src/command.rs contains the slash command registry and scrolling visibility helper.
- src/tool.rs contains native tool schemas, dispatch, and bounded arithmetic.
- src/security.rs and src/sandbox.rs define the execution boundaries.
- src/session.rs and src/config.rs own durable formats and atomic writes.
- src/plugin.rs and src/mcp.rs define local extension protocols.

Keep responsibilities in their module. Add a small helper when it improves a boundary, then add a focused test for the behavior it protects.

## Adding a slash command

1. Add a CommandSpec to COMMANDS in src/command.rs.
2. Add the handler branch in `UiApp` or a focused helper in the relevant `src/ui/` module.
3. Describe the command in the README.
4. Update `docs/TOOLS.md` or `docs/CONFIGURATION.md` when the command changes a protocol or setting.
5. Add parser or state tests when the command changes durable behavior.

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

The Rust loader continues to read the 0.8.x config and session JSON shape.
Keep serde defaults for fields that may be absent, and add a migration path
when changing the session version. Tab files also need normalization for
legacy single-pane entries and invalid layouts.

Credentials must stay out of config and session files. Provider metadata can be persisted, but API keys are read from the environment or held only in memory by the guided connection flow.

The shipped runtime and release packages are Rust only. Python runtimes,
Python plugins, and optional Python dependencies are not embedded or installed.
Do not add a new Python bridge or a second execution path; native tools and
the configured sandbox are the supported extension boundary.

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
- Update `CHANGELOG.md` under Unreleased and the relevant README or docs page.
- If a spec-sized change is planned, add or amend `docs/specs/NNNN-*.md`.
- For TUI changes, test keyboard, mouse, scrolling, and pane/tab persistence.
- For native-window changes, run both smoke modes and capture a PPM when layout changes.
- Run formatting, locked check, Clippy, tests, and release metadata validation.
- Do not commit target/, build/, dist/, caches, credentials, or local config files.

## Release process

Update the version in `Cargo.toml`, refresh `Cargo.lock`, move the matching
Unreleased notes into a dated section in `CHANGELOG.md`, and add release notes
at `docs/release-notes/vX.Y.Z.md`. Validate the metadata:

```sh
./packaging/check_release.sh --tag vX.Y.Z
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
target/release/r105 window --smoke 60
target/release/r105 window --smoke 90 --smoke-chrome
```

Commit the release, push the branch, and push the annotated tag:

```sh
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push origin main vX.Y.Z
```

The tag workflow builds and publishes the native matrix, package formats,
checksums, and release notes. Homebrew and Scoop metadata are synchronized
after the release assets are available. Verify the published asset list,
SHA256SUMS, release body, and metadata commit before calling the release done.
