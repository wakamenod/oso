//! The record of trusted projects (ADR 0114): the text of `trusted-projects`, read and written
//! as plain data. Each line is a root's absolute path, a tab, and the root directory's device
//! and inode; a system with no stable identity records the path alone.
//!
//! Must not depend on: `buffer`, `editor` or file I/O. The host reads and writes the file on a
//! thread of its own, and this says what its text means.

use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use super::RootIdentity;

/// What the file says first, for whoever opens it.
const HEADER: &str =
    "# Projects oso trusts to run their code. Written by oso; delete a line to stop trusting.\n";

/// The roots trusted, each by its path and, where the system has them, its device and inode.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustList {
    roots: Vec<RootIdentity>,
}

/// A change a person made to the record, which the host applies to the file as it is when it
/// writes, so that what was changed there since it was read stays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustChange {
    /// Trust this root, in place of any line for its path.
    Trust(RootIdentity),
    /// Take out any line for this path.
    Untrust(PathBuf),
}

/// Why a root cannot be written in the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotRecordable {
    /// Its path is not UTF-8 text.
    NotUtf8,
    /// Its path holds a control character, which a line could not keep, or an invisible one,
    /// which would make a line read as another where it is not boxed.
    Control,
    /// Its path is not absolute, holds `..`, or is not spelled as its components are.
    NotPlain,
}

impl std::fmt::Display for NotRecordable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotUtf8 => "its path is not UTF-8",
            Self::Control => "its path holds a control or invisible character",
            Self::NotPlain => "its path is not a plain absolute one",
        })
    }
}

impl TrustList {
    /// The roots `text` records, and how many lines that are neither blank nor a comment it
    /// could not read, which are skipped.
    pub fn parse(text: &str) -> (Self, usize) {
        let mut list = Self::default();
        let mut skipped = 0;
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match parse_line(line) {
                Some(root) if !list.roots.contains(&root) => list.roots.push(root),
                Some(_) => {}
                None => skipped += 1,
            }
        }
        (list, skipped)
    }

    /// The file's text: the header, then a line for each root.
    pub fn format(&self) -> String {
        let mut text = HEADER.to_owned();
        for root in &self.roots {
            // A root is recorded only once checked, so its path is UTF-8.
            text.push_str(&root.path.to_string_lossy());
            if let Some((dev, ino)) = root.identity {
                let _ = write!(text, "\tdev={dev} ino={ino}");
            }
            text.push('\n');
        }
        text
    }

    /// This list with `root` trusted too, in place of any line for its path.
    ///
    /// # Errors
    ///
    /// If its path cannot be written as a line.
    pub fn with(&self, root: RootIdentity) -> Result<Self, NotRecordable> {
        recordable(&root.path)?;
        let mut list = self.without(&root.path);
        list.roots.push(root);
        Ok(list)
    }

    /// This list with `change` made.
    ///
    /// # Errors
    ///
    /// If a root to trust cannot be written as a line.
    pub fn apply(&self, change: &TrustChange) -> Result<Self, NotRecordable> {
        match change {
            TrustChange::Trust(root) => self.with(root.clone()),
            TrustChange::Untrust(path) => Ok(self.without(path)),
        }
    }

    /// This list with no line for `path`, whatever identity it recorded.
    pub fn without(&self, path: &Path) -> Self {
        let roots = self
            .roots
            .iter()
            .filter(|root| root.path != path)
            .cloned()
            .collect();
        Self { roots }
    }

    /// Whether `root` is trusted: its path and its identity are both recorded, so a directory
    /// made again at the same path is not.
    pub fn trusts(&self, root: &RootIdentity) -> bool {
        self.roots.contains(root)
    }

    /// The paths of the roots trusted, for a claim to be matched against by its words.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.roots.iter().map(|root| root.path.as_path())
    }

    /// The roots trusted.
    pub(crate) fn roots(&self) -> &[RootIdentity] {
        &self.roots
    }

    /// Whether any line records `path`, whatever its identity.
    pub fn records(&self, path: &Path) -> bool {
        self.paths().any(|known| known == path)
    }
}

/// A line's root: an absolute path that [`recordable`] passes, then, unless the system records
/// the path alone, a tab and `dev=<n> ino=<n>`.
fn parse_line(line: &str) -> Option<RootIdentity> {
    let (path, identity) = match line.split_once('\t') {
        Some((path, rest)) => (path, Some(parse_identity(rest)?)),
        None => (line, None),
    };
    let path = PathBuf::from(path);
    recordable(&path).ok()?;
    Some(RootIdentity { path, identity })
}

fn parse_identity(text: &str) -> Option<(u64, u64)> {
    let (dev, ino) = text.split_once(' ')?;
    let number = |text: &str, key: &str| {
        let digits = text.strip_prefix(key)?;
        // Digits alone: `parse` would take a sign.
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse().ok()
    };
    Some((number(dev, "dev=")?, number(ino, "ino=")?))
}

/// Whether `c` is invisible where the file is read outside oso: a bidi control, a zero-width
/// or default-ignorable character, or a line or paragraph separator. The ranges are those
/// `display_map::cell::is_invisible` boxes, which this layer cannot use.
fn is_invisible(c: char) -> bool {
    matches!(
        u32::from(c),
        0x00ad
            | 0x034f
            | 0x061c
            | 0x17b4..=0x17b5
            | 0x180b..=0x180f
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x206f
            | 0x3164
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xffa0
            | 0xfff0..=0xfffb
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0000..=0xe0fff
    )
}

