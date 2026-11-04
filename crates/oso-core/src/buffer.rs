//! Buffers: the text of a file, with its display model (ADR 0022,
//! ADR 0040).
//!
//! Must not depend on: `window`, `editor` or `command`, or file I/O; a buffer
//! does not know who shows it or who reads the file.
//!
//! A file buffer starts empty and read-only, and fills in as the loading
//! thread hands over snapshots ([`Buffer::apply_load`]). Buffers with
//! invalid UTF-8, and binary files, stay read-only and can never be saved.

use std::path::{Path, PathBuf};

use crate::decode::{FileInfo, InvalidBytes};
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::load::{self, LoadSnapshot, Progress};
use crate::rope::Rope;
use crate::secret;

/// Identifies a buffer in its [`Editor`](crate::editor::Editor).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BufferId(pub(crate) u64);

impl BufferId {
    /// The number behind the id, for showing it outside the editor.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Why a buffer cannot be edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadOnly {
    /// The file is still loading.
    Loading,
    /// The file has invalid UTF-8, shown as `\xNN`. It can never be saved.
    InvalidUtf8,
    /// The file is binary (a NUL byte in its first 8 KB). It can never be
    /// saved.
    Binary,
}

impl ReadOnly {
    /// `"loading"`, `"invalid-utf8"` or `"binary"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::InvalidUtf8 => "invalid-utf8",
            Self::Binary => "binary",
        }
    }
}

/// A buffer: text, its display model, and where it came from.
pub struct Buffer {
    name: String,
    path: Option<PathBuf>,
    text: Rope,
    /// Also holds the invalid bytes (ADR 0040).
    display: DisplayMap,
    info: FileInfo,
    /// `Some` while the file loads.
    loading: Option<Progress>,
    /// Changes whenever the content does.
    revision: u64,
    /// The file may hold secrets (ADR 0043). True from the moment a file is
    /// visited until the host knows better.
    secret: bool,
}

impl Buffer {
    /// An empty buffer that visits no file, such as `*scratch*`.
    pub fn new(name: impl Into<String>) -> Self {
        Self::from_text(name, "")
    }

    /// A buffer holding `text`.
    pub fn from_text(name: impl Into<String>, text: &str) -> Self {
        let text = Rope::from(text);
        Self {
            name: name.into(),
            path: None,
            display: DisplayMap::new(&text, TabWidth::default()),
            text,
            info: FileInfo::default(),
            loading: None,
            revision: 0,
            secret: false,
        }
    }

    /// A buffer holding a file's `bytes`, decoded at once. For small
    /// content; files load in the background instead.
    pub fn from_bytes(name: impl Into<String>, bytes: &[u8]) -> Self {
        let mut buffer = Self::new(name);
        buffer.apply_load(load::decode_all(bytes, TabWidth::default()));
        buffer
    }

    /// An empty, read-only buffer for `path`, which a loading thread is
    /// about to fill.
    ///
    /// It is named after the file. Telling apart buffers of files with the
    /// same name (Emacs's uniquify) is left to the editor, which knows the
    /// other buffers.
    pub fn visiting(path: PathBuf) -> Self {
        let name = path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        Self {
            path: Some(path),
            loading: Some(Progress::default()),
            secret: true,
            ..Self::new(name)
        }
    }

    /// Replaces the content with what has loaded so far. O(1).
    pub fn apply_load(&mut self, snapshot: LoadSnapshot) {
        self.text = snapshot.text;
        self.display = snapshot.display;
        self.info = snapshot.info;
        self.loading = (!snapshot.done).then_some(snapshot.progress);
        self.revision += 1;
    }

    /// Ends loading with no content, for a file that does not exist yet.
    pub fn finish_empty(&mut self) {
        self.loading = None;
    }

    /// Decides whether the file may hold secrets, from its canonical path:
    /// `None` if the path could not be resolved, which counts as secret.
    /// The host resolves the path off the UI thread, before any content
    /// arrives.
    pub fn resolve_secret(&mut self, resolved: Option<&Path>) {
        self.secret = self
            .path
            .as_deref()
            .is_some_and(|path| secret::is_secret(path, resolved));
    }

