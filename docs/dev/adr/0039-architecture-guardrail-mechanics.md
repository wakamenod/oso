# 0039. Architecture guardrail mechanics

- Status: Accepted
- Date: 2026-11-03

## Context

[ADR 0038](0038-architecture-guardrails.md) decided which structural rules
are enforced. Implementing them (M1-10a) needed decisions it leaves open:

- the format of the rules
- what counts as a line
- how a crate root is treated
- how to confine OS bindings

It also turned out that `cargo-deny` `wrappers` checks only **direct**
dependents and matches **exact** names. `libc`, the `objc2` family and the
`windows` crates each have about 20 third-party direct dependents, such as
`wgpu-hal`, `winit` and `rustix`.

## Decision

### Where the rules live

- `guardrails/` holds the data the checks read:
  - `layers.txt`: the layer rules
  - `size-allowlist.txt`: the size ratchet
  - `public-api/<crate>.txt`: the API snapshots
- The checks are one integration test, `crates/oso/tests/architecture`. It
  reads source text and manifests with a small lexer. It has no
  dependencies and spawns no processes.
- Editing `guardrails/` or the test code needs the author's approval, like
  `clippy.toml`.

### Layer rules

- Each section of `layers.txt` names a crate, or `[workspace]` for the
  crate graph.
- Each line is one layer, highest first. An entry may use only entries on
  lower lines. Entries on the same line may not use each other.
- **Every crate and every top-level module must be listed.** Adding a
  module therefore changes the rules, and the pull request must cite an
  ADR. An existing ADR that the module implements is enough.
- A use is a `crate::` path, a `super::` path that leaves the module, or,
  in the crate root, a bare path to a child module. Comments and doc links
  do not count. Test modules count.
- The crate root is the node `crate`:
  - Its `mod` declarations and `pub use` re-exports are wiring, not uses.
  - Its other code uses modules like any module.
  - If `crate` is not listed, it sits above every layer.
- A use of something that is neither a listed module nor the root fails
  the check.
- The initial `[oso-core]` layers follow `architecture.md`: editor state
  and cursors sit above the display model, which sits above the rope.

### External crates

- `cargo-deny` `wrappers` confine `wgpu` to `oso-render` and `winit` to
  `oso`. Each has exactly one direct dependent.
- OS bindings are confined to `oso-platform` by the architecture test
  instead. It fails when any other workspace crate **declares** a crate
  matching a binding name or family, for example `objc2*`,
  `core-foundation*`, `libc` or `windows*`. Transitive use through `wgpu`
  or `winit` is allowed.
- The test resolves every rename form (`package =` inline, dotted, in a
  `[dependencies.<key>]` table, or inherited from
  `[workspace.dependencies]`). A rename it cannot read fails the test.

### Size limits

- A file's size is its number of lines outside top-level test items
  (`#[cfg(test)]`, `#[cfg(all(test, …))]`), comments and blank lines
  included.
- The limit applies to all Rust code under `crates/` except `tests/`
  directories.
- Each allowlist entry records the file's current size:
  - The file may not grow past it.
  - The entry must be removed once the file is at or under 600 lines.
- A function over the clippy limits carries
  `#[expect(clippy::too_many_lines, reason = "…")]`. Once the function
  shrinks, the expectation is unfulfilled and fails the build, so the
  attribute must go. This is the function-level ratchet.

### Lints

- `print!` and `eprint!` are banned with `println!` and `eprintln!`.
- `static mut` items are rejected by the architecture test, because no
  lint flags the declaration. rustc's `static_mut_refs` is denied as well.
  There is no `#[expect]` exception. A real need gets an ADR.
- A cache that a lazy static would have held (a compiled regex, a lookup
  table, a scratch buffer) belongs in host-owned state, such as a field of
  `Editor` or the renderer. Rebuilding it on every call or frame is not the
  alternative.
- In `oso-core`, `#![deny(clippy::unwrap_used, clippy::expect_used)]`.
  Tests are exempt through `clippy.toml`.

### Public API snapshots

- `cargo-public-api` runs with all features and without blanket and
  auto-trait impls (`-ss`), on a nightly pinned in the `Justfile`.
- The snapshots are taken for `x86_64-unknown-linux-gnu`, so every OS
  produces the same files. Windows differs only in the dev control socket
  stub.

### ADR citation check

The structure workflow requires `ADR NNNN` in the pull request description
when a change:

- changes `layers.txt`
- adds a `pub mod` or `pub trait` line to a snapshot (other API changes
  are reviewed in the snapshot diff, without an ADR)
- adds or removes a crate, changes the workspace members, or changes a
  dependency line naming a workspace crate
- adds or raises an allowlist entry
- changes the architecture checks or the size thresholds in `clippy.toml`

Removing or lowering an entry is free. Allowlist entries must start at the
beginning of the line, so the check sees each one.

### Staging

- M1-10a implements everything except the `//!` responsibility headers
  (ADR 0038, point 7). They touch every source file, so they follow in
  M1-10b, after M1-6 merges.
- The health report compares file sizes and dependency edges with the
  checkout from 7 days ago. Surviving mutants are added by M1-9a.

## Consequences

- Structure changes show up as diffs to `guardrails/`, and those need an
  approval and an ADR citation.
- New modules carry a small cost: one line in `layers.txt` and an ADR
  citation. This is intended.
- The lexer is approximate: no macro expansion and no `#[path]`, and bare
  paths in a module that glob-imports the crate root (`use super::*;` in a
  top-level file) are not seen. Review covers these.
- Statics with interior mutability, such as `static X: Mutex<…>`, are not
  yet caught mechanically. They are listed in the open questions.
- Contributors who update snapshots need the pinned nightly and
  `cargo-public-api`. `just ci` does not.
- CI builds `cargo-public-api` from source with `cargo install --locked`,
  because `taiki-e/install-action` has no binary for it. This is an
  exception to installing tools as checksum-verified binaries
  ([ADR 0021](0021-development-tooling.md)): the sources are still verified
  against the crates.io checksums, and the job has a read-only token and no
  secrets, but the tool's build scripts run in CI.

## Alternatives considered

- **`cargo-deny` wrappers for OS bindings:** each entry would have to list
  every third-party direct dependent. The lists change with every upgrade,
  and the name families cannot be matched with globs.
- **`syn` or `cargo-modules` for the module graph:** more exact, but they
  add a dependency or a tool. The lexer is enough for `crate::` and
  `super::` paths.
- **Counting only code lines (no comments):** this would not penalize
  documentation, but it is harder to check by eye. `rope.rs` would also
  sit just under the limit without being any smaller.
- **Leaving unlisted modules unconstrained:** every new module would open a
  gap in the rules.
- **Per-OS snapshots:** three files per crate for a difference that today
  exists only in dev-only code.
