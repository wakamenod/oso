# 0042. Statics, API snapshot upkeep and render errors

- Status: Accepted
- Date: 2026-11-04

## Context

[ADR 0039](0039-architecture-guardrail-mechanics.md) and M1-10a left four
guardrail items open until M1-10b:

- Statics with interior mutability, such as `static X: Mutex<…>` and atomics,
  are global state, but no check catches them.
- Nothing says when the pinned nightly and `cargo-public-api` move.
- `oso-render` exposes wgpu types in `RenderError::RecreateSurface` and
  `CaptureError::UnsupportedFormat`. The tip crate cannot match on them
  without wgpu, which only `oso-render` may use.
- The public API snapshots are Linux-only, and the dev control socket stub
  differs on Windows.

Product code has no `static` item today. Two test helpers each hold a counter
that makes scratch directory names unique.

## Decision

### Statics

- The architecture test rejects every `static` item, whatever its type, not
  only `static mut`.
- Inside a top-level test item (`#[cfg(test)]`, `#[cfg(all(test, …))]`), a
  plain `static` is allowed, because a test counter is not editor state.
  `static mut` stays banned there too.
- Data that never changes is a `const`. State is host-owned
  ([ADR 0039](0039-architecture-guardrail-mechanics.md), Lints).
- There is no `#[expect]` exception. A real need, such as a global allocator,
  gets an ADR that changes the check.

### Snapshot toolchain

The pinned nightly and `cargo-public-api` in the `Justfile` move in the same
pull request as `rust-toolchain.toml`. The nightly's compiler version must not
be older than the stable toolchain, or code that uses a newer standard library
API cannot be snapshotted.

### Render errors

Both variants carry a `String`, like `CaptureError::Gpu`. No wgpu type remains
in `oso-render`'s errors. The window handle stays the one intended exception.

### Snapshot platforms

The snapshots stay Linux-only. Implementing the Windows control socket
transport is the point to look again, or any earlier change that makes the
public API differ by OS outside dev-only code.

## Consequences

- A new global cannot arrive by accident, even behind a type alias or a wrapper
  type, since the check does not read types.
- A large read-only table cannot be a `static`. As a `const`, it may be copied
  where it is used by value, so such a table needs a reference to a `const` or
  an ADR.
- Upgrading Rust also installs a new nightly and may rebuild `cargo-public-api`.
- Code cannot match on wgpu error details from these two variants. Nothing does.
- A Windows-only public API difference outside dev code would not be seen until
  the snapshots are reviewed again.

## Alternatives considered

- **Banning statics by type name (`Mutex`, `RwLock`, `Atomic*`):** would allow
  read-only tables, but an alias or a wrapper type passes unseen.
- **Review only for statics:** gradual drift is what ADR 0038 enforces against.
- **No exemption for test items:** every caller of the two helpers would have to
  pass a unique name by hand, for no gain in product code.
- **Upgrading the snapshot toolchain only when the job breaks:** least work, but
  a Rust upgrade can fail with no warning.
- **Keeping the wgpu types in the errors:** would make the exception in
  `oso-render`'s rules permanent.
- **Per-OS snapshots now:** already rejected in ADR 0039. The only difference is
  still dev-only code.
