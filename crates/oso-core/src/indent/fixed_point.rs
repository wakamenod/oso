//! The rustfmt fixed point (ADR 0115): reindenting every line of every `.rs` file in this
//! repository by Rust's rules changes no line, apart from the few shapes the rules do not
//! cover, listed in [`NOT_COVERED`] with the reason for each. It takes about 0.6 s in a
//! debug build, so it runs with the other tests.
//!
//! Must not depend on: `buffer` or `editor`. It reads only files under the repository's
//! root, follows no link, and runs nothing.

use std::fs;
use std::path::{Path, PathBuf};

use super::rust;
use super::tests::misindented;

/// The files with lines the rules indent other than rustfmt does, how many, and why. Each
/// is a shape the rules leave to the person, as the ADR's Consequences say.
const NOT_COVERED: &[(&str, usize, &str)] = &[
    (
        "crates/oso-render/src/rows.rs",
        22,
        "the body of a `proptest!` invocation, which rustfmt does not format",
    ),
    (
        "crates/oso/src/dispatch/prompts.rs",
        3,
        "patterns after `|` inside parentheses sit with the first one",
    ),
    (
        "crates/oso-render/src/lib.rs",
        1,
        "patterns after `|` inside parentheses sit with the first one",
    ),
    (
        "crates/oso-platform/src/text/coretext/direction.rs",
        1,
        "an operand of `&&` inside an operand of `||` is one level further in",
    ),
    (
        "crates/oso-core/src/syntax/highlights/tests.rs",
        1,
        "a chain on a tuple is one level past the tuple's closer",
    ),
];

/// The repository's root: two levels above this crate, where the workspace's manifest is.
fn root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    assert!(
        manifest.contains("[workspace]"),
        "{} is not the workspace",
        root.display()
    );
    root
}

/// Every `.rs` file under `dir`, past directories whose names start with `.` (git's, and
/// worktrees under `.claude`), `target` or `target-*`, following no link.
fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir()
            && !name.starts_with('.')
            && name != "target"
            && !name.starts_with("target-")
        {
            rust_files(&entry.path(), files);
        } else if kind.is_file() && name.ends_with(".rs") {
            files.push(entry.path());
        }
    }
}

#[test]
fn reindenting_this_repository_changes_nothing() {
    let root = root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    assert!(files.len() > 100, "found only {} files", files.len());
    let mut found = Vec::new();
    let mut seen = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.split('\n').collect();
        let path = path.strip_prefix(&root).unwrap_or(path).to_string_lossy();
        let path = path.replace('\\', "/");
        let wrong = misindented(rust, &text);
        seen.push(path.clone());
        let listed = NOT_COVERED.iter().find(|(listed, _, _)| *listed == path);
        match listed {
            Some(&(_, count, _)) if wrong.len() == count => {}
            Some(&(_, count, _)) if wrong.is_empty() => {
                found.push(format!("{path}: listed with {count} lines, has none"));
            }
            _ => found.extend(wrong.into_iter().map(|(row, answer, written)| {
                let line = lines[row].trim_start();
                format!("{path}:{}: {answer} for {written}: {line}", row + 1)
            })),
        }
    }
    for (listed, _, _) in NOT_COVERED {
        if !seen.iter().any(|path| path == listed) {
            found.push(format!("{listed}: listed, but no such file"));
        }
    }
    assert!(
        found.is_empty(),
        "Rust's indentation rules (crates/oso-core/src/indent/rust.rs) indent {} lines of this \
         repository other than rustfmt does; fix the rules, or list the file in NOT_COVERED in \
         crates/oso-core/src/indent/fixed_point.rs with the reason:\n{}",
        found.len(),
        found.join("\n")
    );
}
