//! What the view knows of a side before it opens it (ADR 0122, step 9): which text it is of,
//! the lines of it the review shows, the lines of it on screen, its language, and its text
//! decoded as the loader decodes a file.
//!
//! Must not depend on: `app`, `dispatch`, git's runner or the file system; these read a reading
//! they are given.
//!
//! - A side's text is decoded as the loader decodes it, each byte of an invalid sequence a
//!   U+FFFD and a first BOM left out, so that its offsets are those of the review's lines,
//!   whose bytes the section decodes the same way. No display map is built for it.

use std::cmp::Ordering;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use oso_core::buffer::Mode;
use oso_core::decode::BOM;
use oso_core::review::{Hunk, Side};
use oso_core::rope::Rope;
use oso_syntax::Language;

use crate::git::{Id, os_path};
use crate::review::{Content, Reading};

/// The most runs of lines on screen kept for each side: past them, the last run grows.
const MOST_SHOWN_RUNS: usize = 8;

/// What text a side is of: its blob, or the reading for a work tree file git did not hash.
#[derive(Clone, Debug)]
pub(super) enum Source {
    Blob(Id),
    Reading(Arc<Reading>),
}

impl PartialEq for Source {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Source::Blob(one), Source::Blob(other)) => one == other,
            (Source::Reading(one), Source::Reading(other)) => Arc::ptr_eq(one, other),
            _ => false,
        }
    }
}

/// Old side 0, new side 1.
pub(in crate::review::view) fn index(side: Side) -> usize {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

/// What text `reading`'s `side` is of; none for a reading that is not text. A new side is
/// known by its reading alone: one read from the work tree may differ from the blob git named,
/// as the file may change between the listing and the read.
pub(super) fn source(reading: &Arc<Reading>, side: Side) -> Option<Source> {
    if !matches!(reading.content, Content::Text { .. }) {
        return None;
    }
    let id = &reading.entry.old_id;
    Some(if side == Side::New || id.is_zero() {
        Source::Reading(Arc::clone(reading))
    } else {
        Source::Blob(id.clone())
    })
}

/// The numbers of the lines of `side` that `hunks` show, and the unchanged lines `context`
/// around them, rising.
pub(super) fn lines_of(hunks: &[Hunk], context: &[Range<u32>], side: Side) -> Vec<u32> {
    let changed = hunks.iter().flat_map(move |hunk| {
        let (start, lines) = match side {
            Side::Old => (hunk.old_start, hunk.removed.len()),
            Side::New => (hunk.new_start, hunk.added.len()),
        };
        let lines = u32::try_from(lines).unwrap_or(u32::MAX);
        (0..lines).map(move |offset| start.saturating_add(offset))
    });
    let mut lines: Vec<u32> = changed.chain(context.iter().cloned().flatten()).collect();
    lines.sort_unstable();
    lines.dedup();
    lines
}

/// Whether the review shows any line of `side`, as [`lines_of`] says, without listing them.
pub(super) fn has_lines(hunks: &[Hunk], context: &[Range<u32>], side: Side) -> bool {
    context.iter().any(|range| !range.is_empty())
        || hunks.iter().any(|hunk| match side {
            Side::Old => !hunk.removed.is_empty(),
            Side::New => !hunk.added.is_empty(),
        })
}

impl super::Highlighting {
    /// The file at `path` shows the unchanged lines `context` of each side, old then new,
    /// around its hunks (step 13): a side whose runs are whole but lack one is opened again.
    pub(in crate::review::view) fn context(&mut self, path: &[u8], context: [Vec<Range<u32>>; 2]) {
        let Some(file) = self.files.get_mut(path) else {
            return;
        };
        if file.context == context {
            return;
        }
        file.context = context;
        self.settled = false;
        let Some(Content::Text { shape, .. }) = file.reading.as_deref().map(|it| &it.content)
        else {
            return;
        };
        for side in [Side::Old, Side::New] {
            let state = &mut file.sides[index(side)];
            if state.complete {
                let lines = lines_of(shape.hunks(), &file.context[index(side)], side);
                state.complete = (state.projected.as_ref())
                    .is_some_and(|projected| projected.holds_all(lines.into_iter()));
            }
        }
    }
}

/// The lines of each side on screen, old then new, as runs of numbers, first and last.
pub(in crate::review::view) type SideRuns = [Vec<(u32, u32)>; 2];

/// Adds line `number` to `runs`, lines seen in the order rows show them: it extends the last run
/// when it follows it, else starts one, at most [`MOST_SHOWN_RUNS`].
pub(in crate::review::view) fn add_shown(runs: &mut Vec<(u32, u32)>, number: u32) {
    let full = runs.len() >= MOST_SHOWN_RUNS;
    match runs.last_mut() {
        Some((first, last)) if number.saturating_sub(1) <= *last && number >= *first => {
            *last = (*last).max(number);
        }
        Some((first, last)) if full => {
            *first = (*first).min(number);
            *last = (*last).max(number);
        }
        _ => runs.push((number, number)),
    }
}

/// Whether line `number` is in one of `runs`.
pub(super) fn on_screen(runs: &[(u32, u32)], number: u32) -> bool {
    runs.iter()
        .any(|(first, last)| (*first..=*last).contains(&number))
}

/// The byte ranges of `text` that `runs` of line numbers cover.
pub(super) fn shown_ranges(text: &Rope, runs: &[(u32, u32)]) -> Vec<Range<usize>> {
    runs.iter()
        .map(|(first, last)| {
            let first = (*first as usize).saturating_sub(1);
            let last = (*last as usize).saturating_sub(1);
            text.line_start(first)..text.line_end(last)
        })
        .collect()
}

/// The language of `reading`'s `side`, by the path it was read from: the old path for the old
/// side of a rename.
pub(super) fn language(reading: &Reading, side: Side) -> Option<Language> {
    let entry = &reading.entry;
    let named = match side {
        Side::Old => entry.old_path.as_deref().unwrap_or(&entry.path),
        Side::New => &entry.path,
    };
    let name = os_path(named).ok()?;
    Language::of(Mode::for_path(Path::new(name)))
}

/// Whether `bytes`, a side, starts with a BOM, which its text leaves out.
pub(super) fn has_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(BOM)
}

