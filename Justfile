# oso Editor — Justfile
# Usage: just <recipe>  /  List all recipes: just (no arguments)

# Nightly toolchain for rustdoc JSON, used only by the public API snapshots
# (ADR 0038). cargo-public-api 0.52 needs nightly-2025-11-22 or later; CI
# pins both. `just ci` never needs nightly.
nightly := "nightly-2026-05-25"
cargo_public_api := "0.52.0"
# Snapshots are taken for one target so that they read the same on every OS.
api_target := "x86_64-unknown-linux-gnu"
api_crates := "oso-core oso-platform oso-render"
# Benchmarks build with the dev feature, so they keep out of target/release,
# where the release binary lives (ADR 0044).
perf_target := "target/perf-build"

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

# Run with standard logging (wgpu warnings, oso at debug level); `just run FILE` opens FILE
[positional-arguments]
run *args:
    RUST_LOG="wgpu=warn,oso=debug" cargo run --locked -p oso -- "$@"

# Run with verbose logging (includes wgpu GPU init logs / backtraces enabled)
[positional-arguments]
run-verbose *args:
    RUST_LOG="wgpu=debug,oso=debug" RUST_BACKTRACE=1 cargo run --locked -p oso -- "$@"

# Run a dev build with the control socket; drive it with `oso ctl` (ADR 0023)
[positional-arguments]
run-dev *args:
    RUST_LOG="wgpu=warn,oso=debug" cargo run --locked -p oso --features dev -- "$@"

# Run the release build
[positional-arguments]
run-release *args:
    RUST_LOG="wgpu=warn,oso=info" cargo run --locked -p oso --release -- "$@"

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

# Run tests, with and without dev features (the control socket)
test:
    cargo test --locked --workspace
    cargo test --locked --workspace --all-features

# Run check, lint, fmt-check, and test in sequence (equivalent to CI)
ci: check lint fmt-check test

# Fail if a release build contains dev-only code (ADR 0016, ADR 0032)
release-safety:
    #!/usr/bin/env bash
    # Each check first proves on a dev build that it can see the dev code,
    # so it cannot pass vacuously. The release side checks what `just
    # release` builds: the whole workspace, with features unified.
    set -euo pipefail
    dev_crates="serde_json miniz_oxide"
    marker="oso-dev-control-socket"
    tree() {
        cargo tree --locked --workspace --target all -e normal,build --prefix none \
            --format '{p} [{f}]' "$@" | sort -u
    }
    # Builds, and prints the path of the oso executable cargo reports.
    oso_bin() {
        cargo build --locked --message-format=json-render-diagnostics "$@" \
            | sed -n 's|.*"executable":"\([^"]*/oso\)".*|\1|p' | tail -n 1
    }
    dev_feature='^oso(-[a-z]+)? v.*\[([^]]*,)?dev(,[^]]*)?\]$'
    fail=0

    dev_tree=$(tree --features oso/dev)
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

    dev_bin=$(oso_bin -p oso --features dev)
    [ -f "$dev_bin" ] && grep -qaF "$marker" "$dev_bin" \
        || { echo "error: no dev binary with the marker $marker; fix this check"; exit 1; }
    release_bin=$(oso_bin --workspace --release)
    [ -f "$release_bin" ] \
        || { echo "error: cargo reported no release binary; fix this check"; exit 1; }
    if grep -qaF "$marker" "$release_bin"; then
        echo "error: the release binary contains the dev marker $marker"; fail=1
    fi

    if [ "$fail" -eq 0 ]; then echo "release safety: ok ($release_bin)"; fi
    exit "$fail"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Performance (ADR 0024, ADR 0025, ADR 0044)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Benchmark an optimized build with the dev feature; windows open and take the focus. `just perf OUT --quick` tries it out
perf out="target/perf/perf.json" *args="":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --bin oso --example perf
    {{perf_target}}/release/examples/perf run --oso {{perf_target}}/release/oso --out "{{out}}" {{args}}

# Compare a result with a baseline, and fail on a large regression; a missing baseline passes with a note
perf-compare base new summary="target/perf/summary.md":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --example perf
    {{perf_target}}/release/examples/perf compare "{{base}}" "{{new}}" --summary "{{summary}}"

# Time the incremental dev build of the tip crate after changing one file (median of 5)
build-time out="target/perf/build-time.json":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --example perf
    {{perf_target}}/release/examples/perf build-time --out "{{out}}"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Architecture guardrails (ADR 0038)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Update the public API snapshots in guardrails/public-api
# (needs `rustup toolchain install <nightly>`, `rustup target add` for
# api_target, and `cargo install --locked cargo-public-api@<version>`)
public-api:
    #!/usr/bin/env bash
    set -euo pipefail
    have=$(cargo public-api --version 2>/dev/null || true)
    [ "$have" = "cargo-public-api {{cargo_public_api}}" ] \
        || { echo "error: need cargo-public-api {{cargo_public_api}}, found '${have:-none}'"; exit 1; }
    for crate in {{api_crates}}; do
        cargo +{{nightly}} public-api --color never -p "$crate" --all-features -ss \
            --target {{api_target}} > "guardrails/public-api/$crate.txt"
    done
    git diff --quiet -- Cargo.lock \
        || { echo "error: generating the snapshots changed Cargo.lock"; exit 1; }

