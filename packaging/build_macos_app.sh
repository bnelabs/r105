#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != Darwin ]]; then
  echo "Build the macOS app on macOS." >&2
  exit 1
fi
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
cargo build --release --locked
bundle="$root/target/r105.app"
mkdir -p "$bundle/Contents/MacOS"
cp "$root/target/release/r105" "$bundle/Contents/MacOS/r105"
version="$($root/target/release/r105 --version | awk '{print $2}')"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist.in" > "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
printf 'Local app: %s\n' "$bundle"
printf 'Ad-hoc signed for local use; public distribution still requires Developer ID signing and notarization.\n'
