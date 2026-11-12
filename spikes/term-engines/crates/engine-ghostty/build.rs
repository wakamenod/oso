//! Builds libghostty-vt from the vendored source with Zig and links its static archive.
//!
//! Inputs (all optional):
//! - `GHOSTTY_VT_SRC`: the libghostty-vt source tree. Default: the vendored 1.0.1 release
//!   under `spikes/term-engines/vendor/src`.
//! - `GHOSTTY_VT_SYSTEM`: an unpacked dependency directory passed as `zig build --system`.
//!   Default: the release's sibling `-deps` directory when building the default source.
//! - `ZIG`: the Zig binary. Default: `zig` on PATH. This script never downloads anything.
//! - `ZIG_GLOBAL_CACHE_DIR`: passed through when set; otherwise `<OUT_DIR>/zig-global-cache`,
//!   created empty (Zig 0.17 crashes with "reached unreachable code" on a used one).
//! - `GHOSTTY_VT_OPTIMIZE`: Zig optimize mode. Default: `ReleaseFast`.

// oso's clippy.toml (found by walking up from this spike) bans println and Command for
// editor code. A build script talks to cargo through stdout and has to run zig, so both
// are allowed here; this crate never ships in oso.
#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const VERSION: &str = "1.0.1";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_SRC");
    println!("cargo:rerun-if-env-changed=ZIG");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_OPTIMIZE");
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_SYSTEM");
    println!("cargo:rerun-if-env-changed=ZIG_GLOBAL_CACHE_DIR");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let default_src = manifest.join(format!("../../vendor/src/libghostty-vt-{VERSION}"));
    let src = match env::var_os("GHOSTTY_VT_SRC") {
        Some(dir) => PathBuf::from(dir),
        None => default_src.clone(),
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
    let global_cache = match env::var_os("ZIG_GLOBAL_CACHE_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            let dir = out.join("zig-global-cache");
            // Start empty on every run of this script.
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create zig global cache dir");
            dir
        }
    };
    // A release's dependency tarball, unpacked: Zig takes its packages from there, offline.
    let system = match env::var_os("GHOSTTY_VT_SYSTEM") {
        Some(dir) => Some(PathBuf::from(dir)),
        None if src == default_src => {
            let deps = manifest.join(format!("../../vendor/src/libghostty-vt-{VERSION}-deps"));
            deps.is_dir().then_some(deps)
        }
        None => None,
    };

    let mut command = Command::new(&zig);
    command
        .current_dir(&src)
        .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
        .arg("build");
    if let Some(deps) = &system {
        command.arg("--system").arg(deps);
    }
    let status = command
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg("-Demit-xcframework=false")
        .arg("-Demit-themes=false")
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
