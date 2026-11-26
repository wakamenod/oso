//! The review layout's own state (ADR 0184): read as it is left and put back on the layout made
//! again, the slot's buffer, line, rows above, width and the side selected the same; nothing
//! that may hold secrets kept or shown again; a file gone or a side no longer there falls back
//! to what the layout shows first; a line past the end and a share past the frame held in.
//!
//! Must not depend on: the app's threads, git or files; these work on an editor made here.

use std::collections::HashSet;
use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer, BufferId, Replacement};
use oso_core::editor::{Editor, ReviewId};
use oso_core::review::Side;

use super::super::laid::{
    SlotKinds, Source, lay_out_kept, layout_state, put_at, slot_shows, source_fits,
};
use super::super::windows::fit_slot_to;
use super::super::{enter, restore};
use super::{columns, editor, windows};
use crate::review::saved::{SavedLayout, SlotShows};
use crate::sessions::spaces::kept::Kept;

const SOURCE: &str = "/home/me/src/oso/c.rs";

/// A file buffer at `path`, loaded and judged as a load judges it, of `lines` lines.
fn file(editor: &mut Editor, path: &str, lines: usize) -> BufferId {
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.finish_empty();
    buffer.judge_secret(Some(&[PathBuf::from(path)]));
    buffer.set_baseline(Baseline {
        location: PathBuf::from(path),
        file: None,
    });
    let id = editor.add_buffer(buffer);
    let text: String = (0..lines)
        .map(|i| format!("fn line_{i}() {{}}\n"))
        .collect();
    let edit = Replacement {
        range: 0..0,
        text: text.as_str(),
    };
    editor.apply_edits(id, &[edit]).unwrap();
    id
}

/// The kinds of a review with no side and no message, and nothing held.
fn plain<'a>(held: &'a HashSet<PathBuf>) -> SlotKinds<'a> {
    SlotKinds {
        side: &|_| None,
        message: None,
        held,
    }
}

/// The layout's state now, with nothing held.
fn state(editor: &Editor, kept: &[Kept], review: ReviewId) -> Option<SavedLayout> {
    layout_state(editor, kept, review, &plain(&HashSet::new()))
}

/// The layout of `review` made again, as a continue makes it, with `layout` put back, the
/// slot given what `find` finds for what it showed.
fn again(
    editor: &mut Editor,
    kept: &mut Vec<Kept>,
    review: ReviewId,
    layout: &SavedLayout,
    find: Option<BufferId>,
) {
    assert!(enter(editor, kept, review));
    assert!(lay_out_kept(editor, kept, review, layout, |_, _| find));
}

#[test]
fn a_layout_left_comes_back_with_its_slot_s_line_rows_above_width_and_selection() {
    let (mut editor, review, _, _) = editor();
    let source = file(&mut editor, SOURCE, 200);
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    // The person showed a file in the slot, moved in it, widened it and stayed in it.
    editor.show_in(slot, source);
    put_at(&mut editor, slot, (120, 3), 10);
    fit_slot_to(&mut editor, slot, Some(300));
    editor.select_window(slot);
    let left_as = state(&editor, &kept, review).unwrap();
    assert_eq!(
        left_as,
        SavedLayout {
            slot_selected: true,
            slot_permille: 300,
            ask_permille: None,
            slot: SlotShows::Source {
                path: SOURCE.into(),
                line: 120,
                column: 3,
                above: 10,
            },
        }
    );
    // `q`: the layout goes, its state kept; continued, the layout comes back as it was.
    restore(&mut editor, &mut kept).unwrap();
    assert_eq!(state(&editor, &kept, review), None, "not laid out");
    again(&mut editor, &mut kept, review, &left_as, Some(source));
    assert_eq!(state(&editor, &kept, review).unwrap(), left_as);
    // The review selected, the slot back at the right, and the window start where it was.
    restore(&mut editor, &mut kept).unwrap();
    let review_selected = SavedLayout {
        slot_selected: false,
        ..left_as.clone()
    };
    again(
        &mut editor,
        &mut kept,
        review,
        &review_selected,
        Some(source),
    );
    let shown = windows(&editor);
    assert_eq!(editor.window().id(), shown[0].0);
    assert_ne!(shown[0].0, left, "a layout made again");
    let slot = shown[1].0;
    let window = editor.window_by_id(slot).unwrap();
    let text = editor.buffer(source).unwrap().text();
    assert_eq!(text.offset_to_point(window.scroll().start).row, 109);
    assert_eq!(columns(&editor, slot), 36, "three tenths of 120 columns");
}

