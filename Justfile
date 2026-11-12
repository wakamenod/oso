# oso Editor — Justfile
# Usage: just <recipe>  /  List all recipes: just (no arguments)

# Nightly toolchain for rustdoc JSON, used only by the public API snapshots
# (ADR 0038). cargo-public-api 0.52 needs nightly-2025-11-22 or later; CI
# pins both. `just ci` never needs nightly.
nightly := "nightly-2026-05-25"
cargo_public_api := "0.52.0"
# Snapshots are taken for one target so that they read the same on every OS.
api_target := "x86_64-unknown-linux-gnu"
api_crates := "oso-core oso-platform oso-render oso-syntax"
# oso's own macOS window layer exists only there, so oso-platform also has a snapshot
# taken for macOS, on macOS (ADR 0070).
api_macos_target := "aarch64-apple-darwin"
# Benchmarks build with the dev feature, so they keep out of target/release,
# where the release binary lives (ADR 0046).
perf_target := "target/perf-build"
# `perf-remote` builds both sides here, apart from those, with the features asked for, the
# base's own if given.
perf_remote_target := "target/perf-remote"
# The whole test run stays within this on the author's Mac (ADR 0037).
test_budget_s := "30"
# The design health report lists this many surviving mutants.
mutants_listed := "100"

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

# Run a dev build and restart it on source changes, keeping its state (ADR 0023)
[positional-arguments]
dev *args:
    RUST_LOG="wgpu=warn,oso=info" cargo run --locked -p oso --features dev -- dev "$@"

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

# Lint for Windows from any OS, before every push. The grammars' C is not compiled here:
# `cc` gets the stubs below, so only Rust is checked, and CI's Windows job builds the C
# (ADR 0111). The `std::os::unix::net` entries of clippy.toml warn there, as expected.
clippy-windows:
    #!/usr/bin/env bash
    set -euo pipefail
    stubs=$(just c-stubs)
    CC_x86_64_pc_windows_msvc="$stubs/cc" AR_x86_64_pc_windows_msvc="$stubs/ar" \
        cargo clippy --locked --workspace --all-targets --all-features \
        --target x86_64-pc-windows-msvc -- -D warnings

# Write a C compiler and an archiver that compile nothing, for checks of a target whose C
# headers this machine lacks, and print their directory (ADR 0111). Each writes empty files
# where it is told to write, so build scripts that compile C finish and nothing links them.
c-stubs:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="$PWD/target/c-stubs"
    mkdir -p "$dir"
    printf '%s\n' '#!/bin/sh' \
        'while [ $# -gt 0 ]; do case "$1" in -o) shift; : > "$1";; -Fo*) : > "${1#-Fo}";; esac; shift; done' \
        > "$dir/cc"
    printf '%s\n' '#!/bin/sh' \
        'for a in "$@"; do case "$a" in -out:*) printf "!<arch>\n" > "${a#-out:}"; exit 0;; *.a|*.lib) printf "!<arch>\n" > "$a"; exit 0;; esac; done' \
        > "$dir/ar"
    chmod +x "$dir/cc" "$dir/ar"
    echo "$dir"

# Format code
fmt:
    cargo fmt --all

# Check formatting without modifying files (for CI)
fmt-check:
    cargo fmt --all -- --check

# Nothing here accepts a snapshot: a new, changed or pending one fails (ADR 0045).
# Run tests, with and without dev features (the control socket)
test: snapshots-pending
    INSTA_UPDATE=no INSTA_FORCE_PASS=0 cargo test --locked --workspace
    INSTA_UPDATE=no INSTA_FORCE_PASS=0 cargo test --locked --workspace --all-features

# Fail if a snapshot is waiting for review. git also lists ignored files here.
snapshots-pending:
    @if [ -n "$(git ls-files --cached --others -- 'crates/*.snap.new' 'crates/*.pending-snap')" ]; then echo "error: pending snapshots; run 'just snapshots' or delete them:"; git ls-files --cached --others -- 'crates/*.snap.new' 'crates/*.pending-snap'; exit 1; fi

# Rewrite the text-grid snapshots; review the result with `git diff`, like code
snapshots:
    INSTA_UPDATE=always INSTA_FORCE_PASS=0 cargo test --locked -p oso grid_tests

