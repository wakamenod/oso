//! Watching the sources for changes, for `oso dev` (ADR 0023, ADR 0047).
//!
//! Must not depend on: `app`, `server` or anything about building; it only
//! notices that files changed.
//!
//! Shaders count, since they are compiled in until they can reload in a running
//! instance (ADR 0002). It polls modification times and sizes. A workspace has few enough files
//! that a look costs well under a millisecond, and this needs no dependency.
//! File-event crates would save part of one poll interval out of a restart
//! that takes seconds.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One file's identity for noticing changes.
type Entry = (PathBuf, Option<SystemTime>, u64);

/// The sources under a workspace root, as last seen.
#[derive(Debug)]
pub struct Sources {
    root: PathBuf,
    last: Vec<Entry>,
}

impl Sources {
    /// Starts watching the workspace at `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            last: snapshot(root),
        }
    }

    /// Whether anything was changed, added or removed since the last call.
    pub fn changed(&mut self) -> bool {
        let now = snapshot(&self.root);
        if now == self.last {
            return false;
        }
        self.last = now;
        true
    }
}

/// Every source file with its time and size, in a stable order. A file that
/// goes away during the walk is simply left out.
fn snapshot(root: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    for manifest in ["Cargo.toml", "Cargo.lock"] {
        add(&root.join(manifest), &mut entries);
    }
    walk(&root.join("crates"), &mut entries);
    entries.sort();
    entries
}

fn walk(directory: &Path, entries: &mut Vec<Entry>) {
    let Ok(children) = fs::read_dir(directory) else {
        return;
    };
    for child in children.flatten() {
        let path = child.path();
        let name = child.file_name();
        let name = name.to_string_lossy();
        if child.file_type().is_ok_and(|kind| kind.is_dir()) {
            if !name.starts_with('.') && name != "target" {
                walk(&path, entries);
            }
        } else if name == "Cargo.toml" || name.ends_with(".rs") || name.ends_with(".wgsl") {
            add(&path, entries);
        }
    }
}

fn add(path: &Path, entries: &mut Vec<Entry>) {
    if let Ok(metadata) = fs::metadata(path) {
        entries.push((path.to_owned(), metadata.modified().ok(), metadata.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::Sources;
    use std::fs;
    use std::path::PathBuf;

    fn workspace(name: &str) -> PathBuf {
        let name = format!("oso-watch-test-{}-{name}", std::process::id());
        let root = std::env::temp_dir().join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("crates/a/src")).unwrap();
        fs::write(root.join("crates/a/src/lib.rs"), "1").unwrap();
        fs::write(root.join("crates/a/Cargo.toml"), "1").unwrap();
        fs::write(root.join("Cargo.toml"), "1").unwrap();
        root
    }

    #[test]
    fn notices_changed_added_and_removed_sources() {
        let root = workspace("sources");
        let mut sources = Sources::new(&root);
        assert!(!sources.changed());

        let lib = root.join("crates/a/src/lib.rs");
        fs::write(&lib, "12").unwrap();
        assert!(sources.changed());
        assert!(!sources.changed(), "once per change");

        let new = root.join("crates/a/src/new.rs");
        fs::write(&new, "1").unwrap();
        assert!(sources.changed());
        fs::remove_file(&new).unwrap();
        assert!(sources.changed());

        fs::write(root.join("crates/a/src/text.wgsl"), "1").unwrap();
        assert!(sources.changed());
        fs::write(root.join("crates/a/Cargo.toml"), "12").unwrap();
        assert!(sources.changed());
        fs::write(root.join("Cargo.toml"), "12").unwrap();
        assert!(sources.changed());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignores_everything_else() {
        let root = workspace("others");
        let mut sources = Sources::new(&root);
        fs::write(root.join("crates/a/notes.md"), "x").unwrap();
        fs::create_dir_all(root.join("crates/a/target")).unwrap();
        fs::write(root.join("crates/a/target/built.rs"), "x").unwrap();
        fs::create_dir_all(root.join("crates/a/.hidden")).unwrap();
        fs::write(root.join("crates/a/.hidden/x.rs"), "x").unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/x.rs"), "x").unwrap();
        assert!(!sources.changed());
        fs::remove_dir_all(root).unwrap();
    }
}