# Fail if the public API differs from the committed snapshots
public-api-check: public-api
    #!/usr/bin/env bash
    set -euo pipefail
    untracked=$(git ls-files --others --exclude-standard -- guardrails/public-api)
    if ! git diff --exit-code -- guardrails/public-api || [ -n "$untracked" ]; then
        echo "error: the public API differs from guardrails/public-api (above)."
        echo "Run 'just public-api' and commit the snapshots. A new public trait or"
        echo "module is a structural change and needs an ADR (ADR 0038)."
        exit 1
    fi
    echo "public API: matches the snapshots"

# Write the design health report (ADR 0038) as Markdown. Its clippy run
# uses a changed config, so the next `just lint` re-lints the workspace.
health-report out="target/design-health.md":
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{out}}"
    mkdir -p "$(dirname "$out")"
    tmp=$(mktemp -d)
    since=$(git rev-list -1 --before="7 days ago" HEAD || true)
    # The checkout from 7 days ago, for the growth of files and edges.
    base=
    if [ -n "$since" ]; then
        base="$tmp/base"
        git worktree add -q --detach "$base" "$since"
    fi
    trap '[ -z "$base" ] || git worktree remove --force "$base"; rm -rf "$tmp"' EXIT
    OSO_HEALTH_REPORT="$tmp/tests.md" OSO_HEALTH_BASE="$base" \
        cargo test --locked -q -p oso --test architecture \
        -- --ignored --exact design_health_report >/dev/null
    # Clippy lists every function over 40 lines. Functions with an
    # #[expect(clippy::too_many_lines)] are listed separately below.
    sed 's/^too-many-lines-threshold = .*/too-many-lines-threshold = 40/' clippy.toml >"$tmp/clippy.toml"
    { CLIPPY_CONF_DIR="$tmp" cargo clippy --locked --workspace --all-targets --all-features \
        --message-format=short -- -W clippy::too_many_lines 2>&1 || true; } \
        | sed -n 's|^\([^:]*:[0-9]*\):[0-9]*: .*too many lines (\([0-9]*\)/40).*|\2 \1|p' \
        | sort -rn | head -n 15 >"$tmp/functions"
    {
        echo "# Design health report"
        echo
        echo "Commit $(git rev-parse --short HEAD). Compared with ${since:+$(git rev-parse --short "$since"), }the state 7 days ago."
        echo
        cat "$tmp/tests.md"
        echo
        echo "## Largest functions (over 40 lines; the limit is 80)"
        echo
        echo "| Lines | Function at |"
        echo "|---:|---|"
        while read -r lines at; do echo "| $lines | \`$at\` |"; done <"$tmp/functions"
        echo
        echo "## Size exceptions"
        echo
        echo "Allowlisted files (guardrails/size-allowlist.txt):"
        echo
        grep -Ev '^[[:space:]]*(#|$)' guardrails/size-allowlist.txt | sed 's/^/- /' || echo "- none"
        echo
        echo "Functions with an \`#[expect]\` for a size or complexity lint:"
        echo
        grep -rnE 'clippy::(too_many_lines|cognitive_complexity)' crates --include='*.rs' \
            | grep -v '^crates/oso/tests/architecture' | cut -d: -f1,2 | sed 's/^/- /' || echo "- none"
        echo
        echo "## Changes in the last 7 days"
        echo
        if [ -z "$since" ]; then
            echo "No history from 7 days ago."
        else
            echo "Commits touching the crate graph or the guardrails:"
            echo
            git log --format='- %h %s' "$since"..HEAD -- Cargo.toml 'crates/*/Cargo.toml' guardrails/ \
                | grep . || echo "- none"
            echo
            echo "New packages in Cargo.lock:"
            echo
            comm -13 <(git show "$since:Cargo.lock" | grep '^name = ' | sort -u) \
                <(grep '^name = ' Cargo.lock | sort -u) | sed 's/^name = /- /' | grep . || echo "- none"
            echo
            echo "Public API size (lines in guardrails/public-api):"
            echo
            for crate in {{api_crates}}; do
                f="guardrails/public-api/$crate.txt"
                now=$(wc -l <"$f" | tr -d ' ')
                if git cat-file -e "$since:$f" 2>/dev/null; then
                    echo "- $crate: $now (was $(git show "$since:$f" | wc -l | tr -d ' '))"
                else
                    echo "- $crate: $now (new)"
                fi
            done
        fi
        echo
        echo "## Surviving mutants"
        echo
        # Hook for mutation testing (M1-9a, ADR 0037): cargo-mutants writes
        # mutants.out/missed.txt.
        if [ -f mutants.out/missed.txt ]; then
            sed 's/^/- /' mutants.out/missed.txt
        else
            echo "Not collected yet: mutation testing arrives with M1-9a (ADR 0037)."
        fi
    } >"$out"
    echo "design health report: $out"

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
