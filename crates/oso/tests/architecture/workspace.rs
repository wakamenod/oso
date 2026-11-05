//! The workspace on disk: its root, its crates and their manifests, its Rust
//! files and the text of every file that is not ignored.
//!
//! Must not depend on: the layer rules or any check's verdict; it finds and
//! reads files, and parses manifests through `manifests`.

use std::fs;
use std::io::ErrorKind;
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

/// Whether [`rust_files`] skips the directory `name`: build output (a fuzz
/// workspace has its own `target/`) and hidden directories.
fn is_skipped_dir(name: &str) -> bool {
    name == "target" || name.starts_with('.')
}

/// The file name of `path` as text, empty if it has none.
fn name(path: &Path) -> String {
    let name = path.file_name().unwrap_or_default();
    name.to_string_lossy().into_owned()
}

/// Whether the repository walk skips the directory at `relative`, a path from
/// the root. A test cannot ask git (ADR 0039), so this and [`is_ignored_file`]
/// copy `.gitignore` by hand, and nothing keeps them in step. `target` counts at
/// any depth, as a fuzz workspace has its own. Another session's half-finished
/// merge in a worktree must not fail this session's tests, and because the path
/// is relative, a checkout that is itself one of those worktrees still walks its
/// own files.
fn is_ignored_dir(relative: &Path) -> bool {
    let name = name(relative);
    name == ".git"
        || name == "target"
        || relative == Path::new(".claude/worktrees")
        || relative.to_string_lossy().starts_with("mutants.out")
}

/// Whether the repository walk skips the file at `relative`. In a linked
/// worktree `.git` is a file.
fn is_ignored_file(relative: &Path) -> bool {
    let name = name(relative);
    name == ".git"
        || name.ends_with(".snap.new")
        || name.ends_with(".pending-snap")
        || relative == Path::new(".claude/settings.local.json")
}

/// Every file under `dir`, sorted, leaving out the directories that `skip`
/// picks. Symbolic links are not followed.
fn files_under(dir: &Path, skip: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
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
            if kind.is_dir() && !skip(&path) {
                pending.push(path);
            } else if kind.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Every `.rs` file under `dir`, sorted. Symbolic links are not followed.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = files_under(dir, |path| is_skipped_dir(&name(path)));
    files.retain(|path| path.extension().is_some_and(|e| e == "rs"));
    files
}

/// The text of `path`, or nothing if it is not UTF-8 or has just gone, as an
/// editor's temporary file can.
fn read_text(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if matches!(e.kind(), ErrorKind::InvalidData | ErrorKind::NotFound) => None,
        Err(e) => panic!("{}: {e}", path.display()),
    }
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

    /// The path from the root and the text of every file in the repository,
    /// whatever its type, hidden directories included. Without git this means
    /// everything `.gitignore` does not cover, so also a file not yet added.
    pub fn repository_texts(&self) -> Vec<(String, String)> {
        let relative = |path: &Path| path.strip_prefix(&self.root).unwrap_or(path).to_path_buf();
        let mut files = files_under(&self.root, |dir| is_ignored_dir(&relative(dir)));
        files.retain(|file| !is_ignored_file(&relative(file)));
        files
            .into_iter()
            .filter_map(|file| Some((self.relative_path(&file), read_text(&file)?)))
            .collect()
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
    use std::fs;
    use std::path::Path;
    use std::process;

    use super::{Workspace, data_lines, is_skipped_dir};

    #[test]
    fn the_walk_skips_build_output_and_hidden_directories() {
        assert!(is_skipped_dir("target"));
        assert!(is_skipped_dir(".git"));
        assert!(!is_skipped_dir("src"));
        assert!(!is_skipped_dir("fuzz"));
    }

    /// Writes `text` to `relative` under `root`, creating its directories.
    fn write(root: &Path, relative: &str, text: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn the_repository_walk_reads_every_file_that_is_not_ignored() {
        let name = format!("repository-walk-{}", process::id());
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
        let read = [
            ".cargo/config.toml",
            ".claude/settings.json",
            ".claude/worktrees.md",
            ".github/workflows/ci.yml",
            "Cargo.toml",
            "crates/oso/src/snapshots/a.snap",
            // The rules that name a path hold only at the root.
            "docs/.claude/worktrees/a.md",
            "docs/a.md",
            "docs/mutants.out/a.md",
            "fuzz/src/a.rs",
            "guardrails/layers.txt",
        ];
        for relative in read {
            write(&root, relative, relative.as_bytes());
        }
        for ignored in [
            ".claude/settings.local.json",
            ".claude/worktrees/other/docs/a.md",
            ".git/MERGE_MSG",
            "crates/oso/src/snapshots/a.pending-snap",
            "crates/oso/src/snapshots/a.snap.new",
            "fuzz/target/a.md",
            "mutants.out/log",
            "mutants.out.old/log",
            "target/a.md",
            // A file here, as in a linked worktree.
            "vendored/.git",
        ] {
            write(&root, ignored, b"");
        }
        write(&root, "docs/not-utf8.txt", b"\xFF\n");

        let texts = Workspace::at(&root).repository_texts();
        fs::remove_dir_all(&root).unwrap();

        let expected: Vec<_> = read
            .iter()
            .map(|at| ((*at).to_owned(), (*at).to_owned()))
            .collect();
        assert_eq!(texts, expected);
    }

    #[test]
    fn data_lines_drop_comments_and_blanks_but_keep_indentation() {
        let lines: Vec<_> = data_lines("# c\n\na # x\n  b\n").collect();
        assert_eq!(lines, [(2, "a"), (3, "  b")]);
    }
}
