# 0024. Dev build profile and build-time budget

- Status: Accepted
- Date: 2026-11-03

## Context

[ADR 0002](0002-dev-hot-reload.md) sets a 2 s budget for an incremental
build of the tip crate, and lists a dev profile, a fast linker and optionally
cranelift as the tools. Unoptimized dependencies such as wgpu also make dev
builds slow at runtime. The toolchain is pinned to a stable release
([ADR 0021](0021-development-tooling.md)).

## Decision

- **Dev profile:** optimize dependencies, not oso.
  ```toml
  [profile.dev.package."*"]
  opt-level = 2
  debug = false
  ```
  oso's own crates keep `opt-level = 0` and full debug info. This is added to
  `Cargo.toml` in the M1-2 workspace split, with approval.
- **Linker:** the platform default. On macOS this is Apple's linker
  (ld-prime), and on x86_64 Linux it is the `lld` that Rust ships. Another
  linker (for example mold) is considered only if the budget is missed.
- **cranelift:** not used. It needs a nightly toolchain, which conflicts with
  the pinned stable toolchain.
- **Measuring the budget:** `just build-time` changes one file in the tip
  crate and times the incremental build several times, reporting the median.
  - The author's Mac is the reference: the budget passes or fails there.
  - CI records the time on each platform and fails only on a large
    regression against the last result on `main`. The exact threshold is set
    in M1-8.

## Consequences

- Dev builds run close to release speed in GPU and text code, while edits to
  oso still rebuild quickly. The first build of dependencies takes longer.
- No extra tools to install.
- CI runner speed varies, so CI catches regressions rather than enforcing the
  2 s limit itself.

## Alternatives considered

- **`opt-level = 1` everywhere:** faster oso code in dev, but slower
  incremental builds against the 2 s budget.
- **Default profile:** unoptimized wgpu makes dev builds sluggish.
- **mold on Linux from the start:** helps mostly CI, since development
  happens on macOS.
- **Failing CI at exactly 2 s:** simple, but flaky on shared runners.
