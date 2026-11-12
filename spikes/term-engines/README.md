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
2. Ghostty's source for commit `83fb95624b04`, checked and extracted into `vendor/` (not
   committed):

   ```sh
   mkdir -p vendor/src && cd vendor
   curl -fLO https://tip.files.ghostty.org/83fb95624b0439ee660c2679b43b38c8b63769cd/libghostty-vt-source.tar.gz
   echo "ed57ae9b95581c88f2614e976c1d3f146c02d443a3c26139200eb9de6e645eb4  libghostty-vt-source.tar.gz" | shasum -a 256 -c
   tar xzf libghostty-vt-source.tar.gz -C src
   cd src/libghostty-vt-1.4.0-main+83fb956 && zig build --fetch
   ```

   `zig build --fetch` downloads Zig's hash-pinned packages once; builds then run offline.
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