# Run check, lint, fmt-check, and test in sequence (equivalent to CI)
ci: check lint fmt-check test

# Time the whole test run, built beforehand, and warn past the limit (ADR 0037, ADR 0050)
test-time limit=test_budget_s:
    #!/usr/bin/env bash
    # Failing tests fail this. Being slow only warns. The first run is there to
    # build, so that the number leaves out compiling.
    set -euo pipefail
    {{just_executable()}} test >/dev/null 2>&1 || true
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    # `time` is bash's keyword: macOS's date has no fractions of a second.
    TIMEFORMAT=%R
    if ! { time {{just_executable()}} test >"$tmp/log" 2>&1; } 2>"$tmp/time"; then
        cat "$tmp/log"
        exit 1
    fi
    took=$(tail -n 1 "$tmp/time")
    echo "test time: ${took} s (limit {{limit}} s; the budget is {{test_budget_s}} s on the author's Mac)"
    if awk -v took="$took" -v limit="{{limit}}" 'BEGIN { exit !(took + 0 > limit + 0) }'; then
        message="the test run took ${took} s, past the limit of {{limit}} s; the budget is {{test_budget_s}} s on the author's Mac (ADR 0037)"
        if [ -n "${GITHUB_ACTIONS:-}" ]; then echo "::warning::$message"; else echo "warning: $message"; fi
    fi

# Fail if a release build contains dev-only code (ADR 0016, ADR 0032)
release-safety:
    #!/usr/bin/env bash
    # Each check first proves on a dev build that it can see the dev code,
    # so it cannot pass vacuously. The release side checks what `just
    # release` builds: the whole workspace, with features unified.
    set -euo pipefail
    # serde_json ships for the agent channel (ADR 0085).
    dev_crates="miniz_oxide"
    # Dev method names too: with serde_json shipping, they show that no dev method got into
    # the agent channel (ADR 0085). `oso dev`'s supervisor has a marker of its own, as
    # spawning ships (ADR 0120).
    markers="oso-dev-control-socket oso-dev-supervisor send-keys export-state"
    # On macOS, the window layer's dev-only Carbon import, for the dev socket's secure input
    # check (ADR 0070, ADR 0073).
    if [ "$(uname -s)" = Darwin ]; then markers="$markers IsSecureEventInputEnabled"; fi
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
    release_bin=$(oso_bin --workspace --release)
    [ -f "$release_bin" ] \
        || { echo "error: cargo reported no release binary; fix this check"; exit 1; }
    for marker in $markers; do
        [ -f "$dev_bin" ] && grep -qaF "$marker" "$dev_bin" \
            || { echo "error: no dev binary with the marker $marker; fix this check"; exit 1; }
        if grep -qaF "$marker" "$release_bin"; then
            echo "error: the release binary contains the dev marker $marker"; fail=1
        fi
    done

    if [ "$fail" -eq 0 ]; then echo "release safety: ok ($release_bin)"; fi
    exit "$fail"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# End-to-end tests (ADR 0037, ADR 0049)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Drive real dev builds over the control socket; windows open and take the focus. `just e2e 20` repeats the suite to look for flakiness
e2e runs="1":
    #!/usr/bin/env bash
    set -euo pipefail
    [[ "{{runs}}" =~ ^[0-9]+$ ]] || { echo "error: not a number of runs: {{runs}}"; exit 1; }
    export RUST_LOG=oso=debug,wgpu=warn
    for run in $(seq 1 {{runs}}); do
        cargo test --locked -p oso --features dev --test e2e -- --ignored --test-threads=1 \
            || { echo "e2e: run $run of {{runs}} failed"; exit 1; }
    done

# Draw panes on a real GPU and read the pixels (ADR 0061); needs no window and no text backend
pixel-tests:
    cargo test --locked -p oso-render -- --ignored

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Performance (ADR 0024, ADR 0025, ADR 0046)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Benchmark an optimized build with the dev feature; windows open and take the focus. `just perf OUT --quick` tries it out
perf out="target/perf/perf.json" *args="":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --bin oso --example perf
    {{perf_target}}/release/examples/perf run --oso {{perf_target}}/release/oso --out "{{out}}" {{args}}

