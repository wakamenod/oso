# 0139. Line tables for oso's own crates, and pruning stale build outputs

- Status: Proposed
- Date: 2026-11-16

## Context

[ADR 0024](0024-dev-build-profile.md) gives oso's own crates `opt-level = 0` and full debug
info in dev builds, and its dependencies `opt-level = 2` and none. Tests use the `test`
profile, which inherits `dev`. Each worktree's `target/` grows to 15–20 GB, and `main`'s
reached 44 GB, while several sessions build side by side on one disk.

Measured on 2026-11-16 on the author's Mac (rustc 1.96.0). Each `just ci` started from an
empty `target/`. Other sessions were building on the machine at the same time (load 4–17), so
wall times vary by about ±30 s between repeats of the same setting. Each setting was passed as
a `CARGO_PROFILE_DEV_*` variable.

| Setting for oso's crates | `target/` after `just ci` | `deps/` | `incremental/` | `just ci` | Edit in oso-core, `cargo test --no-run` | Edit in `main.rs`, `cargo build -p oso` |
|---|---|---|---|---|---|---|
| Full debug info (ADR 0024) | 5.9 GB | 2.2 GB | 3.1 GB | 269 s, 246 s | 4.2 s | 1.3–1.7 s |
| `line-tables-only` | 5.0 GB | 1.4 GB | 3.0 GB | 261 s, 213 s | 4.0 s | 1.2–1.6 s |
| No debug info | 4.6 GB | 0.9 GB | 3.2 GB | 289 s | 3.9 s | 1.4 s |
| `line-tables-only`, incremental off | 2.0 GB | 1.4 GB | 0 | 224 s | 13.1 s | 6–7 s |
| `line-tables-only`, `split-debuginfo = "packed"` | 5.5 GB | 1.4 GB | 3.0 GB | 206 s | 4.8 s | 2.0 s |
| Full, `split-debuginfo = "packed"` | 7.1 GB | 2.2 GB | 3.0 GB | 225 s | 7.9 s | 5.3 s |

- A second run started from both test builds alone (`cargo test --no-run`, with and without
  `--all-features`), then made 15 edits spread over oso-core, oso-render and oso, each followed
  by both test builds. `target/` went from 4.3 to 5.7 GB with full debug info and from 3.5 to
  4.8 GB with line tables. Incremental caches add about 1.4 GB once and then stay level.
- A worktree that had grown to 20 GB held 11 GB in `deps/`: 38,000 objects, 4.4 GB of them
  named by no binary. It held 7.2 GB in 100 incremental caches, where a fresh `just ci` makes 29,
  and 1.4 GB in the Windows and Linux directories that `clippy-windows` and `public-api` write.
  Merging `main` changes `Cargo.lock`, which gives every unit of oso a new hash. Cargo never
  removes the units, caches and objects of the old hashes.
- On macOS, `split-debuginfo` is already `unpacked` in dev builds. The debug info stays in the
  objects in `deps/`, and each binary lists the objects it was linked from. Each relink adds new
  objects and leaves the old ones.
- With line tables, a failing `assert_eq!` and an `unwrap` in a test still print the panic's
  file:line and the file:line of every frame under `RUST_BACKTRACE=1`. Frames of oso's crates
  lose their module path (`inner` instead of `oso_core::…::inner`). With no debug info, oso's
  frames lose file:line.
- With line tables, lldb still sets breakpoints by function and by file:line, and `bt`, `next`
  and the source listing work. `frame variable`, arguments in frames, `p` and types are gone
  ("no variable information is available"). `packed` behaves the same.

## Decision

1. **Line tables only.** `Cargo.toml` sets `debug = "line-tables-only"` under `[profile.dev]`.
   The `test` profile inherits it. Dependencies keep ADR 0024's `opt-level = 2` and
   `debug = false`. The release profile is unchanged.
2. **`split-debuginfo` and incremental stay as they are.** On macOS, `split-debuginfo` stays at
   the dev default, `unpacked`. Incremental compilation stays on: turning it off makes
   edit-and-rebuild three to five times slower, against [ADR 0002](0002-dev-hot-reload.md)'s
   2 s budget (measured as in ADR 0024).
3. **Variables in a debugger are opt-in.** A session that needs them builds with
   `CARGO_PROFILE_DEV_DEBUG=true`, for example
   `CARGO_PROFILE_DEV_DEBUG=true cargo test -p oso-core --no-run`. Units built that way get
   their own hashes, so a later plain build does not rebuild over them, and `just prune-target`
   removes them once they are unused.
