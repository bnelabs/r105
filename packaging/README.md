# Release packages

The release workflow builds the Rust binary once per target and then packages it in the native format for each operating system.

| Target | Artifact | Build environment |
| --- | --- | --- |
| Linux GNU x86_64 | r105-linux-x86_64.tar.gz | Ubuntu 22.04 |
| Linux GNU aarch64 | r105-linux-aarch64.tar.gz | cross container |
| macOS x86_64 | r105-macos-x86_64.tar.gz | macOS runner |
| macOS arm64 | r105-macos-arm64.tar.gz | macOS runner; Apple silicon |
| Windows x86_64 | r105-windows-x86_64.zip | Windows runner |
| Windows arm64 | r105-windows-arm64.zip | Windows MSVC ARM64 target |
| Ubuntu/Debian x86_64 | r105_<VERSION>_amd64.deb | dpkg-deb |
| Arch Linux x86_64 | r105-<VERSION>-1-x86_64.pkg.tar.zst | makepkg |
| Fedora/RHEL x86_64 | r105-<VERSION>-1.x86_64.rpm | rpmbuild |
| FreeBSD amd64 | r105-<VERSION>-freebsd-amd64.pkg | native FreeBSD 14 VM |
| Alpine Linux x86_64 | r105-<VERSION>-r0.apk | native Alpine 3.22 musl container |

The Ubuntu, Arch, and Fedora packages share the Linux x86_64 GNU executable. The FreeBSD and Alpine jobs compile the Rust binary on the target ABI. Alpine is allowed to be an optional best effort job because its musl toolchain can be unavailable during a GitHub hosted runner outage; the GNU and other native assets remain required.

Every archive contains the executable, README, and LICENSE. The release job publishes SHA256SUMS after all required assets are assembled.

## Local package checks

```sh
cargo build --release --locked
target/release/r105 --version
```

For a Debian package, stage the target/release/r105 binary at usr/bin/r105 and use packaging/debian/control.in. The Arch, Fedora, FreeBSD, and Alpine templates follow the same binary only layout.

The Homebrew template in docs/homebrew.rb.in selects the macOS or Linux archive by CPU architecture. The Scoop manifest selects the Windows x86_64 or arm64 archive. Release automation fills checksums after assets exist.