#[test]
fn nothing_that_may_hold_secrets_is_kept_or_shown_again() {
    let (mut editor, review, _, _) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let slot = windows(&editor)[1].0;
    let none = HashSet::new();
    // A buffer that may hold secrets is kept as nothing.
    let env = file(&mut editor, "/home/me/src/oso/.env", 3);
    assert!(editor.buffer(env).unwrap().is_secret());
    editor.show_in(slot, env);
    assert_eq!(slot_shows(&editor, slot, &plain(&none)), SlotShows::Nothing);
    // A file held as secret while a review was open, though its buffer is not now.
    let source = file(&mut editor, SOURCE, 3);
    editor.show_in(slot, source);
    let held: HashSet<PathBuf> = [PathBuf::from(SOURCE)].into();
    assert_eq!(slot_shows(&editor, slot, &plain(&held)), SlotShows::Nothing);
    assert!(matches!(
        slot_shows(&editor, slot, &plain(&none)),
        SlotShows::Source { .. }
    ));
    // A side is kept by its path and side, a message as the message.
    let side = |_| Some((b"src/a.rs".as_slice().into(), Side::Old));
    let kinds = SlotKinds {
        side: &side,
        message: None,
        held: &none,
    };
    assert!(matches!(
        slot_shows(&editor, slot, &kinds),
        SlotShows::Side {
            side: Side::Old,
            line: 1,
            ..
        }
    ));
    let kinds = SlotKinds {
        side: &|_| None,
        message: Some(source),
        held: &none,
    };
    assert_eq!(slot_shows(&editor, slot, &kinds), SlotShows::Message);
    // Put back, the name is judged again: a file renamed to a secret name since, a file held
    // since, or one whose buffer may hold secrets now, is not shown.
    assert_eq!(
        source_fits(&editor, &none, SOURCE.as_ref()),
        Source::Open(source)
    );
    let renamed = std::path::Path::new("/home/me/src/oso/id_rsa");
    assert_eq!(source_fits(&editor, &none, renamed), Source::Withheld);
    assert_eq!(
        source_fits(&editor, &held, SOURCE.as_ref()),
        Source::Withheld
    );
    let env = std::path::Path::new("/home/me/src/oso/.env");
    assert_eq!(source_fits(&editor, &none, env), Source::Withheld);
    let closed = std::path::Path::new("/home/me/src/oso/d.rs");
    assert_eq!(source_fits(&editor, &none, closed), Source::Closed);
}

#[test]
fn a_file_gone_or_a_side_no_longer_there_leaves_what_the_layout_shows_first() {
    let (mut editor, review, file_shown, _) = editor();
    let mut kept = Vec::new();
    let gone = SavedLayout {
        slot_selected: true,
        slot_permille: 500,
        ask_permille: None,
        slot: SlotShows::Source {
            path: "/home/me/src/oso/gone.rs".into(),
            line: 4,
            column: 0,
            above: 1,
        },
    };
    again(&mut editor, &mut kept, review, &gone, None);
    let shown = windows(&editor);
    assert_eq!(shown[1].1, file_shown, "what the layout shows first");
    assert_eq!(
        editor.window().id(),
        shown[1].0,
        "the side selected is still put back"
    );
    assert_eq!(columns(&editor, shown[1].0), 60);
    restore(&mut editor, &mut kept).unwrap();
    let side = SavedLayout {
        slot: SlotShows::Side {
            path: b"src/renamed.rs".as_slice().into(),
            side: Side::New,
            line: 3,
            above: 0,
        },
        ..gone
    };
    again(&mut editor, &mut kept, review, &side, None);
    assert_eq!(windows(&editor)[1].1, file_shown);
}

#[test]
fn a_line_past_the_end_and_a_share_past_the_frame_are_held_in() {
    let (mut editor, review, _, _) = editor();
    let source = file(&mut editor, SOURCE, 20);
    let mut kept = Vec::new();
    let past = SavedLayout {
        slot_selected: false,
        slot_permille: 1000,
        ask_permille: Some(1000),
        slot: SlotShows::Source {
            path: SOURCE.into(),
            line: 10_000,
            column: 999,
            above: 5_000,
        },
    };
    again(&mut editor, &mut kept, review, &past, Some(source));
    let [(left, ..), (slot, shows, cursor)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    assert_eq!(shows, source);
    let text = editor.buffer(source).unwrap().text();
    // The last line, which is empty, at its start.
    assert_eq!(cursor, text.len());
    assert_eq!(columns(&editor, slot), 108, "nine tenths at most");
    assert!(columns(&editor, left) >= 12, "the review keeps a tenth");
    // A file shorter now: its last line, as far as it goes.
    let shorter = SavedLayout {
        slot: SlotShows::Source {
            path: SOURCE.into(),
            line: 20,
            column: 999,
            above: 0,
        },
        ..past
    };
    restore(&mut editor, &mut kept).unwrap();
    again(&mut editor, &mut kept, review, &shorter, Some(source));
    let cursor = windows(&editor)[1].2;
    let text = editor.buffer(source).unwrap().text();
    assert_eq!(cursor, text.line_end(19));
}
