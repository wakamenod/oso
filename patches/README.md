# Patched third-party crates

oso builds these crates from the copies here, through `[patch.crates-io]` in the workspace's
`Cargo.toml`, instead of from crates.io. Each copy is the released crate unpacked as it was
published, then oso's change on its own commit, so that `git log -- patches/<crate>` shows the
release and the change apart. They are not workspace members: their manifests, lints and tests
stay their own.

## tree-sitter-yaml 0.7.2

- **Source:** `tree-sitter-yaml-0.7.2.crate` from crates.io, sha256
  `53c223db85f05e34794f065454843b0668ebc15d240ada63e2b5939f43ce7c97` (the checksum crates.io's
  index gives it), from upstream commit `7708026449bed86239b1cd5bce6e3c34dbca6415`
  (`.cargo_vcs_info.json`). MIT, whose text in `LICENSE` is unchanged.
- **oso's change** ([ADR 0157](../docs/dev/adr/0157-highlighting-html-css-json-and-yaml.md)):
  one line of `src/scanner.c`, a bound on what the external scanner serializes, so that it stays
  within tree-sitter's serialization buffer.
- **At a bump:** read the new release's `build.rs` and scanner diff, as for every grammar
  (ADR 0111). If it carries the change, delete this copy and the `[patch.crates-io]` entry and
  take the crate from crates.io again. If it does not, unpack the new `.crate` here on its own
  commit, checking its sha256 against `Cargo.lock`, and apply the change again on the next.
- `cargo audit` reads `Cargo.lock`, where a patched crate has no registry source, so it does
  not look this crate up: check RustSec for `tree-sitter-yaml` by hand at a bump.
