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
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp "$root/target/release/r105" "$bundle/Contents/MacOS/r105"
iconset="$bundle/Contents/Resources/r105.iconset"
mkdir -p "$iconset"
for spec in \
  '16 icon_16x16' '32 icon_16x16@2x' \
  '32 icon_32x32' '64 icon_32x32@2x' \
  '128 icon_128x128' '256 icon_128x128@2x' \
  '256 icon_256x256' '512 icon_256x256@2x' \
  '512 icon_512x512' '1024 icon_512x512@2x'; do
  size="${spec%% *}"
  name="${spec#* }"
  sips -z "$size" "$size" "$root/assets/r105-icon.png" \
    --out "$iconset/$name.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/r105.icns"
rm -rf "$iconset"
version="$($root/target/release/r105 --version | awk '{print $2}')"
sed "s/@VERSION@/$version/g" "$root/packaging/macos/Info.plist.in" > "$bundle/Contents/Info.plist"
plutil -lint "$bundle/Contents/Info.plist"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"
printf 'Local app: %s\n' "$bundle"
printf 'Ad-hoc signed for local use; public distribution still requires Developer ID signing and notarization.\n'
