# Release packages

The release workflow builds the current x86_64 binary for each target package
format. The Linux binary is built on Ubuntu 22.04 so the glibc baseline is
usable on current Ubuntu, Arch, and Fedora systems.

| Target | Artifact | Build environment |
| --- | --- | --- |
| Ubuntu/Debian | `r105_VERSION_amd64.deb` | `dpkg-deb` on Ubuntu 22.04 |
| Arch Linux | `r105-VERSION-1-x86_64.pkg.tar.zst` | `makepkg` in Arch Linux |
| Fedora/RHEL-like | `r105-VERSION-1.x86_64.rpm` | `rpmbuild` on Ubuntu 22.04 |
| FreeBSD | `r105-VERSION-freebsd-amd64.pkg` | native PyInstaller and `pkg create` in FreeBSD 14 |
| Alpine Linux | `r105-VERSION-r0.apk` | native musl PyInstaller and `abuild` in Alpine 3.22 |

Standalone executables are distributed as `tar.gz` archives for Linux x86_64
and macOS x86_64/ARM64, plus `zip` archives for Windows x86_64/ARM64. The
archive contains the executable, README, and license so the file type is clear
and the unpacked binary is still named `r105` or `r105.exe`.

The package recipes use `@VERSION@` placeholders. The release workflow fills
them from the `vX.Y.Z` tag and validates that the tag, `pyproject.toml`, and
`r105.__version__` agree before any artifact is published.
