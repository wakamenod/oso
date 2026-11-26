//! The session below the review layout's slot, kept by its record name (ADR 0184, amending ADR
//! 0169): a live hosted one shown below the slot again with the split it had, an ended one
//! only named, nothing started; a session whose buffer may hold secrets never named; a layout a
//! restart carried given back only what its slot lacks; and a saved slot's file put back only
//! under the review's root.
//!
//! Must not depend on: the app's threads, git or files; these work on an editor made here, and
//! a table of hosted sessions with no process behind them.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::frame::Way;

use super::super::below::{SessionBack, session_back, session_below};
use super::super::laid::{
    SlotKinds, lay_out_kept, layout_state, put_at, put_back_carried_slot, under_root,
};
use super::super::{carried, enter, restore, show_asked};
use super::{editor, rows, terminal, windows};
use crate::review::saved::{SavedLayout, SlotShows};
use crate::sessions::hosted::{New, Run, Table};
use crate::sessions::identity::{Origin, SessionId};
use crate::sessions::spaces::kept::{self, Kept};

/// A table of one hosted session named `oso-2`, running in `buffer`, with no process.
fn hosting(buffer: BufferId) -> Table {
    let mut table = Table::default();
    let (key, _) = table.reserve(Path::new("/home/me/src/oso"), "oso-2");
    let new = New {
        native: false,
        id: SessionId::from_random([7; 16]),
        root: None,
        parent: None,
        origin: Origin::Person,
        unasked_brief: None,
        changed: 1,
    };
    let session = table.add(key, new).unwrap();
    session.buffer = Some(buffer);
    session.run = Run::Running { child: None };
    assert_eq!(session.name, "oso-2");
    table
}

/// The layout's state now, `name` the record name of the session `session` shows.
fn state_with(
    editor: &Editor,
    kept: &[Kept],
    review: oso_core::editor::ReviewId,
    session: BufferId,
) -> Option<SavedLayout> {
    let none = HashSet::new();
    let named = move |buffer| (buffer == session).then(|| "oso-2".to_owned());
    let kinds = SlotKinds {
        side: &|_| None,
        message: None,
        held: &none,
        session: &named,
    };
    layout_state(editor, kept, review, &kinds)
}

