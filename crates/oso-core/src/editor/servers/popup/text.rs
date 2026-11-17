//! The text the completion popup reads (ADR 0138, step 9): the identifier before the cursor,
//! a range's text, compared where it lies, an offset moved through the edits since a revision,
//! and a row's text as it shows.
//!
//! Must not depend on: `command`, `keymap` or the host.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::Buffer;
use crate::rope::Rope;
use crate::selection::{OffsetEdit, follow_sorted};
use crate::sum_tree::Bias;

use super::{SHOWN_BYTES_MAX, WORD_BYTES_MAX};

/// `offset` in `buffer`'s text at `revision` moved through its edits since, as the cursors
/// move; `None` once its log no longer reaches back there.
pub(super) fn moved_to(buffer: &Buffer, revision: u64, offset: usize) -> Option<usize> {
    let len = buffer.text().len();
    if buffer.revision() == revision {
        return Some(offset.min(len));
    }
    let mut edits: Vec<OffsetEdit> = (buffer.edits_since(revision)?.iter())
        .map(|splice| OffsetEdit {
            start: splice.start,
            old_len: splice.old_len,
            new_len: splice.new_len,
        })
        .collect();
    edits.sort_unstable_by_key(|edit| edit.start);
    let mut moved = [offset];
    follow_sorted(&mut moved, &edits, Bias::Left);
    Some(moved[0].min(len))
}

/// `text` as a row shows it: its runs of blanks as one, at most [`SHOWN_BYTES_MAX`] of it, cut
/// at a character.
pub(super) fn shown_text(text: &str) -> Box<str> {
    let mut shown = String::with_capacity(text.len().min(SHOWN_BYTES_MAX));
    for word in text.split_whitespace() {
        if shown.len() >= SHOWN_BYTES_MAX {
            break;
        }
        if !shown.is_empty() {
            shown.push(' ');
        }
        shown.push_str(word);
    }
    if shown.len() > SHOWN_BYTES_MAX {
        let end = shown.floor_char_boundary(SHOWN_BYTES_MAX);
        shown.truncate(end);
    }
    shown.into()
}

/// The text of `range`, at most [`WORD_BYTES_MAX`] of it.
pub(super) fn text_of(text: &Rope, range: Range<usize>) -> String {
    let start = text.clip_offset(range.start.min(range.end), Bias::Left);
    let end = text.clip_offset(range.end.min(start + WORD_BYTES_MAX), Bias::Left);
    let mut word = String::with_capacity(end - start);
    for chunk in text.chunks_in_range(start..end) {
        word.push_str(chunk);
    }
    word
}

/// Whether the text of `range` is `word`, compared where it lies.
pub(super) fn text_is(text: &Rope, range: Range<usize>, word: &str) -> bool {
    if range.len() != word.len() {
        return false;
    }
    let mut rest = word.as_bytes();
    for chunk in text.chunks_in_range(range) {
        let Some(tail) = rest.strip_prefix(chunk.as_bytes()) else {
            return false;
        };
        rest = tail;
    }
    rest.is_empty()
}

/// Whether the text of `range` holds `c`.
pub(super) fn text_holds(text: &Rope, range: Range<usize>, c: char) -> bool {
    text.chunks_in_range(range).any(|chunk| chunk.contains(c))
}

/// Where the identifier before `cursor` starts: whole grapheme clusters whose first character
/// is alphanumeric or `_`, within [`WORD_BYTES_MAX`] and the cursor's line.
pub(super) fn word_start(text: &Rope, cursor: usize) -> usize {
    let line = text.line_start(text.offset_to_point(cursor).row);
    let from = line.max(cursor.saturating_sub(WORD_BYTES_MAX));
    let before = text_of(text, from..cursor);
    let kept: usize = before
        .graphemes(true)
        .rev()
        .take_while(|cluster| {
            cluster
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
        .map(str::len)
        .sum();
    cursor - kept
}
