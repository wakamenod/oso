//! The session `review-ask` shows below what the review layout's slot shows (ADR 0169): the
//! split, a jump that changes the top alone, the window below reused, made again or made from
//! when the person deleted one, too short to split, carried across a restart, and its terminal's
//! size.
//!
//! Must not depend on: the app's threads, git or files; these work on an editor made here.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::window::WindowId;

use super::super::{
    ask_takes_size, back, carried, enter, in_slot, restore, show_asked, show_in_slot,
};
use super::{columns, editor, jump_window, rows, terminal, windows};
use crate::sessions::spaces::kept;

#[test]
fn a_question_goes_below_the_source_and_a_jump_then_changes_only_the_top() {
    let (mut editor, review, file, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    jump_window(&mut editor, &mut kept, review).unwrap();
    editor.show(other);
    back(&mut editor, &mut kept).unwrap();
    let session = terminal(&mut editor, "*claude: repo*");
    assert!(show_asked(&mut editor, &mut kept, session));
    let shown = windows(&editor);
    let [(_, _, _), (top, top_shows, _), (below, below_shows, _)] = shown[..] else {
        panic!("the review, the source and the session: {shown:?}");
    };
    assert_eq!((top, top_shows), (slot, other), "the source stays on top");
    assert_eq!(below_shows, session);
    assert_eq!(editor.window().id(), left, "the review stays selected");
    assert_eq!(
        columns(&editor, below),
        48,
        "the slot's column, two fifths wide"
    );
    assert!(
        rows(&editor, top).abs_diff(rows(&editor, below)) <= 1,
        "halves"
    );
    // A jump changes the top alone.
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    editor.show(file);
    let shown = windows(&editor);
    assert_eq!(shown.len(), 3);
    assert_eq!((shown[1].1, shown[2].1), (file, session));
    // A later question reuses the window below, whichever session it shows.
    back(&mut editor, &mut kept).unwrap();
    let again = terminal(&mut editor, "*claude: other*");
    assert!(show_asked(&mut editor, &mut kept, again));
    let shown = windows(&editor);
    assert_eq!(shown.len(), 3);
    assert_eq!((shown[1].1, shown[2]), (file, (below, again, 0)));
    assert_eq!(editor.window().id(), left);
    // The message of comments takes the top too.
    let message = editor.add_buffer(Buffer::from_text("*review message*", "x"));
    assert!(show_in_slot(&mut editor, &mut kept, message, true));
    assert_eq!(editor.window().id(), slot);
    assert_eq!(
        (windows(&editor)[1].1, windows(&editor)[2].1),
        (message, again)
    );
}

#[test]
fn a_question_takes_the_whole_slot_over_messages_or_itself_and_a_jump_then_splits_it() {
    let (mut editor, review, _, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    let messages = editor.messages_buffer();
    assert!(show_in_slot(&mut editor, &mut kept, messages, false));
    let session = terminal(&mut editor, "*claude: repo*");
    assert!(show_asked(&mut editor, &mut kept, session));
    assert_eq!(windows(&editor).len(), 2, "*Messages* is the slot's filler");
    assert_eq!(windows(&editor)[1].1, session);
    assert!(show_asked(&mut editor, &mut kept, session));
    assert_eq!(
        windows(&editor).len(),
        2,
        "the slot shows the session already"
    );
    // A jump while the session fills the slot: the source on top, the session below.
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    editor.show(other);
    let shown = windows(&editor);
    assert_eq!(shown.len(), 3);
    assert_eq!((shown[1].0, shown[1].1, shown[2].1), (slot, other, session));
    back(&mut editor, &mut kept).unwrap();
    assert_eq!(editor.window().id(), left);
}

#[test]
fn a_window_of_the_split_slot_deleted_by_hand_is_made_again_below_or_above() {
    let (mut editor, review, file, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let left = editor.window().id();
    let session = terminal(&mut editor, "*claude: repo*");
    assert!(show_asked(&mut editor, &mut kept, session));
    assert_eq!(windows(&editor).len(), 3, "below a.rs");
    // The window below deleted: the next question splits again.
    let below = windows(&editor)[2].0;
    editor.select_window(below);
    editor.delete_window().unwrap();
    editor.select_window(left);
    assert!(show_asked(&mut editor, &mut kept, session));
    let shown = windows(&editor);
    assert_eq!((shown.len(), shown[1].1, shown[2].1), (3, file, session));
    // The top deleted: the next jump splits the window below, the session staying below.
    editor.select_window(shown[1].0);
    editor.delete_window().unwrap();
    editor.select_window(left);
    jump_window(&mut editor, &mut kept, review).unwrap();
    editor.show(other);
    let again = windows(&editor);
    assert_eq!(again.len(), 3);
    assert_eq!(
        (again[1].0, again[1].1, again[2].1),
        (shown[2].0, other, session)
    );
    assert!(in_slot(&editor, &kept));
    assert!(rows(&editor, again[1].0).abs_diff(rows(&editor, again[2].0)) <= 1);
}

#[test]
fn a_slot_too_short_to_split_shows_the_question_in_place() {
    let (mut editor, review, _, _) = editor();
    editor.set_frame_size(FrameSize {
        rows: 5,
        columns: 120,
    });
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let session = terminal(&mut editor, "*claude: repo*");
    assert!(show_asked(&mut editor, &mut kept, session));
    let shown = windows(&editor);
    assert_eq!((shown.len(), shown[1].1), (2, session));
    // Outside the layout, nothing is shown.
    restore(&mut editor, &mut kept).unwrap();
    assert!(!show_asked(&mut editor, &mut kept, session));
}

#[test]
fn a_restart_carries_the_window_below_the_slot_which_the_next_question_reuses() {
    let (mut editor, review, file, _) = editor();
    let buffer = editor.review_buffer(review).unwrap();
    let left = editor.tiled().0.id();
    editor.show_in(left, buffer);
    let slot = editor.split_window(Way::Across, false).unwrap();
    editor.show_in(slot, file);
    editor.select_window(slot);
    let below = editor.split_window(Way::Down, false).unwrap();
    let session = terminal(&mut editor, "*claude: repo*");
    editor.show_in(below, session);
    editor.select_window(left);
    let mut kept = Vec::new();
    carried(
        &editor,
        &mut kept,
        review,
        left,
        slot,
        Some(below),
        Some(session),
    );
    let laid = kept::review(&kept).unwrap();
    assert_eq!((laid.ask, laid.asked), (Some(below), Some(session)));
    let again = terminal(&mut editor, "*claude: other*");
    assert!(show_asked(&mut editor, &mut kept, again));
    let shown = windows(&editor);
    assert_eq!((shown.len(), shown[2].0, shown[2].1), (3, below, again));
    // A window below that is the slot, or gone, is not carried.
    let mut held = Vec::new();
    carried(&editor, &mut held, review, left, slot, Some(slot), None);
    let laid = kept::review(&held).unwrap();
    assert_eq!((laid.ask, laid.asked), (None, None));
}

#[test]
fn the_window_below_takes_the_terminal_s_size_while_no_window_showing_it_owns_it() {
    let (mut editor, review, file, _) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let left = editor.window().id();
    let session = terminal(&mut editor, "*claude: repo*");
    let none = |_| None;
    assert_eq!(
        ask_takes_size(&editor, &kept, none),
        None,
        "no window below"
    );
    assert!(show_asked(&mut editor, &mut kept, session));
    let below = windows(&editor)[2].0;
    // Whether the terminal's owner shows it, as the app judges it, said for the session alone.
    let terminal_of = |editor: &Editor, owner: Option<WindowId>| {
        let owned = (owner.and_then(|window| editor.window_by_id(window)))
            .is_some_and(|window| window.buffer() == session);
        move |buffer: BufferId| (buffer == session).then_some(owned)
    };
    // Owned by no window, or by one gone or showing something else: the window below takes it.
    let taken = Some((session, below));
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, None)),
        taken
    );
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, Some(left))),
        taken
    );
    // Owned by a window showing it: kept, as ADR 0120 has it.
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, Some(below))),
        None
    );
    let other = editor.split_window(Way::Down, false).unwrap();
    editor.show_in(other, session);
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, Some(other))),
        None
    );
    // A buffer below that is no terminal has no size to take.
    editor.show_in(below, file);
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, None)),
        None
    );
    // Outside the layout, nothing.
    editor.show_in(below, session);
    restore(&mut editor, &mut kept).unwrap();
    assert_eq!(
        ask_takes_size(&editor, &kept, terminal_of(&editor, None)),
        None
    );
}

#[test]
fn a_session_filling_a_carried_slot_goes_below_on_the_next_jump() {
    let (mut editor, review, _, other) = editor();
    let buffer = editor.review_buffer(review).unwrap();
    let left = editor.tiled().0.id();
    editor.show_in(left, buffer);
    let slot = editor.split_window(Way::Across, false).unwrap();
    let session = terminal(&mut editor, "*claude: repo*");
    editor.show_in(slot, session);
    let mut kept = Vec::new();
    carried(&editor, &mut kept, review, left, slot, None, Some(session));
    jump_window(&mut editor, &mut kept, review).unwrap();
    editor.show(other);
    let shown = windows(&editor);
    assert_eq!((shown.len(), shown[1].1, shown[2].1), (3, other, session));
}
