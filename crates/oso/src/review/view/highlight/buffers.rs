//! An open buffer's highlights taken for a review's new side (ADR 0122, step 9): the buffer
//! the host hands in, compared with the side's text, its runs projected when they are the same.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; the host hands in the buffer.
//!
//! - A buffer offered is kept, the latest of each path, and tried with the next batch, so that
//!   offers that come together cost one comparison. Only the review's files' are kept once it
//!   lists them, and none while highlighting is off.
//! - It is tried again when its file shows a new reading or comes on screen, so that runs the
//!   budget dropped come back from it.
//! - Its text is compared with the side's once a revision, and its runs taken each time it is
//!   tried while they are the same: all the lines the review shows once the buffer's are whole,
//!   which closes the side's document or keeps it from opening, else those on screen.
//! - Never while highlighting is off, for a file the text cut short, or from a buffer in
//!   another language than the side's path says.

use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;
use oso_syntax::{Language, MAX_BYTES};

use super::sides::{self, index, lines_of, on_screen};
use super::{Highlighting, project};
use crate::review::Content;

/// An open buffer's text and highlights, as of its revision, for the new side of the file at
/// `path` (a path as git spells it), whose text it may be: its highlights are then that side's,
/// and no document is parsed for it once they are whole.
pub(crate) struct BufferSide {
    pub(crate) path: Box<[u8]>,
    pub(crate) revision: u64,
    pub(crate) text: Rope,
    pub(crate) highlights: Highlights,
    /// Whether its whole text is highlighted at that revision.
    pub(crate) whole: bool,
    /// The language its highlights are of.
    pub(crate) language: Option<Language>,
}

impl Highlighting {
    /// Keeps an open buffer's text and highlights, for the new side of the file at its path,
    /// to try with the next batch.
    pub(in crate::review::view) fn offer_buffer(&mut self, buffer: BufferSide) {
        if !self.is_on() || buffer.text.len() > MAX_BYTES {
            return;
        }
        let path = buffer.path.clone();
        self.buffers.insert(path.clone(), buffer);
        self.pending_buffers.insert(path);
    }

    /// Tries the buffers waiting, but those of files not laid out yet, which wait on; whether
    /// that changed what the text shows.
    pub(super) fn take_buffers(&mut self, order: &[Box<[u8]>]) -> bool {
        let pending: Vec<Box<[u8]>> = self.pending_buffers.drain().collect();
        let mut changed = false;
        for path in pending {
            if self.laid.contains(&path) {
                changed |= self.try_buffer(&path, order);
            } else if self.files.contains_key(&path) {
                self.pending_buffers.insert(path);
            }
        }
        changed
    }

    /// Keeps only the buffers of the files the review lists.
    pub(in crate::review::view) fn keep_listed_buffers(&mut self) {
        let files = &self.files;
        self.buffers.retain(|path, _| files.contains_key(path));
        self.pending_buffers.retain(|path| files.contains_key(path));
    }

    /// Takes the runs of the open buffer at `path` as its file's new side's, if its text is
    /// that side's; whether that changed what the text shows.
    fn try_buffer(&mut self, path: &[u8], order: &[Box<[u8]>]) -> bool {
        let shown = self.shown.get(path).map(|runs| &runs[index(Side::New)]);
        let (Some(buffer), Some(file)) = (self.buffers.get(path), self.files.get_mut(path)) else {
            return false;
        };
        let Some(reading) = &file.reading else {
            return false;
        };
        let Content::Text { shape, new, .. } = &reading.content else {
            return false;
        };
        let state = &mut file.sides[index(Side::New)];
        if state.complete
            || state.stuck
            || new.len() > MAX_BYTES
            || buffer.language.is_none()
            || buffer.language != sides::language(reading, Side::New)
        {
            return false;
        }
        let same = match state.buffer_same {
            Some((revision, same)) if revision == buffer.revision => same,
            _ => {
                let same = sides::same_text(new, &buffer.text);
                state.buffer_same = Some((buffer.revision, same));
                same
            }
        };
        if !same {
            return false;
        }
        let numbers = lines_of(shape.hunks(), &file.context[index(Side::New)], Side::New);
        let bom = sides::has_bom(new);
        let projected = if buffer.whole {
            project::project(&buffer.text, bom, &buffer.highlights, numbers, true)
        } else {
            let shown = shown.map_or(&[][..], Vec::as_slice);
            let numbers = (numbers.into_iter()).filter(|number| on_screen(shown, *number));
            project::project(&buffer.text, bom, &buffer.highlights, numbers, false)
        };
        if projected.lines() == 0 {
            return false;
        }
        if projected.done {
            // Its whole runs are the side's: its own document is of no more use.
            self.close(path, Side::New);
        }
        self.keep(path, Side::New, projected, order);
        true
    }
}
