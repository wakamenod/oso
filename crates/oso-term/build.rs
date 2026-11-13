//! Builds libghostty-vt with Zig from the release pinned in `ghostty-vt.lock`, and links its
//! static archive (ADR 0120). It never downloads: `just ghostty-vt` fetches the tarballs and
//! checks their signatures; this checks their sha256 again, unpacks them into `OUT_DIR`, and
//! fails unless the dependency tarball holds exactly the packages the lock lists. It then
//! applies oso's patches in `patches/`, each checked against its sha256 in the lock, and fails
//! unless every patched file has the sha256 the lock gives it.
//!
//! Must not depend on: the network, or anything but the lock, the tarballs, the patches, Zig,
//! `tar`, `patch` and a sha256 tool.
//!
//! Inputs, all optional:
//! - `OSO_GHOSTTY_VT_DIR`: the directory holding the tarballs. Default:
//!   `$OSO_VENDOR_DIR/libghostty-vt-<version>`, `OSO_VENDOR_DIR` defaulting to
//!   `$HOME/.cache/oso/vendor`.
//! - `ZIG`: the Zig binary, which must be the lock's version. Default: `zig` on `PATH`.
//! - `OSO_GHOSTTY_VT_OPTIMIZE`: Zig's optimize mode, `ReleaseFast` by default; the fuzz runs
//!   `ReleaseSafe` too.
//! - `OSO_TERM_ENGINE=skip`: build nothing and link nothing, for rustdoc (`just public-api`).
//!   Refused in a release build.
//!
//! Only Unix targets build the library: oso has terminals on macOS and Linux (ADR 0120, Q3).

// A build script runs Zig, `tar`, `patch` and a sha256 tool, and talks to cargo through its standard
// output; it never ships in oso (crates/oso/tests/architecture allows this file).
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "a build script runs Zig and prints cargo directives (ADR 0120)"
)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const OPTIMIZE_MODES: [&str; 4] = ["Debug", "ReleaseSafe", "ReleaseFast", "ReleaseSmall"];

/// What `ghostty-vt.lock` pins.
struct Lock {
    version: String,
    zig: String,
    source: (String, String),
    deps: (String, String),
    packages: Vec<String>,
    /// oso's patches, in order: (file in `patches/`, sha256).
    patches: Vec<(String, String)>,
    /// What the patched source must hold: (path in the source, sha256).
    patched: Vec<(String, String)>,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=ghostty-vt.lock");
    for name in [
        "OSO_TERM_ENGINE",
        "OSO_GHOSTTY_VT_DIR",
        "OSO_VENDOR_DIR",
        "ZIG",
        "OSO_GHOSTTY_VT_OPTIMIZE",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let lock = read_lock(&manifest.join("ghostty-vt.lock"));
    println!("cargo:rustc-env=OSO_GHOSTTY_VT_VERSION={}", lock.version);
    if env::var_os("CARGO_CFG_UNIX").is_none() {
        return;
    }
    if env::var("OSO_TERM_ENGINE").is_ok_and(|v| v == "skip") {
        if env::var("PROFILE").is_ok_and(|p| p == "release") {
            panic!("OSO_TERM_ENGINE=skip is refused in a release build");
        }
        return;
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let tarballs = tarball_dir(&lock.version);
    let src = unpack(&tarballs, &lock.source, &out.join("src"));
    apply_patches(&manifest.join("patches"), &src, &out.join("patches"), &lock);
    let deps = unpack(&tarballs, &lock.deps, &out.join("deps"));
    check_packages(&deps, &lock.packages);
    let zig = env::var_os("ZIG").unwrap_or_else(|| "zig".into());
    check_zig(&zig, &lock.zig);
    let archive = zig_build(&zig, &src, &deps, &out);
    let link_dir = out.join("link");
    fs::create_dir_all(&link_dir).expect("create the link directory");
    // The archive alone in its own directory, so the linker never picks the dylib beside it.
    fs::copy(&archive, link_dir.join("libghostty-vt.a")).expect("copy libghostty-vt.a");
    println!("cargo:rustc-link-search=native={}", link_dir.display());
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}

fn read_lock(path: &Path) -> Lock {
    let text = fs::read_to_string(path).expect("read ghostty-vt.lock");
    let mut version = None;
    let mut zig = None;
    let mut source = None;
    let mut deps = None;
    let mut packages = Vec::new();
    let mut patches = Vec::new();
    let mut patched = Vec::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
    {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let pair = |f: &[&str]| (f[1].to_owned(), f[2].to_owned());
        match fields.as_slice() {
            ["version", v] => version = Some((*v).to_owned()),
            ["zig", v] => zig = Some((*v).to_owned()),
            ["source", _, _] => source = Some(pair(&fields)),
            ["deps", _, _] => deps = Some(pair(&fields)),
            ["package", name] => packages.push((*name).to_owned()),
            ["patch", _, _] => patches.push(pair(&fields)),
            ["patched", _, _] => patched.push(pair(&fields)),
            ["url" | "minisign", _] => {}
            _ => panic!("ghostty-vt.lock: unknown line {line:?}"),
        }
    }
    Lock {
        version: version.expect("ghostty-vt.lock: no version"),
        zig: zig.expect("ghostty-vt.lock: no zig"),
        source: source.expect("ghostty-vt.lock: no source"),
        deps: deps.expect("ghostty-vt.lock: no deps"),
        packages,
        patches,
        patched,
    }
}

fn tarball_dir(version: &str) -> PathBuf {
    if let Some(dir) = env::var_os("OSO_GHOSTTY_VT_DIR") {
        return PathBuf::from(dir);
    }
    let vendor = env::var_os("OSO_VENDOR_DIR").map_or_else(
        || {
            let home = env::var_os("HOME").expect("HOME, or set OSO_VENDOR_DIR");
            PathBuf::from(home).join(".cache/oso/vendor")
        },
        PathBuf::from,
    );
    vendor.join(format!("libghostty-vt-{version}"))
}

/// Checks `tarball`'s sha256 against the lock and unpacks it, fresh, into `into`. Returns
/// the one directory it holds.
fn unpack(dir: &Path, (name, sum): &(String, String), into: &Path) -> PathBuf {
    let tarball = dir.join(name);
    println!("cargo:rerun-if-changed={}", tarball.display());
    if !tarball.is_file() {
        panic!(
            "{} is missing: run `just ghostty-vt`, which fetches and checks it",
            tarball.display()
        );
    }
    // The copy is what is checked and unpacked, so the cache cannot change it in between.
    let copy = into.with_extension("tar.gz");
    fs::copy(&tarball, &copy).expect("copy the tarball");
    let have = sha256(&copy);
    if have != *sum {
        panic!("{name}: sha256 {have} is not the lock's {sum}; run `just ghostty-vt` again");
    }
    let _ = fs::remove_dir_all(into);
    fs::create_dir_all(into).expect("create the unpack directory");
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&copy)
        .arg("-C")
        .arg(into));
    let mut entries: Vec<PathBuf> = fs::read_dir(into)
        .expect("read the unpacked tarball")
        .map(|e| e.expect("read an unpacked entry").path())
        .collect();
    if entries.len() != 1 || !entries[0].is_dir() {
        panic!("{name} does not unpack into one directory");
    }
    entries.remove(0)
}