4. **`just prune-target`** removes stale build outputs from the worktree's own `target/`.
   `--dry-run` lists what would go and removes nothing. `--days N` sets the age, 3 days by
   default.
   - It runs only when cargo's target and build directories are this worktree's `target/`,
     and neither `target/`, `target/debug` nor any directory it removes from is a symbolic
     link. It refuses before it removes anything if any path it chose lies outside, or if the
     directory holding the path resolves elsewhere.
   - It chooses and removes while holding cargo's lock on `target/debug`, so no build in the
     worktree runs meanwhile. A build already running is waited for.
   - It first runs the builds of `just ci` (`check`, `clippy` and `test --no-run`, with and
     without `--all-features`), of `just e2e`, and of `just clippy-windows` once that has run in
     the worktree. These build nothing when they ran last. Cargo's messages name the units in
     use. A copy without a hash, such as `target/debug/oso`, is matched to its unit by content
     right after the build that wrote it. If a build fails, nothing is removed.
   - **Objects** (macOS): an object in `deps/` or `examples/` that its binary's debug map does
     not name, or that has no binary, as a library's objects do: debug maps name the copies in
     its rlib. Objects of the last hour are kept. If `nm` cannot read a binary, all of its
     objects are kept.
   - **Units:** a unit in `target/debug` that none of these builds uses and that nothing touched
     for `--days` days. Its fingerprint, build-script directory and files in `deps/` and
     `examples/` all go.
   - **Incremental caches:** a cache that no unit in use compiles into, and that nothing
     touched for `--days` days. A cache is in use when its newest session started within 5 s of
     the last compile of a unit in use of the same crate (the unit's `invoked.timestamp`).
   - **Other targets:** the directories of `clippy-windows`, `public-api` and
     `public-api-macos` (`target/<triple>`), when nothing in them was touched for `--days`
     days.
   - "Touched" means written. Cargo does not rewrite a unit it finds fresh, so a unit that only
     a build outside the list uses goes once it is `--days` old, even if that build ran today.
     `just build`, `run-dev`, `dev` and `release-safety` pay that rebuild where their units
     differ from those of the listed builds.
   - Units of an old `Cargo.lock` that were built within `--days` days stay. `--days 0` removes
     everything unused at once.
   - It needs `jq`, `perl` for the lock, and `nm` on macOS.
   - A unit or cache it removes in error only costs a rebuild. Cargo checks every unit's
     fingerprint and outputs, and rustc rebuilds a missing cache from scratch.

## Changes to accepted ADRs

[ADR 0024](0024-dev-build-profile.md), Decision, "Dev profile": "oso's own crates keep
`opt-level = 0` and full debug info" becomes: oso's own crates keep `opt-level = 0` and line
tables only (`debug = "line-tables-only"`).

## Consequences

- A fresh `target/` is about 0.9 GB smaller (5.9 → 5.0 GB), and it stays about that much
  smaller as it grows. Build times and `just ci` do not change measurably. The test run time
  that ADR 0037 budgets was not measured on its own.
- Backtraces keep file:line. Frame names of oso's crates are shorter.
- Inspecting variables in lldb needs the opt-in build in decision 3.
- How much a worktree grows depends on how often stale outputs are pruned. `just prune-target`
  is run by hand, for example after merging `main`. Nothing runs it on its own. A worktree keeps
  about `--days` days of old outputs.
- It handles `target/debug` and whole cross-target directories. `target/release`, the perf
  builds (`target/perf-build`, `target/perf-remote`), coverage and fuzz outputs are left
  alone. `just clean` still empties everything.
- Its list of builds repeats the commands of `just check`, `lint`, `test`, `e2e` and
  `clippy-windows`, so a change to those recipes needs the same change in `prune-target`. A
  comment above `ci` says so. A build missing from the list fails safe: its units go after
  `--days` days, which costs a rebuild.

## Alternatives considered

- **`debug = false` for oso too:** 0.4 GB less than line tables, but backtraces lose file:line
  on oso's frames, which is what a failing test needs most.
- **`split-debuginfo = "packed"`:** no objects left in `deps/`, but each binary gets a `.dSYM`,
  which makes `target/` larger (5.5 GB instead of 5.0) and every relink slower (`dsymutil`).
- **Incremental compilation off:** `target/` drops to 2.0 GB, but rebuilding after an edit
  goes from about 4 s to 13 s, and `cargo build -p oso` from about 1.3 s to 6 s, against ADR
  0002's 2 s budget.
- **One target directory shared by all worktrees** (`CARGO_TARGET_DIR` or `build.target-dir`):
  less disk, but every session would wait on the others for cargo's build lock, and each
  worktree's sources would rebuild what another worktree just built over it. `prune-target`
  refuses a target directory outside the worktree for the same reason: it cannot tell what
  the other worktrees use.
- **sccache:** a new tool to install, and its cache is one more directory that grows.
- **Cargo's own garbage collection:** it cleans the global cache under `~/.cargo`, not target
  directories.
- **`cargo-sweep`:** a new dependency on a tool, and it judges staleness by file times alone.
  A dependency that is still in use but was built long ago would look stale too.
- **Grouping units by their fingerprint and keeping the newest:** `check` and `clippy` units
  of the same crate have the same fingerprint fields and differ only in their hashes, so one of
  each pair would be removed and rebuilt on every `just ci`.
- **Keeping the newest caches of each crate, as many as it has units in use:** a cache of a
  build outside `just ci` that is newer than one in use would push the one in use out.
