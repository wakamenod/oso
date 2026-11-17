//! An open buffer's highlights taken for a review's new side (ADR 0122, step 9): the buffer
//! the host hands in, compared with the side's text once a revision, its runs projected when
//! they are the same.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; the host hands in the buffer.

use oso_core::review::Side;
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

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
}

impl Highlighting {
    /// Takes an open buffer's text and highlights, for the new side of the file at its path.
    /// Whether that changed what the text shows.
    pub(in crate::review::view) fn buffer(
        &mut self,
        buffer: BufferSide,
        order: &[Box<[u8]>],
    ) -> bool {
        let path = buffer.path.clone();
        self.buffers.insert(path.clone(), buffer);
        self.try_buffer(&path, order)
    }

    /// Takes the runs of the open buffer at `path` as its file's new side's, if its text is
    /// that side's: all the lines the review shows of it once the buffer's are whole, which
    /// closes the side's document, else those on screen. Compared once a revision. Whether
    /// that changed what the text shows.
    pub(super) fn try_buffer(&mut self, path: &[u8], order: &[Box<[u8]>]) -> bool {
        let shown = self
            .shown
            .get(path)
            .map(|runs| runs[index(Side::New)].clone());
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
        let tried = Some((buffer.revision, buffer.whole));
        if state.complete || state.stuck || state.buffer_tried == tried {
            return false;
        }
        state.buffer_tried = tried;
        if !sides::same_text(new, &buffer.text) {
            return false;
        }
        let numbers = lines_of(shape.hunks(), Side::New);
        let bom = sides::has_bom(new);
        let projected = if buffer.whole {
            project::project(&buffer.text, bom, &buffer.highlights, numbers, true)
        } else {
            let shown = shown.unwrap_or_default();
            let numbers = numbers.filter(|number| on_screen(&shown, *number));
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
