---
name: add-dependency
description: Research, propose and add a new or upgraded crate dependency. Use before any cargo add, version bump or Cargo.toml dependency change.
---

# Add or upgrade a dependency

New and upgraded dependencies are a human gate (ADR 0010, ADR 0018). Research
first, report, and change nothing until the author approves.

## 1. Research

For the crate and version you propose:

- **Need:** what it is for, why the standard library or an existing
  dependency is not enough, and which alternatives you compared.
- **Policy:** no networking crates and no regex engine other than `regex`
  (ADR 0010). Check the `[bans]` list in `deny.toml`.
- **Maintenance:** maintainers or owning organization, recent releases and
  commit activity, open security issues, and whether RustSec lists it as
  unmaintained.
- **`unsafe`:** roughly how much the crate uses and what for (FFI, SIMD,
  allocation tricks). Search the crate source.
- **`build.rs` and proc macros:** whether it runs code at build time (T2 in
  `docs/dev/security.md`).
- **Transitive dependencies:** which new crates it adds to `Cargo.lock`, and
  any duplicate versions. To measure without touching the repo, copy
  `Cargo.toml`, `Cargo.lock`, `deny.toml`, `clippy.toml`,
  `rust-toolchain.toml` and `crates/` to a scratch directory, add it
  there and compare `cargo tree` output. Do not build the scratch copy:
  building runs the candidate's `build.rs` and proc macros.
- **License:** must be allowed by `[licenses]` in `deny.toml` (ADR 0020).
  Do not widen the allow list without approval.
- **`cargo deny check`** and **`cargo audit`** results, run in the scratch
  copy.
- **Features:** enable only what is needed; prefer `default-features = false`
  when defaults pull in more.

## 2. Report and wait

Report the findings above in a short summary with a recommendation, the exact
`Cargo.toml` diff and the list of new transitive crates. **Wait for explicit
approval.** Do not edit `Cargo.toml` or `Cargo.lock` before it.

## 3. After approval

1. Add the dependency with exactly the approved version and features.
2. Run `just deny`, `just audit` and `just ci`. All must pass.
3. Commit the dependency change on its own, and mention the approval and the
   research summary in the commit message or PR description.
