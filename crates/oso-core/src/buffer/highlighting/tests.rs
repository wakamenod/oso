//! Tests of a buffer's highlights through edits, undo, loads and secrets (ADR 0111).
//!
//! Must not depend on: `window`, `editor` or `command`.

use std::path::PathBuf;

use crate::buffer::{Buffer, Mode, Replacement};
use crate::display_map::TabWidth;
use crate::load::Builder;
use crate::rope::Rope;
use crate::secret::Verdict;
use crate::syntax::{Highlight, Highlights, MAX_BYTES, Splice};
use crate::undo::{Direction, Opening};

const S: Option<Highlight> = Some(Highlight::String);

fn edit(buffer: &mut Buffer, range: std::ops::Range<usize>, text: &str) {
    let staged = buffer
        .stage_edits(&[Replacement { range, text }])
        .unwrap()
        .unwrap();
    buffer.commit_edit(staged, Opening::New(None), None);
}

fn cross(buffer: &mut Buffer, direction: Direction) {
    let staged = buffer.stage_crossing(direction).unwrap();
    buffer.commit_crossing(staged);
}

fn undo(buffer: &mut Buffer) {
    cross(buffer, Direction::Undo);
}

/// A buffer of Rust source holding `text`, which can be highlighted.
fn rust(text: &str) -> Buffer {
    let mut buffer = Buffer::from_text("t.rs", text);
    buffer.set_mode(Mode::Rust);
    buffer
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
    let mut buffer = rust("let s = \"ab\";");
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
    let mut buffer = rust("abc");
    let now = buffer.revision();
    assert!(!buffer.take_highlights(now, Highlights::new(2)));
    assert!(!buffer.take_highlights(now + 1, Highlights::new(3)));
    assert!(buffer.highlights().is_none());
    assert!(buffer.take_highlights(now, Highlights::new(3)));
}