/// Applies the lock's patches to the unpacked source `src`, in order, each copied into `into`
/// and checked against its sha256 first; then checks the sha256 of every file the lock lists
/// as patched. Fails if `dir` holds a patch the lock does not list.
fn apply_patches(dir: &Path, src: &Path, into: &Path, lock: &Lock) {
    if dir.exists() {
        println!("cargo:rerun-if-changed={}", dir.display());
    }
    let mut present: Vec<String> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| {
                    e.expect("read a patch")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        })
        .unwrap_or_default();
    present.sort();
    let mut listed: Vec<String> = lock.patches.iter().map(|(name, _)| name.clone()).collect();
    listed.sort();
    if present != listed {
        panic!("patches/ holds {present:?}, but ghostty-vt.lock lists {listed:?}");
    }
    let _ = fs::remove_dir_all(into);
    fs::create_dir_all(into).expect("create the patch directory");
    let mut touched = Vec::new();
    for (name, sum) in &lock.patches {
        // The copy is what is checked and applied, as with the tarballs.
        let copy = into.join(name);
        fs::copy(dir.join(name), &copy).expect("copy a patch");
        let have = sha256(&copy);
        if have != *sum {
            panic!("patches/{name}: sha256 {have} is not the lock's {sum}");
        }
        touched.extend(changed_files(name, &copy));
        run(Command::new("patch")
            .current_dir(src)
            .args(["-p1", "--batch", "--forward", "--fuzz=0", "-i"])
            .arg(&copy)
            .stdout(Stdio::null()));
    }
    touched.sort();
    touched.dedup();
    let mut hashed: Vec<String> = lock.patched.iter().map(|(path, _)| path.clone()).collect();
    hashed.sort();
    if touched != hashed {
        panic!("the patches change {touched:?}, but ghostty-vt.lock hashes {hashed:?}");
    }
    for (path, sum) in &lock.patched {
        let have = sha256(&src.join(path));
        if have != *sum {
            panic!("patched {path}: sha256 {have} is not the lock's {sum}");
        }
    }
}

