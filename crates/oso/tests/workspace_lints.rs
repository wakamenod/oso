//! Guards the workspace lints (ADR 0021). Each crate opts in to
//! `[workspace.lints]`, and the process and network bans in the root
//! `clippy.toml` (ADR 0010) apply only through it. A crate that drops the
//! opt-in, or a `clippy.toml` that shadows the root one, would silently fail
//! open, so this test fails instead.
//!
//! Must not depend on: any crate outside `std`; it only reads files.

use std::fs;
use std::path::{Path, PathBuf};

/// Every directory under `crates/`.
fn crate_dirs() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&crates)
        .expect("read crates/")
        .map(|entry| entry.expect("read crates/ entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Whether `manifest` has a `[lints]` table containing `workspace = true`.
fn opts_in_to_workspace_lints(manifest: &str) -> bool {
    let mut in_lints = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_lints = line == "[lints]";
        } else if in_lints && line.replace(' ', "") == "workspace=true" {
            return true;
        }
    }
    false
}

#[test]
fn every_crate_uses_the_workspace_lints() {
    let dirs = crate_dirs();
    assert!(dirs.len() >= 4, "expected the four crates, found {dirs:?}");
    for dir in dirs {
        let manifest = fs::read_to_string(dir.join("Cargo.toml"))
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        assert!(
            opts_in_to_workspace_lints(&manifest),
            "{} needs `[lints] workspace = true`",
            dir.display()
        );
    }
}

#[test]
fn no_crate_shadows_the_root_clippy_config() {
    for dir in crate_dirs() {
        for name in ["clippy.toml", ".clippy.toml"] {
            let path = dir.join(name);
            assert!(
                !path.exists(),
                "{} shadows the root clippy.toml",
                path.display()
            );
        }
    }
}

#[test]
fn the_lints_check_detects_a_missing_opt_in() {
    assert!(opts_in_to_workspace_lints("[lints]\nworkspace = true\n"));
    assert!(!opts_in_to_workspace_lints("[package]\nname = \"x\"\n"));
    assert!(!opts_in_to_workspace_lints(
        "[lints]\n[dependencies]\nworkspace = true\n"
    ));
}