#[test]
fn the_log_reaches_back_64_revisions() {
    let mut buffer = rust("");
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
    let mut buffer = rust("fn main() {}\n// note\n");
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
fn undo_and_redo_map_highlights_and_a_load_drops_them() {
    let mut buffer = rust("\"abc\"");
    assert!(buffer.take_highlights(buffer.revision(), string(5, 0..5)));
    edit(&mut buffer, 2..4, "");
    assert_eq!(runs(&buffer), [(0..3, S)]);
    undo(&mut buffer);
    assert_eq!(runs(&buffer), [(0..5, S)]);
    cross(&mut buffer, Direction::Redo);
    assert_eq!(runs(&buffer), [(0..3, S)]);
    let before = buffer.revision();
    let mut builder = Builder::new(None, TabWidth::default());
    builder.push(b"new text");
    builder.finish();
    buffer.apply_load(builder.snapshot());
    assert!(
        buffer.highlights().is_none(),
        "a report from before a load is dropped"
    );
    assert_eq!(buffer.splices_since(before), None);
    assert!(!buffer.take_highlights(before, Highlights::new(3)));
}

#[test]
fn buffers_that_cannot_be_highlighted_keep_no_log() {
    let mut fundamental = Buffer::from_text("t", "abc");
    let start = fundamental.revision();
    edit(&mut fundamental, 0..0, "x");
    assert_eq!(fundamental.splices_since(start), None);
    assert!(!fundamental.take_highlights(fundamental.revision(), Highlights::new(4)));

    let mut internal = Buffer::internal("*m*");
    internal.set_mode(Mode::Rust);
    let start = internal.revision();
    let staged = internal
        .stage_internal(&[Replacement {
            range: 0..0,
            text: "x",
        }])
        .unwrap();
    internal.commit_internal(staged);
    assert_eq!(internal.splices_since(start), None);

    let mut large = rust(&"x".repeat(MAX_BYTES));
    let start = large.revision();
    assert_eq!(large.splices_since(start), Some(Vec::new()));
    edit(&mut large, 0..0, "y");
    assert_eq!(large.splices_since(start), None, "past the size limit");
}

#[test]
fn the_log_holds_at_most_4096_splices() {
    let mut buffer = rust(&"ab".repeat(5000));
    let start = buffer.revision();
    let edits: Vec<_> = (0..5000)
        .map(|index| Replacement {
            range: index * 2..index * 2 + 1,
            text: "c",
        })
        .collect();
    assert!(buffer.take_highlights(start, string(10_000, 1..3)));
    let staged = buffer.stage_edits(&edits).unwrap().unwrap();
    assert!(staged.highlighting.batch.is_none(), "composed past the log");
    buffer.commit_edit(staged, Opening::New(None), None);
    assert_eq!(buffer.splices_since(start), None);
    assert_eq!(
        runs(&buffer)[1],
        (1..3, S),
        "the highlights follow a batch too large to log"
    );
    let middle = buffer.revision();
    edit(&mut buffer, 0..0, "z");
    assert_eq!(buffer.splices_since(middle).map(|b| b.len()), Some(1));
}

#[test]
fn a_new_mode_forgets_what_the_old_one_had() {
    let mut buffer = rust("abc");
    let start = buffer.revision();
    assert!(buffer.take_highlights(start, string(3, 0..3)));
    edit(&mut buffer, 0..0, "x");
    buffer.set_mode(Mode::Toml);
    assert!(!buffer.keeps_highlighting());
    assert_eq!(buffer.splices_since(start), None);
}

#[test]
fn a_buffer_that_may_hold_secrets_shows_and_takes_none() {
    let mut buffer = rust("abc");
    let start = buffer.revision();
    assert!(buffer.take_highlights(start, string(3, 0..3)));
    edit(&mut buffer, 0..0, "x");
    assert!(buffer.treat_as_secret());
    assert!(!buffer.keeps_highlighting());
    assert_eq!(
        buffer.splices_since(start),
        None,
        "no log of a secret buffer"
    );
    assert!(!buffer.take_highlights(buffer.revision(), string(4, 0..4)));
    edit(&mut buffer, 0..0, "y");
    assert!(!buffer.keeps_highlighting(), "nor of its edits");
}

#[test]
fn a_judgement_that_withholds_drops_highlights_and_the_log() {
    let mut buffer = Buffer::visiting(PathBuf::from("/p/credentials.toml"));
    buffer.set_mode(Mode::Toml);
    assert!(buffer.is_secret(), "unjudged");
    assert!(!buffer.take_highlights(buffer.revision(), Highlights::new(0)));
    buffer.judge_secret(Some(&[PathBuf::from("/p/credentials.toml")]));
    assert!(buffer.is_secret());
    assert!(buffer.highlights().is_none());
    let mut ordinary = Buffer::visiting(PathBuf::from("/p/Cargo.toml"));
    ordinary.set_mode(Mode::Toml);
    ordinary.judge_secret(Some(&[PathBuf::from("/p/Cargo.toml")]));
    assert!(!ordinary.is_secret());
}

/// A visited Rust file judged ordinary, with highlights and a log of one edit since `start`.
fn judged_with_highlighting() -> (Buffer, u64) {
    let path = PathBuf::from("/p/main.rs");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.set_mode(Mode::Rust);
    buffer.finish_empty();
    buffer.judge_secret(Some(&[path]));
    assert!(!buffer.is_secret());
    edit(&mut buffer, 0..0, "abc");
    let start = buffer.revision();
    assert!(buffer.take_highlights(start, string(3, 0..3)));
    edit(&mut buffer, 0..0, "x");
    assert!(buffer.highlights().is_some());
    assert!(buffer.splices_since(start).is_some());
    (buffer, start)
}

#[track_caller]
fn assert_dropped(buffer: &Buffer, start: u64) {
    assert!(buffer.is_secret());
    assert!(!buffer.keeps_highlighting(), "highlights or a log kept");
    assert_eq!(buffer.splices_since(start), None);
}

#[test]
fn a_path_judged_unresolved_drops_highlights_and_the_log() {
    let (mut buffer, start) = judged_with_highlighting();
    buffer.judge_unresolved(&[]);
    assert_dropped(&buffer, start);
}

#[test]
fn a_file_judged_again_drops_highlights_and_the_log() {
    let (mut buffer, start) = judged_with_highlighting();
    buffer.judge_again(Ok(&[PathBuf::from("/p/.env")]));
    assert_dropped(&buffer, start);
}

#[test]
fn a_copy_judged_secret_drops_highlights_and_the_log() {
    let (mut buffer, start) = judged_with_highlighting();
    buffer.judge_copy(&[PathBuf::from("/p/.env")], Verdict::Ordinary);
    assert_dropped(&buffer, start);
}

#[test]
fn a_recovery_and_its_redo_drop_highlights_and_the_log() {
    let (mut buffer, start) = judged_with_highlighting();
    let copy = Buffer::from_text("copy", "recovered");
    let staged = buffer.stage_replacement(&copy).unwrap();
    buffer.commit_recovery(staged, Opening::New(None), None, Verdict::SecretName);
    assert_dropped(&buffer, start);
    // Back past the recovery and the edit before it, the buffer is ordinary and logs again.
    undo(&mut buffer);
    assert!(!buffer.is_secret());
    undo(&mut buffer);
    let before = buffer.revision();
    assert!(buffer.take_highlights(before, string(3, 0..3)));
    cross(&mut buffer, Direction::Redo);
    assert!(buffer.highlights().is_some());
    assert!(buffer.splices_since(before).is_some());
    // Redoing the recovery crosses back to its verdict.
    cross(&mut buffer, Direction::Redo);
    assert_dropped(&buffer, before);
}
