# Terminal engine comparison (ADR 0120, step 1)

A throwaway, standalone Cargo workspace that compares `alacritty_terminal` and libghostty-vt
behind one `Engine` trait. It is never merged into oso; the result is in ADR 0120's step 1
"As built". The author approved its dependencies and tools on 2026-11-13.

- `crates/engine`: the `Engine` trait, `Snapshot`, `Effect`.
- `crates/engine-alacritty`: `alacritty_terminal` 0.26.0 plus a second `vte` parser for the
  OSCs `Term` ignores.
- `crates/engine-ghostty`: libghostty-vt through hand-written FFI; `build.rs` runs Zig.
- `crates/harness`: `compare`, `bench`, `mem`, `string-flood`, `fuzz`, `replay`, and the
  `size_*` examples.
- `record.sh`: records vim, less, htop, lazygit, vttest and Claude Code into `recordings/`
  (not committed).

## Setup

1. Zig 0.17.1 (`brew install zig`).
2. The libghostty-vt 1.0.1 release and its dependency tarball, verified and extracted into
   `vendor/` (not committed). The comparison began on the pre-release commit `83fb95624b04`,
   whose ABI differs; the FFI now follows 1.0.1.

   ```sh
   mkdir -p vendor/src && cd vendor
   base=https://release.files.ghostty.org/libghostty-vt/1.0.1
   for f in SHA256SUMS SHA256SUMS.minisig libghostty-vt-1.0.1.tar.gz libghostty-vt-1.0.1-deps.tar.gz; do
       curl -fLO "$base/$f"
   done
   minisign -V -P RWQlAjJC23149WL2sEpT/l0QKy7hMIFhYdQOFy0Z7z7PbneUgvlsnYcV -m SHA256SUMS
   grep 'libghostty-vt-1.0.1' SHA256SUMS | grep -v docs | shasum -a 256 -c
   tar xzf libghostty-vt-1.0.1.tar.gz -C src
   tar xzf libghostty-vt-1.0.1-deps.tar.gz -C src
   ```

   `build.rs` passes the deps directory to `zig build --system` and a fresh Zig cache, so
   builds run offline.
3. `brew install htop lazygit vttest`, then `./record.sh`.

## Runs

```sh
cargo run --release -p harness -- compare recordings/*.bin
cargo run --release -p harness -- bench
/usr/bin/time -l target/release/harness mem alacritty 10000 200
target/release/harness fuzz ghostty 3600 1 fuzz-state.txt
cargo build --profile size -p harness --examples   # then compare target/size/examples/size_*
```

`GHOSTTY_VT_OPTIMIZE=ReleaseSafe` builds libghostty-vt with Zig's safety checks, for fuzzing.