    /// Whether the file may hold secrets (ADR 0028, ADR 0043). A file's text
    /// must not leave the editor through dev tooling or an emergency save
    /// while this is true.
    pub fn is_secret(&self) -> bool {
        self.secret
    }

    /// A number that changes whenever the content does, so that caches of
    /// it (shaped rows) know when to check it again.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The buffer's name, such as `main.rs` or `*scratch*`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The file the buffer visits, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The text.
    pub fn text(&self) -> &Rope {
        &self.text
    }

    /// A snapshot of the display model. O(1).
    pub fn display(&self) -> DisplaySnapshot {
        self.display.snapshot()
    }

    /// The invalid bytes of the file, each shown as `\xNN`.
    pub fn invalid_bytes(&self) -> &InvalidBytes {
        self.display.invalid_bytes()
    }

    /// The BOM and line ending of the file, and whether it is binary.
    pub fn file_info(&self) -> FileInfo {
        self.info
    }

    /// How far the file has loaded, while it loads.
    pub fn loading(&self) -> Option<Progress> {
        self.loading
    }

    /// Why the buffer cannot be edited, if it cannot. A reason that lasts
    /// wins over loading.
    pub fn read_only(&self) -> Option<ReadOnly> {
        if self.info.binary {
            Some(ReadOnly::Binary)
        } else if !self.invalid_bytes().is_empty() {
            Some(ReadOnly::InvalidUtf8)
        } else if self.loading.is_some() {
            Some(ReadOnly::Loading)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load::Builder;

    #[test]
    fn a_loading_buffer_is_read_only_until_done() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/a.txt"));
        assert_eq!(buffer.read_only(), Some(ReadOnly::Loading));
        assert_eq!(buffer.name(), "a.txt");
        let mut builder = Builder::new(Some(6), TabWidth::default());
        builder.push(b"abc");
        buffer.apply_load(builder.snapshot());
        assert_eq!(buffer.text().to_string(), "abc");
        assert_eq!(
            buffer.loading(),
            Some(Progress {
                read: 3,
                total: Some(6)
            })
        );
        builder.push(b"def");
        builder.finish();
        buffer.apply_load(builder.snapshot());
        assert_eq!(buffer.read_only(), None);
        assert_eq!(buffer.loading(), None);
        assert_eq!(buffer.display().row_cells(0), 6);
    }

    #[test]
    fn invalid_and_binary_files_stay_read_only() {
        let buffer = Buffer::from_bytes("bad", b"a\xff");
        assert_eq!(buffer.read_only(), Some(ReadOnly::InvalidUtf8));
        assert_eq!(buffer.invalid_bytes().len(), 1);
        let binary = Buffer::from_bytes("bin", b"a\0\xff");
        assert_eq!(binary.read_only(), Some(ReadOnly::Binary));
        assert_eq!(Buffer::from_bytes("ok", b"ok").read_only(), None);
    }

    #[test]
    fn a_missing_file_ends_as_an_empty_editable_buffer() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/new.txt"));
        buffer.finish_empty();
        assert_eq!(buffer.read_only(), None);
        assert!(buffer.text().is_empty());
        assert_eq!(buffer.path(), Some(Path::new("/tmp/new.txt")));
    }

    #[test]
    fn a_visited_file_is_secret_until_its_path_is_resolved() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/notes.txt"));
        assert!(buffer.is_secret(), "fails closed while unknown");
        buffer.resolve_secret(Some(Path::new("/tmp/notes.txt")));
        assert!(!buffer.is_secret());
        buffer.resolve_secret(Some(Path::new("/home/me/.ssh/id_rsa")));
        assert!(buffer.is_secret(), "a link to a secret");
        buffer.resolve_secret(None);
        assert!(buffer.is_secret(), "a path that cannot be resolved");

        let mut named = Buffer::visiting(PathBuf::from("/tmp/.env"));
        named.resolve_secret(Some(Path::new("/tmp/plain.txt")));
        assert!(named.is_secret(), "named like a secret, whatever it is");
    }

    #[test]
    fn a_buffer_without_a_file_is_not_secret() {
        let mut buffer = Buffer::new("*scratch*");
        assert!(!buffer.is_secret());
        buffer.resolve_secret(None);
        assert!(!buffer.is_secret());
    }
}
