# oso Editor — Justfile
# Usage: just <recipe>  /  List all recipes: just (no arguments)

# Default: show the recipe list
default:
    @just --list --unsorted

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Build
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Debug build
build:
    cargo build --locked --workspace

# Release build (with optimizations)
release:
    cargo build --locked --workspace --release

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Run
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Run with standard logging (wgpu warnings only / oso at debug level)
run:
    RUST_LOG="wgpu=warn,oso=debug" cargo run --locked -p oso

# Run with verbose logging (includes wgpu GPU init logs / backtraces enabled)
run-verbose:
    RUST_LOG="wgpu=debug,oso=debug" RUST_BACKTRACE=1 cargo run --locked -p oso

# Run the release build
run-release:
    RUST_LOG="wgpu=warn,oso=info" cargo run --locked -p oso --release

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Verify
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Type-check only (faster than a full build)
check:
    cargo check --locked --workspace

# Lint with Clippy (warnings treated as errors)
lint:
    cargo clippy --locked --workspace --all-targets -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Check formatting without modifying files (for CI)
fmt-check:
    cargo fmt --all -- --check

# Run tests
test:
    cargo test --locked --workspace

# Run check, lint, fmt-check, and test in sequence (equivalent to CI)
ci: check lint fmt-check test

# Check dependencies against deny.toml (advisories, licenses, sources, bans)
deny:
    cargo deny check

# Check Cargo.lock against the RustSec advisory database
audit:
    cargo audit

# Scan the full git history for secrets
secrets:
    gitleaks git --redact

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Docs
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Generate documentation and open it in the browser (excludes dependencies)
doc:
    cargo doc --locked --workspace --open --no-deps

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Clean
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Remove all build artifacts
clean:
    cargo clean
