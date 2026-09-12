#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TAG=
if [ "$#" -ge 2 ] && [ "$1" = "--tag" ]; then
  TAG=$2
fi

VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)
if [ -z "$VERSION" ]; then
  echo "Cargo.toml has no package version" >&2
  exit 1
fi

LOCK_VERSION=$(awk '
  /^\[\[package\]\]/ { in_package = 1; name = ""; version = "" }
  in_package && /^name = "r105"$/ { name = "r105" }
  in_package && /^version = / { version = $0 }
  in_package && name == "r105" && version != "" {
    print version
    exit
  }
' "$ROOT/Cargo.lock" | sed -n 's/version = "\([^"]*\)"/\1/p')
if [ "$LOCK_VERSION" != "$VERSION" ]; then
  echo "version mismatch: Cargo.toml=$VERSION, Cargo.lock=$LOCK_VERSION" >&2
  exit 1
fi

if ! grep -Fq "## [$VERSION]" "$ROOT/CHANGELOG.md"; then
  echo "CHANGELOG.md has no release section for $VERSION" >&2
  exit 1
fi

if [ -n "$TAG" ]; then
  TAG_VERSION=$(printf '%s' "$TAG" | sed 's/^v//')
  if [ "$TAG_VERSION" != "$VERSION" ]; then
    echo "tag version mismatch: tag=$TAG_VERSION, project=$VERSION" >&2
    exit 1
  fi
fi

echo "release metadata is consistent for $VERSION"
