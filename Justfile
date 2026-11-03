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

# Run a dev build with the control socket; drive it with `oso ctl` (ADR 0023)
run-dev:
    RUST_LOG="wgpu=warn,oso=debug" cargo run --locked -p oso --features dev

# Run the release build
run-release:
    RUST_LOG="wgpu=warn,oso=info" cargo run --locked -p oso --release

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Verify
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Type-check only (faster than a full build), with and without dev features
check:
    cargo check --locked --workspace
    cargo check --locked --workspace --all-targets --all-features

# Lint with Clippy (warnings treated as errors), with and without dev features
lint:
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# Format code
fmt:
    cargo fmt --all

# Check formatting without modifying files (for CI)
fmt-check:
    cargo fmt --all -- --check

# Run tests, including dev-feature code (the control socket)
test:
    cargo test --locked --workspace --all-features

# Run check, lint, fmt-check, and test in sequence (equivalent to CI)
ci: check lint fmt-check test

# Fail if a release build contains dev-only code (ADR 0016, ADR 0031)
release-safety:
    #!/usr/bin/env bash
    # Each check first proves on a dev build that it can see the dev code,
    # so it cannot pass vacuously.
    set -euo pipefail
    dev_crates="serde_json miniz_oxide"
    marker="oso-dev-control-socket"
    target="${CARGO_TARGET_DIR:-target}"
    tree() {
        cargo tree --locked -p oso --target all -e normal,build --prefix none \
            --format '{p} [{f}]' "$@" | sort -u
    }
    dev_feature='^oso(-[a-z]+)? v.*\[([^]]*,)?dev(,[^]]*)?\]$'
    fail=0

    dev_tree=$(tree --features dev)
    release_tree=$(tree)
    grep -Eq "$dev_feature" <<<"$dev_tree" \
        || { echo "error: the dev tree shows no dev feature; fix this check"; exit 1; }
    if grep -E "$dev_feature" <<<"$release_tree"; then
        echo "error: the release build enables a dev feature (above)"; fail=1
    fi
    for crate in $dev_crates; do
        grep -q "^$crate v" <<<"$dev_tree" \
            || { echo "error: $crate is not in the dev tree; fix this check"; exit 1; }
        if grep -q "^$crate v" <<<"$release_tree"; then
            echo "error: the release build depends on the dev-only crate $crate"; fail=1
        fi
    done

    cargo build --locked -p oso --features dev
    grep -qaF "$marker" "$target/debug/oso" \
        || { echo "error: the dev binary lacks the marker $marker; fix this check"; exit 1; }
    cargo build --locked -p oso --release
    if grep -qaF "$marker" "$target/release/oso"; then
        echo "error: the release binary contains the dev marker $marker"; fail=1
    fi

    if [ "$fail" -eq 0 ]; then echo "release safety: ok"; fi
    exit "$fail"

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