# Benchmark the working tree against BASE on the ssh host OSO_PERF_HOST, in PAIRS alternating pairs, built with FEATURES, the base with BASE_FEATURES if given. It builds here and only runs there, so builds here may go on meanwhile
perf-remote base="origin/main" pairs="3" features="dev" base_features="" *args="":
    #!/usr/bin/env bash
    set -euo pipefail
    host="${OSO_PERF_HOST:?set OSO_PERF_HOST to an ssh host, a Mac that runs what is built here}"
    [[ "{{pairs}}" =~ ^[1-9][0-9]*$ ]] || { echo "error: not a number of pairs: {{pairs}}"; exit 1; }
    # The base is built with `base_features` if given, as winit against the native layer is.
    base_features="{{ if base_features == "" { features } else { base_features } }}"
    for f in "{{features}}" "$base_features"; do
        [[ "$f" =~ ^dev(,[a-z-]+)*$ ]] || { echo "error: features must start with dev: $f"; exit 1; }
    done
    rev=$(git rev-parse --verify "{{base}}^{commit}")
    new=$(git rev-parse --short HEAD)
    git diff --quiet HEAD || new="$new with changes"
    run=$(date +%Y%m%d-%H%M%S)
    out="target/perf/remote/$run"
    src="{{perf_remote_target}}/base-src"
    mkdir -p "$out"
    rm -rf "$src" && mkdir -p "$src"
    # The base from git alone, built apart; the harness of the working tree runs both sides.
    git archive "$rev" | tar -x -C "$src"
    cargo build --locked --release --manifest-path "$src/Cargo.toml" --target-dir "$PWD/{{perf_remote_target}}/base" -p oso --features "$base_features" --bin oso
    cargo build --locked --release --target-dir {{perf_remote_target}}/new -p oso --features "{{features}}" --bin oso --example perf
    echo "base: {{base}} at ${rev:0:7} with $base_features; new: $new with {{features}}; host: $host" | tee "$out/what.txt"
    ssh "$host" "mkdir -p oso-perf/$run"
    rsync -q "{{perf_remote_target}}/base/release/oso" "$host:oso-perf/$run/oso-base"
    rsync -q "{{perf_remote_target}}/new/release/oso" "$host:oso-perf/$run/oso-new"
    rsync -q "{{perf_remote_target}}/new/release/examples/perf" "$host:oso-perf/$run/perf"
    # Base first in each pair. Swapping during a run makes it worth less, so each says how much.
    ssh "$host" bash -s -- "$run" "{{pairs}}" {{args}} <<'REMOTE' | tee -a "$out/what.txt"
    set -euo pipefail
    cd "oso-perf/$1"
    pairs=$2
    shift 2
    swapouts() { vm_stat | awk '/Swapouts/ { gsub(/\./, "", $2); print $2 }'; }
    for pair in $(seq 1 "$pairs"); do
        for side in base new; do
            before=$(swapouts)
            caffeinate -d ./perf run --oso "./oso-$side" --out "$side-$pair.json" "$@" > "$side-$pair.log" 2>&1 \
                || { echo "perf-remote: $side of pair $pair failed"; tail -20 "$side-$pair.log"; exit 1; }
            echo "pair $pair, $side: $(( $(swapouts) - before )) swapouts"
        done
    done
    REMOTE
    scp -q "$host:oso-perf/$run/*.json" "$host:oso-perf/$run/*.log" "$out/"
    ssh "$host" "rm -rf oso-perf/$run"
    status=0
    for pair in $(seq 1 {{pairs}}); do
        echo "━━ pair $pair"
        {{perf_remote_target}}/new/release/examples/perf compare "$out/base-$pair.json" "$out/new-$pair.json" \
            --summary "$out/summary-$pair.md" || status=1
    done
    echo "results in $out"
    exit $status

# Compare a result with a baseline, and fail on a large regression; a missing baseline passes with a note
perf-compare base new summary="target/perf/summary.md":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --example perf
    {{perf_target}}/release/examples/perf compare "{{base}}" "{{new}}" --summary "{{summary}}"