/// Whether `path` can be a line: UTF-8 with no control character, absolute, with no `..`, and
/// spelled as its components are, with no `.`, doubled separator or final one, so that it
/// reads one way only.
fn recordable(path: &Path) -> Result<(), NotRecordable> {
    let text = path.to_str().ok_or(NotRecordable::NotUtf8)?;
    if text.chars().any(|c| c.is_control() || is_invisible(c)) {
        return Err(NotRecordable::Control);
    }
    let components: PathBuf = path.components().collect();
    let plain = components.as_os_str() == path.as_os_str()
        && path
            .components()
            .all(|component| component != Component::ParentDir);
    if !path.is_absolute() || !plain {
        return Err(NotRecordable::NotPlain);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str, identity: Option<(u64, u64)>) -> RootIdentity {
        RootIdentity {
            path: PathBuf::from(path),
            identity,
        }
    }

    #[cfg(unix)]
    #[test]
    fn what_is_written_reads_back() {
        let list = TrustList::default()
            .with(root("/w/oso", Some((16_777_230, 48_211_907))))
            .unwrap()
            .with(root("/w/other", None))
            .unwrap();
        let text = list.format();
        assert_eq!(
            text,
            format!("{HEADER}/w/oso\tdev=16777230 ino=48211907\n/w/other\n")
        );
        assert_eq!(TrustList::parse(&text), (list, 0));
    }

    #[cfg(unix)]
    #[test]
    fn a_root_is_trusted_only_by_its_path_and_identity_both() {
        let (list, _) = TrustList::parse("/w/oso\tdev=1 ino=7\n");
        assert!(list.trusts(&root("/w/oso", Some((1, 7)))));
        assert!(!list.trusts(&root("/w/oso", Some((1, 8)))), "made again");
        assert!(!list.trusts(&root("/w/oso", None)));
        assert!(!list.trusts(&root("/w/oso/sub", Some((1, 7)))));
        assert!(list.records(Path::new("/w/oso")));
    }

    #[cfg(unix)]
    #[test]
    fn a_line_it_cannot_read_is_skipped_and_counted() {
        let text = "# a comment\n\
                    \n\
                    /w/a\tdev=1 ino=2\n\
                    relative\tdev=1 ino=2\n\
                    /w/../etc\tdev=1 ino=2\n\
                    /w/b\tdev=1\n\
                    /w/b\tdev=-1 ino=2\n\
                    /w/b\tdev=+1 ino=2\n\
                    /w/b\tdev=1 ino=2 more\n\
                    /w/b\tino=2 dev=1\n\
                    /w/b\tdev=1 ino=99999999999999999999\n\
                    /w/c\tdev=1 ino=3\r\n\
                    /w/a\tdev=1 ino=2\n";
        let (list, skipped) = TrustList::parse(text);
        assert_eq!(skipped, 8);
        let paths: Vec<&Path> = list.paths().collect();
        assert_eq!(paths, [Path::new("/w/a"), Path::new("/w/c")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_path_a_line_cannot_keep_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let list = TrustList::default();
        let not_utf8 = PathBuf::from(std::ffi::OsStr::from_bytes(b"/w/\xFF"));
        let refused = |path: PathBuf| {
            list.with(RootIdentity {
                path,
                identity: None,
            })
        };
        assert_eq!(refused(not_utf8), Err(NotRecordable::NotUtf8));
        for path in [
            "/w/a\tdev=1 ino=2",
            "/w/a\nb",
            "/w/\u{85}",
            "/w/\u{202e}oso",
            "/w/o\u{200b}so",
            "/w/a\u{2028}b",
        ] {
            assert_eq!(
                refused(PathBuf::from(path)),
                Err(NotRecordable::Control),
                "{path:?}"
            );
        }
        for path in ["w/a", "/w/../a", "/w/./a", "/w//a", "/w/a/"] {
            assert_eq!(
                refused(PathBuf::from(path)),
                Err(NotRecordable::NotPlain),
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn trusting_again_replaces_and_untrusting_removes_any_identity() {
        let list = TrustList::default()
            .with(root("/w/oso", Some((1, 7))))
            .unwrap()
            .with(root("/w/oso", Some((1, 9))))
            .unwrap();
        assert_eq!(list.paths().count(), 1);
        assert!(list.trusts(&root("/w/oso", Some((1, 9)))));
        let list = list.without(Path::new("/w/oso"));
        assert_eq!(list, TrustList::default());
        assert_eq!(list.format(), HEADER);
    }

    /// Windows records the path alone, as `std` keeps a directory's identity unstable there.
    #[cfg(windows)]
    #[test]
    fn windows_records_the_path_alone() {
        let list = TrustList::default().with(root(r"C:\w\oso", None)).unwrap();
        let text = list.format();
        assert_eq!(text, format!("{HEADER}C:\\w\\oso\n"));
        assert_eq!(TrustList::parse(&text), (list.clone(), 0));
        assert!(list.trusts(&root(r"C:\w\oso", None)));
        assert!(list.with(root(r"\w\oso", None)).is_err(), "no drive");
    }
}
