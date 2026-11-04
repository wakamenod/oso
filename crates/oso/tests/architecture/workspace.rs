//! The workspace on disk: its root, its crates and their manifests, and its
//! Rust files.
//!
//! Must not depend on: the layer rules or any check's verdict; it finds and
//! reads files, and parses manifests through `manifests`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::manifests::{Manifest, workspace_renames};

/// A workspace checkout: this one, or an older one for the health report.
pub struct Workspace {
    root: PathBuf,
}

/// A workspace crate: its directory and its parsed manifest.
pub struct Crate {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

pub fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The lines of a rules or manifest file that hold data, with their index:
/// comments (`#` to the end of the line) and trailing whitespace removed,
/// blank lines skipped. Leading whitespace is kept.
pub fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim_end())
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
}

/// Whether the walk skips the directory `name`: build output (a fuzz
/// workspace has its own `target/`) and hidden directories.
fn is_skipped_dir(name: &str) -> bool {
    name == "target" || name.starts_with('.')
}

/// Every `.rs` file under `dir`, sorted. Symbolic links are not followed.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let entry = entry.expect("read directory entry");
            let kind = entry.file_type().expect("read file type");
            let path = entry.path();
            if kind.is_dir() && !is_skipped_dir(&entry.file_name().to_string_lossy()) {
                pending.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

impl Workspace {
    /// The workspace this test belongs to.
    pub fn current() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Self::at(&root)
    }

    pub fn at(root: &Path) -> Self {
        let root = root
            .canonicalize()
            .unwrap_or_else(|e| panic!("{}: {e}", root.display()));
        Self { root }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Every crate under `crates/`, sorted by directory. An older checkout
    /// without `crates/` has none.
    pub fn crates(&self) -> Vec<Crate> {
        let root_manifest = self.path("Cargo.toml");
        let renames = workspace_renames(&read(&root_manifest))
            .unwrap_or_else(|e| panic!("{}: {e}", root_manifest.display()));
        let mut dirs: Vec<PathBuf> = fs::read_dir(self.path("crates"))
            .into_iter()
            .flatten()
            .map(|entry| entry.expect("read crates/ entry").path())
            .filter(|path| path.join("Cargo.toml").is_file())
            .collect();
        dirs.sort();
        dirs.into_iter()
            .map(|dir| {
                let path = dir.join("Cargo.toml");
                let manifest = Manifest::parse(&read(&path), &renames)
                    .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                Crate { dir, manifest }
            })
            .collect()
    }

    /// Every Rust file under `crates/`.
    pub fn rust_files(&self) -> Vec<PathBuf> {
        rust_files(&self.path("crates"))
    }

    /// `path` relative to the workspace root, with `/` separators.
    pub fn relative_path(&self, path: &Path) -> String {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let relative = path.strip_prefix(&self.root).unwrap_or(&path);
        relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::{data_lines, is_skipped_dir};

    #[test]
    fn the_walk_skips_build_output_and_hidden_directories() {
        assert!(is_skipped_dir("target"));
        assert!(is_skipped_dir(".git"));
        assert!(!is_skipped_dir("src"));
        assert!(!is_skipped_dir("fuzz"));
    }

    #[test]
    fn data_lines_drop_comments_and_blanks_but_keep_indentation() {
        let lines: Vec<_> = data_lines("# c\n\na # x\n  b\n").collect();
        assert_eq!(lines, [(2, "a"), (3, "  b")]);
    }
}