# Time the incremental dev build of the tip crate after changing one file (median of 5)
build-time out="target/perf/build-time.json":
    cargo build --locked --release --target-dir {{perf_target}} -p oso --features dev --example perf
    {{perf_target}}/release/examples/perf build-time --out "{{out}}"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Test quality (ADR 0037, ADR 0050)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Mutation-test oso-core (needs cargo-mutants); surviving mutants are listed in mutants.out/missed.txt
[positional-arguments]
mutants *args:
    #!/usr/bin/env bash
    # 2 and 3 say that mutants survived or timed out, which is what a run is
    # for. Anything else, such as tests failing before any mutation, is an error.
    status=0
    cargo mutants --package oso-core --no-shuffle --colors never "$@" || status=$?
    case "$status" in 0|2|3) exit 0 ;; *) exit "$status" ;; esac

# List entries of exclude_re in .cargo/mutants.toml that match no mutant, or more than the one they should; fails if there are any
mutants-exclusions:
    #!/usr/bin/env bash
    set -euo pipefail
    mutants=$(cargo mutants --list --package oso-core --no-config) \
        || { echo "cargo mutants --list failed, so the entries were not checked"; exit 1; }
    fail=0
    while IFS= read -r pattern; do
        count=$(grep -Ec -- "$pattern" <<<"$mutants" || true)
        case "$pattern" in
            *:\[0-9\]+:*) want=1 ;;
            *) want=$count; [ "$count" -gt 0 ] || want=any ;;
        esac
        if [ "$count" != "$want" ]; then
            echo "\`$pattern\` matches $count mutants"
            fail=1
        fi
    done < <(sed -n 's/^ *"\(.*\)",$/\1/p' .cargo/mutants.toml | sed 's/\\\\/\\/g')
    exit "$fail"

# Write a coverage summary as Markdown (needs cargo-llvm-cov and `rustup component add llvm-tools`)
coverage out="target/coverage/summary.md":
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{out}}"
    json="$(dirname "$out")/coverage.json"
    mkdir -p "$(dirname "$out")"
    INSTA_UPDATE=no INSTA_FORCE_PASS=0 cargo llvm-cov --locked --workspace --all-features \
        --json --summary-only --output-path "$json"
    {
        echo "# Coverage"
        echo
        echo "Lines, functions and regions that \`cargo test\` runs, with the dev features, on $(uname -s). It is a"
        echo "map of what no test reaches, not a target (ADR 0037). The window, the event loop and the GPU cannot run"
        echo "here, so \`crates/oso\` and \`crates/oso-render\` read low. Code for other OSes is not counted at all,"
        echo "and neither are the end-to-end tests. Look at \`crates/oso-core\` first."
        echo
        echo "| Crate | Lines | Functions | Regions |"
        echo "|---|---:|---:|---:|"
        jq -r '
            def pct($kind): (map(.summary[$kind].covered) | add) as $hit
                | (map(.summary[$kind].count) | add) as $all
                | if $all == 0 then "-" else "\(($hit * 1000 / $all | round) / 10)%" end;
            .data[0].files
            | map(. + {crate: (.filename | capture("crates/(?<c>[^/]+)/").c)})
            | group_by(.crate)[]
            | "| \(.[0].crate) | \(pct("lines")) | \(pct("functions")) | \(pct("regions")) |"
        ' "$json"
        echo
        echo "<details><summary>By file, least covered first</summary>"
        echo
        echo "| File | Lines | Not run |"
        echo "|---|---:|---:|"
        jq -r '.data[0].files | sort_by(.summary.lines.percent)[]
            | "| \(.filename | sub(".*/crates/"; "crates/")) | \(.summary.lines.percent * 10 | round / 10)% | \(.summary.lines.count - .summary.lines.covered) |"' "$json"
        echo
        echo "</details>"
    } >"$out"
    echo "coverage: $out"

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
    # oso-syntax's grammars compile C, for which another OS has no Linux headers (ADR 0111).
    if [ "$(uname -s)" != Linux ]; then
        stubs=$(just c-stubs)
        export CC_x86_64_unknown_linux_gnu="$stubs/cc" AR_x86_64_unknown_linux_gnu="$stubs/ar"
    fi
    for crate in {{api_crates}}; do
        cargo +{{nightly}} public-api --color never -p "$crate" --all-features -ss \
            --target {{api_target}} > "guardrails/public-api/$crate.txt"
    done
    git diff --quiet -- Cargo.lock \
        || { echo "error: generating the snapshots changed Cargo.lock"; exit 1; }
    if [ "$(uname -s)" = Darwin ]; then just public-api-macos; fi

