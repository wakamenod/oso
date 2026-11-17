//! What a rename's preview is built from, on the UI thread (ADR 0138, step 14): each file's
//! edits with a buffer's text, or its path to read, once judged as `applying` judges a file
//! before it opens; and the names and paths it shows.
//!
//! Must not depend on: the window system or the drawing; nothing here reads a file.

use std::path::Path;

use oso_lsp::server::ServerEdit;

use super::super::super::App;
use super::super::applying::{holders, judge_unsent};
use super::preview::{FileSnap, Snapshot, Source};

impl App {
    /// What the preview of `edit`, server `index`'s rename of `old` to `new`, is built from; or
    /// why the edit is refused before anything shows.
    pub(super) fn preview_snapshot(
        &self,
        index: usize,
        (old, new): (&str, &str),
        edit: &ServerEdit,
    ) -> Result<Snapshot, &'static str> {
        let running = &self.servers.running[index];
        let root = super::super::read_back(&running.root).unwrap_or_else(|| running.root.clone());
        let mut files = Vec::with_capacity(edit.open.len() + edit.unsent.len());
        for open in &edit.open {
            let buffer = running
                .buffer_of(open.document)
                .and_then(|id| self.editor.buffer(id))
                .ok_or("a buffer it edits is gone")?;
            if buffer.revision() != open.revision {
                return Err("a buffer it edits changed since");
            }
            if buffer.is_secret() {
                return Err("it edits a buffer that may hold secrets");
            }
            files.push(FileSnap {
                shown: shown_path(buffer.path(), &root, buffer.name()),
                source: Source::Text {
                    text: buffer.text().clone(),
                    edits: open.edits.clone(),
                },
            });
        }
        let paths: Vec<&Path> = (edit.unsent.iter())
            .map(|unsent| unsent.path.as_path())
            .collect();
        let held = holders(&self.editor, &paths);
        for unsent in &edit.unsent {
            let found = held.get(&unsent.path).copied();
            judge_unsent(&self.editor, &root, (&unsent.path, found))?;
            let shown = shown_path(Some(&unsent.path), &root, "");
            let source = match found.and_then(|id| self.editor.buffer(id)) {
                // A buffer that holds it, unmodified as judged, shows its text.
                Some(buffer) if buffer.loading().is_none() => {
                    let text = buffer.text();
                    let edits = (unsent.edits.iter())
                        .map(|(start, end, new)| {
                            let range =
                                oso_lsp::position::to_range(text, *start, *end, unsent.encoding);
                            (range, new.clone())
                        })
                        .collect();
                    Source::Text {
                        text: text.clone(),
                        edits,
                    }
                }
                _ => Source::File {
                    path: unsent.path.clone(),
                    edits: unsent.edits.clone(),
                    encoding: unsent.encoding,
                },
            };
            files.push(FileSnap { shown, source });
        }
        Ok(Snapshot {
            server: running.entry.name.clone(),
            old: old.to_owned(),
            new: new.to_owned(),
            root,
            home: super::super::super::home(),
            files,
        })
    }
}

/// The most bytes of a name, old or new.
pub(super) const NEW_NAME_MAX: usize = oso_lsp::limits::NEW_NAME_BYTES_MAX;

/// The identifier around `offset` of `text`: its letters, digits and `_`; `None` for none.
pub(super) fn identifier_at(text: &str, offset: usize) -> Option<String> {
    let offset = text.floor_char_boundary(offset.min(text.len()));
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let start = (text[..offset].char_indices().rev())
        .take_while(|(_, c)| word(*c))
        .last()
        .map_or(offset, |(at, _)| at);
    let end = (text[offset..].char_indices())
        .find(|(_, c)| !word(*c))
        .map_or(text.len(), |(at, _)| offset + at);
    (start < end && end - start <= NEW_NAME_MAX).then(|| text[start..end].to_owned())
}

/// `path` as the preview shows it: from `root`, spelled with `/`, else whole; `name` for a
/// buffer with no file.
pub(super) fn shown_path(path: Option<&Path>, root: &Path, name: &str) -> String {
    let Some(path) = path else {
        return name.to_owned();
    };
    match path.strip_prefix(root) {
        Ok(relative) => {
            let parts: Vec<String> = (relative.components())
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            parts.join("/")
        }
        Err(_) => path.to_string_lossy().into_owned(),
    }
}