#[test]
fn a_live_hosted_session_goes_below_the_slot_again_with_its_split() {
    let (mut editor, review, _, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let slot = windows(&editor)[1].0;
    editor.show_in(slot, other);
    put_at(&mut editor, slot, (40, 0), 5);
    // A session's terminal, which agents never read: still named by its record name.
    let session = editor.add_terminal_buffer("claude: oso-2");
    assert!(editor.buffer(session).unwrap().withheld_from_agents());
    assert!(show_asked(&mut editor, &mut kept, session));
    let left_as = state_with(&editor, &kept, review, session).unwrap();
    assert_eq!(left_as.session.as_deref(), Some("oso-2"));
    let below_rows = rows(&editor, windows(&editor)[2].0);
    // `q`, then continued: the layout is made again, the slot first, then the session below.
    restore(&mut editor, &mut kept).unwrap();
    assert!(enter(&mut editor, &mut kept, review));
    assert!(lay_out_kept(
        &mut editor,
        &mut kept,
        review,
        &left_as,
        None,
        |_, _| Some(other)
    ));
    let table = hosting(session);
    let back = session_back(&editor, table.iter(), "oso-2");
    assert_eq!(back, Some(SessionBack::Show(session)));
    assert!(session_below(
        &mut editor,
        &mut kept,
        session,
        left_as.ask_permille
    ));
    let shown = windows(&editor);
    let [(left, ..), (top, top_shows, _), (below, below_shows, _)] = shown[..] else {
        panic!("the review, the source and the session: {shown:?}");
    };
    assert_eq!((top_shows, below_shows), (other, session));
    assert_eq!(editor.window().id(), left, "the review stays selected");
    assert!(rows(&editor, below).abs_diff(below_rows) <= 1, "the split");
    let laid = kept::review(&kept).unwrap();
    assert_eq!((laid.ask, laid.asked), (Some(below), Some(session)));
    assert_eq!(laid.slot, Some(top));
    assert_eq!(
        state_with(&editor, &kept, review, session).unwrap(),
        left_as
    );
}

#[test]
fn an_ended_session_is_only_named_and_nothing_starts() {
    let (mut editor, review, _, _) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let session = terminal(&mut editor, "*claude: oso-2*");
    let mut table = hosting(session);
    let key = table.iter().next().unwrap().key;
    let before = windows(&editor);
    for run in [Run::Stopped, Run::Failed] {
        table.get_mut(key).unwrap().run = run;
        assert_eq!(
            session_back(&editor, table.iter(), "oso-2"),
            Some(SessionBack::Ended("oso-2".to_owned()))
        );
    }
    // One no longer hosted at all is ended too.
    assert_eq!(
        session_back(&editor, table.iter(), "oso-9"),
        Some(SessionBack::Ended("oso-9".to_owned()))
    );
    // Still starting: nothing yet, and nothing said.
    table.get_mut(key).unwrap().run = Run::Starting;
    assert_eq!(session_back(&editor, table.iter(), "oso-2"), None);
    // Nothing was started, and the frame is as it was: the table holds what it held.
    assert_eq!(table.iter().count(), 1);
    assert_eq!(table.get(key).unwrap().run, Run::Starting);
    assert_eq!(windows(&editor), before);
}

#[test]
fn a_session_whose_terminal_may_hold_secrets_is_never_named_nor_shown() {
    let (mut editor, review, _, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let slot = windows(&editor)[1].0;
    editor.show_in(slot, other);
    let session = editor.add_terminal_buffer("claude: oso-2");
    assert!(show_asked(&mut editor, &mut kept, session));
    // Its program reads a password: the buffer may hold secrets.
    editor.set_echo_off(session, true);
    assert!(editor.buffer(session).unwrap().is_secret());
    let left_as = state_with(&editor, &kept, review, session).unwrap();
    assert_eq!(left_as.session, None, "never named");
    // Live, but its buffer may hold secrets: nothing shown, and nothing said.
    let table = hosting(session);
    assert_eq!(session_back(&editor, table.iter(), "oso-2"), None);
}

/// The windows a restart carried: the review in `left`, the slot showing `file`, standing for
/// what it showed, and a session below it; the review selected.
fn carried_windows(
    editor: &mut Editor,
    review: oso_core::editor::ReviewId,
    file: BufferId,
) -> (
    Vec<Kept>,
    oso_core::window::WindowId,
    oso_core::window::WindowId,
    BufferId,
) {
    let buffer = editor.review_buffer(review).unwrap();
    let left = editor.tiled().0.id();
    editor.show_in(left, buffer);
    let slot = editor.split_window(Way::Across, false).unwrap();
    editor.show_in(slot, file);
    editor.select_window(slot);
    let below = editor.split_window(Way::Down, false).unwrap();
    let session = terminal(editor, "*claude: oso-2*");
    editor.show_in(below, session);
    editor.select_window(left);
    let mut kept = Vec::new();
    carried(
        editor,
        &mut kept,
        review,
        left,
        slot,
        Some(below),
        Some(session),
    );
    (kept, left, slot, session)
}

#[test]
fn a_layout_a_restart_carried_gets_back_only_what_its_slot_lacks() {
    let (mut editor, review, file, other) = editor();
    let (mut kept, left, slot, session) = carried_windows(&mut editor, review, file);
    let sizes: Vec<_> = (windows(&editor).iter())
        .map(|(id, ..)| (*id, rows(&editor, *id)))
        .collect();
    let saved = SavedLayout {
        slot_selected: true,
        slot_permille: 250,
        ask_permille: Some(800),
        slot: SlotShows::Source {
            path: "/home/me/src/oso/b.rs".into(),
            line: 60,
            column: 2,
            above: 4,
        },
        session: Some("oso-2".to_owned()),
    };
    let none = HashSet::new();
    let kinds = SlotKinds {
        side: &|_| None,
        message: None,
        held: &none,
        session: &|_| None,
    };
    // The slot lacks the file it showed: it goes in, at its line and rows above; the widths,
    // the window below and the selection stay as carried.
    let shows_other = |_: &mut Editor, _: &SlotShows| Some(other);
    assert!(put_back_carried_slot(
        &mut editor,
        &mut kept,
        (review, &kinds),
        &saved,
        shows_other
    ));
    let shown = windows(&editor);
    assert_eq!(shown[1].1, other);
    assert_eq!(shown[2].1, session);
    assert_eq!(editor.window().id(), left, "the selection carried");
    let now: Vec<_> = (shown.iter())
        .map(|(id, ..)| (*id, rows(&editor, *id)))
        .collect();
    assert_eq!(now, sizes, "the sizes carried");
    let window = editor.window_by_id(slot).unwrap();
    let text = editor.buffer(other).unwrap().text();
    assert_eq!(
        text.offset_to_point(window.selections.primary().cursor).row,
        59
    );
    assert_eq!(text.offset_to_point(window.scroll().start).row, 55);
    // Already showing that file, as a carried file buffer does: left as it is.
    editor.show_in(slot, file);
    put_at(&mut editor, slot, (10, 0), 0);
    let was = windows(&editor);
    let mut asked = false;
    let ask = |_: &mut Editor, _: &SlotShows| {
        asked = true;
        Some(other)
    };
    let kept_as = SavedLayout {
        slot: SlotShows::Source {
            path: "/home/me/src/oso/a.rs".into(),
            line: 60,
            column: 2,
            above: 4,
        },
        ..saved
    };
    assert!(!put_back_carried_slot(
        &mut editor,
        &mut kept,
        (review, &kinds),
        &kept_as,
        ask
    ));
    assert!(!asked, "nothing looked up");
    assert_eq!(windows(&editor), was, "the slot as carried");
    assert_eq!(windows(&editor)[1].1, file);
}

#[test]
fn a_saved_slot_file_goes_back_only_under_the_review_s_root() {
    let root = Path::new("/home/me/src/oso");
    let under = |path: &str| under_root(root, &PathBuf::from(path));
    assert!(under("/home/me/src/oso/src/a.rs"));
    assert!(under("/home/me/src/oso/a.rs"));
    assert!(
        !under("/home/me/src/oso"),
        "the root itself is no file of it"
    );
    assert!(!under("/home/me/src/other/a.rs"));
    assert!(
        !under("/home/me/src/oso-2/a.rs"),
        "a sibling sharing its start"
    );
    assert!(!under("/home/me/src/oso/../other/a.rs"));
    assert!(!under("/home/me/src/oso/src/../../x"));
    assert!(!under("/etc/passwd"));
    assert!(!under("src/a.rs"), "a relative path");
}