# Update oso-platform's macOS snapshot, which holds the native window layer (ADR 0070).
# Needs macOS, as its build compiles for it.
public-api-macos:
    #!/usr/bin/env bash
    set -euo pipefail
    have=$(cargo public-api --version 2>/dev/null || true)
    [ "$have" = "cargo-public-api {{cargo_public_api}}" ] \
        || { echo "error: need cargo-public-api {{cargo_public_api}}, found '${have:-none}'"; exit 1; }
    cargo +{{nightly}} public-api --color never -p oso-platform --all-features -ss \
        --target {{api_macos_target}} > guardrails/public-api/oso-platform-macos.txt
    git diff --quiet -- Cargo.lock \
        || { echo "error: generating the snapshot changed Cargo.lock"; exit 1; }

# Fail if oso-platform's macOS API differs from its committed snapshot
public-api-macos-check: public-api-macos
    #!/usr/bin/env bash
    set -euo pipefail
    if ! git diff --exit-code -- guardrails/public-api/oso-platform-macos.txt; then
        echo "error: oso-platform's macOS API differs from its snapshot (above)."
        echo "Run 'just public-api-macos' and commit it (ADR 0038, ADR 0070)."
        exit 1
    fi
    echo "public API on macOS: matches the snapshot"

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
        # CI puts the last weekly run's lists here (ADR 0050). The list is
        # capped so that the issue stays within GitHub's limit on its size.
        if [ -f mutants.out/missed.txt ]; then
            if [ -f mutants.out/run.md ]; then cat mutants.out/run.md; echo; fi
            count() { if [ -f "mutants.out/$1.txt" ]; then wc -l <"mutants.out/$1.txt" | tr -d ' '; else echo 0; fi; }
            echo "Missed $(count missed), caught $(count caught), timed out $(count timeout), unviable $(count unviable)."
            echo
            # The lists are data from another job, so nothing in them may leave
            # its code span. Rust signatures hold no backticks.
            span() { tr -d '`' | cut -c1-300 | sed 's/.*/- `&`/'; }
            head -n {{mutants_listed}} mutants.out/missed.txt | span
            more=$(($(count missed) - {{mutants_listed}}))
            if [ "$more" -gt 0 ]; then echo; echo "And $more more, in the run's artifact."; fi
            if [ -s mutants.out/exclusions.txt ]; then
                echo
                echo "Problems with the entries in \`.cargo/mutants.toml\`:"
                echo
                head -n 20 mutants.out/exclusions.txt | span
            fi
        else
            echo "Not collected: no weekly mutation run with results (\`mutants.yml\`, ADR 0050)."
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

# Fail on a conflict marker in a tracked file, as the architecture test does, without a
# build: for CI's docs-only path (ADR 0038)
markers:
    #!/usr/bin/env bash
    set -euo pipefail
    if git grep -nE '^(<{7}|\|{7}|>{7})( |$)|^={7}$'; then
        echo "error: conflict markers above; finish resolving the merge, or write a heading with '#'."
        exit 1
    fi
    echo "conflict markers: none"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Docs
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Generate documentation and open it in the browser (excludes dependencies)
doc:
    cargo doc --locked --workspace --open --no-deps

# List the ADRs with their status, read from each file; `just adrs Proposed` keeps the
# lines that contain the word (docs/dev/adr/README.md)
[positional-arguments]
adrs filter="":
    #!/usr/bin/env bash
    set -euo pipefail
    for f in docs/dev/adr/[0-9][0-9][0-9][0-9]-*.md; do
        title=$(head -n 1 "$f")
        status=$(grep -m 1 '^- Status:' "$f" || true)
        printf '%s  (%s)\n' "${title#\# }" "${status#- Status: }"
    done | { grep -F -- "${1:-}" || true; }

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Clean
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Remove all build artifacts
clean:
    cargo clean
