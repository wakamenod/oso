# 0020. License

- Status: Accepted
- Date: 2026-11-03

## Context

The repository is public, and others may use oso as-is. Without a license,
nobody else may legally use, copy or modify it. The license also decides which
dependency licenses are acceptable.

## Decision

- oso is dual-licensed as **MIT OR Apache-2.0**, the Rust ecosystem
  convention (`LICENSE-MIT`, `LICENSE-APACHE`).
- Contributions are accepted under the same terms.
- The `cargo-deny` license allowlist admits only permissive licenses
  compatible with this choice (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode and
  similar).
- The `license` field in `Cargo.toml` is set when the workspace is
  restructured in M1.

## Consequences

- Anyone can use, modify and redistribute oso with few obligations.
- GPL code and data cannot be bundled in the binary. This affects, for
  example, migemo dictionaries derived from SKK dictionaries: oso reads one only from a file
  the person names ([ADR 0116](0116-migemo-and-jumps.md)).

## Alternatives considered

- **GPL-3.0 (as Emacs):** forces derivatives to stay open and would allow
  bundling GPL data, but restricts reuse and is unusual in the Rust ecosystem.
