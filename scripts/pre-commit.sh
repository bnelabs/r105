#!/bin/sh
# Pre-commit hook for r105
# Install this as .git/hooks/pre-commit to enable automatic checks
# Usage: cp scripts/pre-commit.sh .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit

set -e

echo "Running pre-commit checks..."

# Check formatting
echo "Checking formatting..."
cargo fmt --all -- --check

# Run clippy
echo "Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings

# Run tests
echo "Running tests..."
cargo test --all-targets

echo "All checks passed!"