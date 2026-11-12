//! Builds libghostty-vt from the vendored source with Zig and links its static archive.
//!
//! Inputs (all optional):
//! - `GHOSTTY_VT_SRC`: the libghostty-vt source tree. Default: the vendored copy under
//!   `spikes/term-engines/vendor/src`.
//! - `ZIG`: the Zig binary. Default: `zig` on PATH. Zig's package cache must already hold
//!   the library's dependencies; this script never downloads anything.
//! - `GHOSTTY_VT_OPTIMIZE`: Zig optimize mode. Default: `ReleaseFast`.

// oso's clippy.toml (found by walking up from this spike) bans println and Command for
// editor code. A build script talks to cargo through stdout and has to run zig, so both
// are allowed here; this crate never ships in oso.
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "1.4.0-main+83fb956";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_SRC");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_OPTIMIZE");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let src = match env::var_os("GHOSTTY_VT_SRC") {
        Some(dir) => PathBuf::from(dir),
        None => manifest.join(format!("../../vendor/src/libghostty-vt-{VERSION}")),
    };
    if !src.join("build.zig").is_file() || !src.join("include/ghostty/vt.h").is_file() {
        panic!(
            "libghostty-vt source not found at {} (expected build.zig and include/ghostty/vt.h). \
             Extract the vendored tarball there or set GHOSTTY_VT_SRC.",
            src.display()
        );
    }

    let optimize = env::var("GHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    if !["Debug", "ReleaseSafe", "ReleaseFast", "ReleaseSmall"].contains(&optimize.as_str()) {
        panic!(
            "GHOSTTY_VT_OPTIMIZE must be Debug, ReleaseSafe, ReleaseFast or ReleaseSmall, got {optimize:?}"
        );
    }

    let zig = env::var_os("ZIG").unwrap_or_else(|| "zig".into());
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let prefix = out.join("zig-out");
    let cache = out.join("zig-cache");

    let status = Command::new(&zig)
        .current_dir(&src)
        .arg("build")
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg("-Demit-xcframework=false")
        .arg("-Demit-themes=false")
        .arg(format!("-Dversion-string={VERSION}"))
        .arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(&cache)
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "could not run {:?} (set ZIG to the Zig 0.17 binary): {e}",
                Path::new(&zig)
            )
        });
    if !status.success() {
        panic!("zig build of libghostty-vt failed: {status}");
    }

    // Copy the static archive alone into its own directory so the linker can never pick
    // the dylib that zig installs next to it.
    let archive = prefix.join("lib/libghostty-vt.a");
    if !archive.is_file() {
        panic!("zig build did not produce {}", archive.display());
    }
    let link_dir = out.join("link");
    std::fs::create_dir_all(&link_dir).expect("create link dir");
    std::fs::copy(&archive, link_dir.join("libghostty-vt.a")).expect("copy libghostty-vt.a");

    println!("cargo:rustc-link-search=native={}", link_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
    println!("cargo:rustc-env=GHOSTTY_VT_SOURCE_VERSION={VERSION}");
}