/// The files a patch changes, from its `+++ b/` lines. Fails on what a plain change to
/// existing files never holds: a path leaving the source, or git's headers for renames,
/// copies, modes, new or deleted files and binary data.
fn changed_files(name: &str, patch: &Path) -> Vec<String> {
    let text = fs::read_to_string(patch).expect("read a patch");
    let mut files = Vec::new();
    for line in text.lines() {
        let refused = [
            "rename ",
            "copy ",
            "old mode",
            "new mode",
            "new file mode",
            "deleted file mode",
            "similarity index",
            "GIT binary patch",
            "Binary files",
        ];
        if refused.iter().any(|head| line.starts_with(head)) {
            panic!("patches/{name}: {line:?} is not a plain change to an existing file");
        }
        let Some(rest) = line.strip_prefix("+++ ") else {
            continue;
        };
        let path = rest.split('\t').next().unwrap_or(rest);
        let Some(path) = path.strip_prefix("b/") else {
            panic!("patches/{name}: {path:?} does not start with b/");
        };
        if path.starts_with('/') || path.split('/').any(|part| part == ".." || part.is_empty()) {
            panic!("patches/{name}: {path:?} leaves the library's source");
        }
        files.push(path.to_owned());
    }
    files
}

fn sha256(file: &Path) -> String {
    let output = Command::new("sha256sum")
        .arg(file)
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .or_else(|| {
            Command::new("shasum")
                .args(["-a", "256"])
                .arg(file)
                .output()
                .ok()
                .filter(|o| o.status.success())
        })
        .expect("sha256sum or shasum");
    let text = String::from_utf8(output.stdout).expect("the sha256 tool's output");
    text.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Fails unless the dependency tarball holds exactly the packages the lock lists, each of
/// which third-party.md describes (ADR 0120).
fn check_packages(deps: &Path, listed: &[String]) {
    let mut found: Vec<String> = fs::read_dir(deps)
        .expect("read the dependency directory")
        .map(|e| {
            e.expect("read a package")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    let mut listed = listed.to_vec();
    listed.sort();
    if found != listed {
        panic!(
            "the dependency tarball holds {found:?}, but ghostty-vt.lock lists {listed:?}: \
             read each package and update the lock and third-party.md"
        );
    }
}

fn check_zig(zig: &std::ffi::OsStr, wanted: &str) {
    let output = Command::new(zig)
        .arg("version")
        .output()
        .unwrap_or_else(|e| panic!("could not run {zig:?} (set ZIG to Zig {wanted}): {e}"));
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version != wanted && !version.starts_with(&format!("{wanted}.")) {
        panic!("Zig {version} found, but libghostty-vt needs Zig {wanted} (set ZIG)");
    }
}

fn zig_build(zig: &std::ffi::OsStr, src: &Path, deps: &Path, out: &Path) -> PathBuf {
    let optimize = env::var("OSO_GHOSTTY_VT_OPTIMIZE").unwrap_or_else(|_| "ReleaseFast".into());
    if !OPTIMIZE_MODES.contains(&optimize.as_str()) {
        panic!("OSO_GHOSTTY_VT_OPTIMIZE must be one of {OPTIMIZE_MODES:?}, not {optimize:?}");
    }
    let prefix = out.join("zig-out");
    // Zig 0.17 fails ("reached unreachable code") on a global cache that already holds a
    // package archive, so each run starts with an empty one.
    let global_cache = out.join("zig-global-cache");
    let _ = fs::remove_dir_all(&global_cache);
    fs::create_dir_all(&global_cache).expect("create Zig's global cache");
    let mut command = Command::new(zig);
    command.arg("build");
    if let Some(target) = zig_target() {
        command.arg(format!("-Dtarget={target}"));
    }
    run(command
        .current_dir(src)
        .env("ZIG_GLOBAL_CACHE_DIR", &global_cache)
        .arg("--system")
        .arg(deps)
        .arg("-Demit-lib-vt")
        .arg(format!("-Doptimize={optimize}"))
        .arg("-Dsimd=true")
        .arg("-Demit-xcframework=false")
        .arg("-Demit-themes=false")
        .arg("--prefix")
        .arg(&prefix)
        .arg("--cache-dir")
        .arg(out.join("zig-cache")));
    let archive = prefix.join("lib/libghostty-vt.a");
    if !archive.is_file() {
        panic!("zig build did not produce {}", archive.display());
    }
    archive
}

/// Zig's name for cargo's target, when it is not the host: Zig builds for the host otherwise.
fn zig_target() -> Option<String> {
    let target = env::var("TARGET").expect("TARGET");
    if env::var("HOST").is_ok_and(|host| host == target) {
        return None;
    }
    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH");
    let os = match env::var("CARGO_CFG_TARGET_OS")
        .expect("CARGO_CFG_TARGET_OS")
        .as_str()
    {
        "macos" => "macos".to_owned(),
        "linux" => {
            let libc = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
            format!("linux-{}", if libc.is_empty() { "gnu" } else { &libc })
        }
        other => panic!("no Zig target for {target} ({other})"),
    };
    Some(format!("{arch}-{os}"))
}

fn run(command: &mut Command) {
    let status = command
        .status()
        .unwrap_or_else(|e| panic!("could not run {command:?}: {e}"));
    if !status.success() {
        panic!("{command:?} failed: {status}");
    }
}