/// Whether `text`, a buffer's, is `bytes`, a side, as the loader decodes it: a BOM left out.
/// O(1) when the buffer is shorter, or more than three times longer, as after most edits; else
/// O(n), and a decoded copy only when the buffer is longer than the side's bytes, as a side with
/// invalid bytes is once decoded.
pub(super) fn same_text(bytes: &[u8], text: &Rope) -> bool {
    let rest = bytes.strip_prefix(BOM).unwrap_or(bytes);
    // Decoding only grows a text, each invalid byte to a U+FFFD of three: the same length
    // means the same bytes, as the buffer's are UTF-8, and a shorter one cannot be it.
    match text.len().cmp(&rest.len()) {
        Ordering::Less => false,
        Ordering::Equal => same_bytes(rest, text),
        Ordering::Greater if text.len() > rest.len().saturating_mul(3) => false,
        Ordering::Greater => {
            let (decoded, _) = decoded(bytes);
            same_bytes(decoded.as_bytes(), text)
        }
    }
}

/// Whether `text` holds `bytes`, chunk by chunk.
fn same_bytes(bytes: &[u8], text: &Rope) -> bool {
    if bytes.len() != text.len() {
        return false;
    }
    let mut at = 0;
    text.chunks().all(|chunk| {
        let chunk = chunk.as_bytes();
        let same = bytes.get(at..at + chunk.len()) == Some(chunk);
        at += chunk.len();
        same
    })
}

/// `bytes` decoded as the loader decodes a file, and whether it started with a BOM, which the
/// text leaves out. O(n).
pub(super) fn decode(bytes: &[u8]) -> (Rope, bool) {
    let (text, bom) = decoded(bytes);
    (Rope::from(text.as_str()), bom)
}

/// `bytes` decoded as [`decode`] decodes them, as a string.
fn decoded(bytes: &[u8]) -> (String, bool) {
    let (mut rest, bom) = match bytes.strip_prefix(BOM) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    let mut text = String::with_capacity(rest.len());
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                let invalid = error.error_len().unwrap_or(rest.len() - valid);
                // The bytes before `valid` are UTF-8, as `from_utf8` said.
                text.push_str(std::str::from_utf8(&rest[..valid]).unwrap_or_default());
                text.extend(std::iter::repeat_n(char::REPLACEMENT_CHARACTER, invalid));
                rest = &rest[valid + invalid..];
            }
        }
    }
    (text, bom)
}

#[cfg(test)]
mod tests {
    use oso_core::display_map::TabWidth;
    use oso_core::load::decode_all;

    use super::*;

    #[test]
    fn a_side_decodes_as_the_loader_decodes_a_file() {
        let texts: [&[u8]; 6] = [
            b"fn a() {}\n",
            b"\xEF\xBB\xBFfn\r\nb\n",
            b"a\xFFb\xE2\x82\n\xF0\x9F",
            b"\xC3",
            b"",
            "é漢字\u{1F600}\n".as_bytes(),
        ];
        for bytes in texts {
            let (text, bom) = decode(bytes);
            let loaded = decode_all(bytes, TabWidth::default());
            assert_eq!(text.to_string(), loaded.text().to_string(), "{bytes:?}");
            assert_eq!(bom, bytes.starts_with(BOM));
        }
    }

    #[test]
    fn a_buffers_text_is_a_side_when_it_decodes_the_same() {
        let rope = |text: &str| Rope::from(text);
        assert!(same_text(b"fn a() {}\n", &rope("fn a() {}\n")));
        assert!(
            same_text(b"\xEF\xBB\xBFfn\n", &rope("fn\n")),
            "the BOM left out"
        );
        assert!(!same_text(b"fn b() {}\n", &rope("fn a() {}\n")));
        assert!(!same_text(b"fn\n", &rope("fn\n\n")));
        assert!(same_text(b"a\xFFb", &rope("a\u{FFFD}b")), "an invalid byte");
        let long = "x".repeat(3000) + "\n";
        let mut edited = long.clone();
        edited.replace_range(2500..2501, "y");
        assert!(same_text(long.as_bytes(), &rope(&long)));
        assert!(!same_text(long.as_bytes(), &rope(&edited)));
    }

    #[test]
    fn lines_on_screen_make_runs_and_their_ranges() {
        let mut runs = Vec::new();
        for number in [3, 4, 5, 40, 41, 4] {
            add_shown(&mut runs, number);
        }
        assert_eq!(runs, [(3, 5), (40, 41), (4, 4)]);
        assert!(on_screen(&runs, 41) && !on_screen(&runs, 6));
        let text = Rope::from("a\nbb\nccc\n");
        assert_eq!(
            shown_ranges(&text, &[(2, 3)]),
            std::slice::from_ref(&(2..8))
        );
        let mut many = Vec::new();
        for number in (0..20).map(|n| n * 10) {
            add_shown(&mut many, number);
        }
        assert_eq!(many.len(), MOST_SHOWN_RUNS);
        assert_eq!(many.last(), Some(&(70, 190)));
    }
}
