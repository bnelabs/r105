#!/bin/sh
# Development helper script for r105
# This script provides common development workflows

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_ROOT"

case "$1" in
    check)
        echo "Running all checks..."
        cargo fmt --all -- --check
        cargo check --locked --all-targets --all-features
        cargo clippy --locked --all-targets --all-features -- -D warnings
        cargo test --locked --all-targets
        ./packaging/check_release.sh
        echo "All checks passed!"
        ;;
    
    build)
        echo "Building release version..."
        cargo build --release --locked
        echo "Build complete: target/release/r105"
        ;;
    
    test)
        echo "Running tests..."
        cargo test --locked --all-targets
        echo "Tests passed!"
        ;;
    
    fmt)
        echo "Formatting code..."
        cargo fmt --all
        echo "Formatting complete!"
        ;;
    
    fmt-check)
        echo "Checking formatting..."
        cargo fmt --all -- --check
        echo "Formatting check passed!"
        ;;
    
    clippy)
        echo "Running clippy..."
        cargo clippy --locked --all-targets --all-features -- -D warnings
        echo "Clippy passed!"
        ;;
    
    audit)
        echo "Running security audit..."
        cargo audit
        echo "Security audit passed!"
        ;;
    
    outdated)
        echo "Checking for outdated dependencies..."
        cargo outdated || true
        ;;
    
    clean)
        echo "Cleaning build artifacts..."
        cargo clean
        echo "Clean complete!"
        ;;
    
    dev)
        echo "Starting development build..."
        cargo run -- chat
        ;;
    
    *)
        echo "Usage: $0 {check|build|test|fmt|fmt-check|clippy|audit|outdated|clean|dev}"
        echo ""
        echo "Commands:"
        echo "  check       - Run all checks (fmt, clippy, test, release metadata)"
        echo "  build       - Build release version"
        echo "  test        - Run tests"
        echo "  fmt         - Format code"
        echo "  fmt-check   - Check formatting"
        echo "  clippy      - Run clippy linter"
        echo "  audit       - Run security audit"
        echo "  outdated    - Check for outdated dependencies"
        echo "  clean       - Clean build artifacts"
        echo "  dev         - Start development TUI"
        exit 1
        ;;
esac
