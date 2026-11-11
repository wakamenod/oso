//! Tests of a buffer's highlights through edits, undo, loads and secrets (ADR 0111).
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::path::PathBuf;

use crate::buffer::{Buffer, Replacement};
use crate::display_map::TabWidth;
use crate::load::Builder;
use crate::rope::Rope;
use crate::syntax::{Highlight, Highlights, Splice};
use crate::undo::{Direction, Opening};

const S: Option<Highlight> = Some(Highlight::String);

fn edit(buffer: &mut Buffer, range: std::ops::Range<usize>, text: &str) {
    let staged = buffer
        .stage_edits(&[Replacement { range, text }])
        .unwrap()
        .unwrap();
    buffer.commit_edit(staged, Opening::New(None), None);
}

fn undo(buffer: &mut Buffer) {
    let staged = buffer.stage_crossing(Direction::Undo).unwrap();
    buffer.commit_crossing(staged);
}

/// `len` bytes, with a string over `string`.
fn string(len: usize, string: std::ops::Range<usize>) -> Highlights {
    let mut highlights = Highlights::new(len);
    highlights.replace(string.clone(), [(string.len(), S)]);
    highlights
}

fn runs(buffer: &Buffer) -> Vec<(std::ops::Range<usize>, Option<Highlight>)> {
    let highlights = buffer.highlights().unwrap();
    highlights.runs(0..highlights.len()).collect()
}

/// Applies `batch` to `text`, taking each splice's new text from `after`, as a parser sees it.
fn replay(text: &Rope, batch: &[Splice], after: &Rope) -> String {
    let mut out = String::new();
    let (mut old_at, mut new_at) = (0, 0);
    for splice in batch {
        let gap = splice.start - old_at;
        out.push_str(&text.slice(old_at..splice.start).to_string());
        let new_start = new_at + gap;
        out.push_str(
            &after
                .slice(new_start..new_start + splice.new_len)
                .to_string(),
        );
        old_at = splice.start + splice.old_len;
        new_at = new_start + splice.new_len;
    }
    out.push_str(&text.slice(old_at..text.len()).to_string());
    out
}

#[test]
fn a_report_for_an_older_revision_follows_the_edits_since() {
    let mut buffer = Buffer::from_text("t", "let s = \"ab\";");
    let reported = buffer.revision();
    assert!(buffer.highlights().is_none());
    // Typed inside the string, and at the start, before the report comes.
    edit(&mut buffer, 10..10, "x");
    edit(&mut buffer, 0..0, "  ");
    assert!(buffer.take_highlights(reported, string(13, 8..12)));
    assert_eq!(runs(&buffer), [(0..10, None), (10..15, S), (15..16, None)]);
    // An edit after the report maps what is shown.
    edit(&mut buffer, 12..12, "y");
    assert_eq!(runs(&buffer), [(0..10, None), (10..16, S), (16..17, None)]);
}

#[test]
fn a_report_that_does_not_cover_the_text_is_dropped() {
    let mut buffer = Buffer::from_text("t", "abc");
    let now = buffer.revision();
    assert!(!buffer.take_highlights(now, Highlights::new(2)));
    assert!(!buffer.take_highlights(now + 1, Highlights::new(3)));
    assert!(buffer.highlights().is_none());
    assert!(buffer.take_highlights(now, Highlights::new(3)));
}

#[test]
fn the_log_reaches_back_64_revisions() {
    let mut buffer = Buffer::from_text("t", "");
    let start = buffer.revision();
    for _ in 0..64 {
        let end = buffer.text().len();
        edit(&mut buffer, end..end, "a");
    }
    assert_eq!(
        buffer.splices_since(start),
        Some(vec![Splice {
            start: 0,
            old_len: 0,
            new_len: 64
        }])
    );
    edit(&mut buffer, 0..0, "b");
    assert_eq!(buffer.splices_since(start), None);
    assert!(!buffer.take_highlights(start, Highlights::new(0)));
    assert_eq!(buffer.splices_since(start + 1).map(|b| b.len()), Some(2));
    assert_eq!(buffer.splices_since(buffer.revision()), Some(Vec::new()));
}

#[test]
fn splices_since_a_revision_take_its_text_to_the_text_now() {
    let mut buffer = Buffer::from_text("t", "fn main() {}\n// note\n");
    let start = buffer.revision();
    let before = buffer.text().clone();
    edit(&mut buffer, 3..7, "start");
    edit(&mut buffer, 0..2, "pub fn");
    edit(&mut buffer, 20..22, "");
    undo(&mut buffer);
    edit(&mut buffer, 8..8, "日本");
    let batch = buffer.splices_since(start).unwrap();
    assert_eq!(
        replay(&before, &batch, buffer.text()),
        buffer.text().to_string()
    );
}

#[test]
fn undo_and_a_load_map_highlights_too() {
    let mut buffer = Buffer::from_text("t", "\"abc\"");
    assert!(buffer.take_highlights(buffer.revision(), string(5, 0..5)));
    edit(&mut buffer, 2..4, "");
    assert_eq!(runs(&buffer), [(0..3, S)]);
    undo(&mut buffer);
    assert_eq!(runs(&buffer), [(0..5, S)]);
    let mut builder = Builder::new(None, TabWidth::default());
    builder.push(b"new text");
    builder.finish();
    buffer.apply_load(builder.snapshot());
    assert_eq!(runs(&buffer), [(0..8, None)]);
}

#[test]
fn a_buffer_that_may_hold_secrets_shows_and_takes_none() {
    let mut buffer = Buffer::from_text("t", "abc");
    assert!(buffer.take_highlights(buffer.revision(), string(3, 0..3)));
    assert!(buffer.treat_as_secret());
    assert!(buffer.highlights().is_none());
    assert!(!buffer.take_highlights(buffer.revision(), string(3, 0..3)));
    let visiting = Buffer::visiting(PathBuf::from("/tmp/notes.md"));
    assert!(visiting.is_secret(), "unjudged");
}
