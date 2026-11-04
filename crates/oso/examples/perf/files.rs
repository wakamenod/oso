//! Test files for the benchmarks, generated into a temporary directory and
//! never committed.
//!
//! Must not depend on: the app or the control socket.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Mixed text: code, tabs, Japanese, emoji and combining marks.
const MIXED_LINES: [&str; 6] = [
    "fn main() {\n",
    "\tlet value = compute(42); // 計算する\n",
    "\t\tprintln!(\"{value} 👍\");\n",
    "}\n",
    "日本語のテキストと English text が混ざった行です。\n",
    "e\u{301}a\u{308} combining marks, and a longer line of plain ASCII text to fill the row\n",
];

/// One repeated piece of a very long line.
const LONG_LINE_PIECE: &str = "abc\tdef 日本語 x\u{301} 👍 ";

/// Creates a fresh, private directory: never one that is already there.
#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(path)
}

/// Creates a fresh directory: never one that is already there.
#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

/// A temporary directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new directory; `tag` keeps uses in one process apart.
    pub fn new(tag: &str) -> io::Result<Self> {
        let name = format!("oso-perf-{}-{tag}", std::process::id());
        let path = std::env::temp_dir().join(name);
        create_private_dir(&path)?;
        Ok(Self(path))
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A new file, for writing; never one that is already there.
fn create_new(path: &Path) -> io::Result<BufWriter<File>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok(BufWriter::with_capacity(1 << 20, file))
}

/// Writes about `bytes` of mixed text, at least that many.
pub fn write_mixed(path: &Path, bytes: u64) -> io::Result<()> {
    let mut out = create_new(path)?;
    let mut written = 0;
    for line in MIXED_LINES.iter().cycle() {
        if written >= bytes {
            break;
        }
        out.write_all(line.as_bytes())?;
        written += line.len() as u64;
    }
    out.flush()
}

/// Writes one line of at least `bytes`, with no newline.
pub fn write_long_line(path: &Path, bytes: u64) -> io::Result<()> {
    let mut out = create_new(path)?;
    let mut written = 0;
    while written < bytes {
        out.write_all(LONG_LINE_PIECE.as_bytes())?;
        written += LONG_LINE_PIECE.len() as u64;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::{TempDir, write_long_line, write_mixed};
    use std::fs;

    #[test]
    fn writes_the_requested_size_of_valid_text() {
        let dir = TempDir::new("sizes").unwrap();
        let mixed = dir.file("mixed.txt");
        write_mixed(&mixed, 10_000).unwrap();
        let text = fs::read_to_string(&mixed).unwrap();
        assert!(
            text.len() >= 10_000 && text.len() < 10_200,
            "{}",
            text.len()
        );
        assert!(text.lines().count() > 100);

        let long = dir.file("long.txt");
        write_long_line(&long, 10_000).unwrap();
        let text = fs::read_to_string(&long).unwrap();
        assert!(text.len() >= 10_000);
        assert_eq!(text.lines().count(), 1);

        let path = dir.file("");
        drop(dir);
        assert!(!path.exists(), "the directory is removed");
    }

    #[test]
    fn refuses_to_write_over_what_is_there() {
        let dir = TempDir::new("over").unwrap();
        let file = dir.file("a.txt");
        fs::write(&file, "keep").unwrap();
        assert!(write_mixed(&file, 10).is_err());
        assert!(write_long_line(&file, 10).is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
        assert!(TempDir::new("over").is_err(), "the directory exists");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_planted_symlink_and_keeps_the_directory_private() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = TempDir::new("private").unwrap();
        let mode = fs::metadata(dir.file("")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let target = dir.file("target.txt");
        fs::write(&target, "keep").unwrap();
        let link = dir.file("link.txt");
        symlink(&target, &link).unwrap();
        assert!(write_mixed(&link, 10).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
    }
}
