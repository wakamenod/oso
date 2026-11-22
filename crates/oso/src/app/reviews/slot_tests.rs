//! The review layout (ADR 0160): entered and restored exactly, the slot each jump replaces,
//! found again after the person rearranged the windows, the key's choices, a zoom inside it,
//! a layout a restart carried, and a property that whatever is done inside it, restoring gives
//! back the tree from before. The session `review-ask` shows below what the slot shows (ADR
//! 0169) is tested in `ask`.
//!
//! Must not depend on: the app's threads, git or files; these work on an editor made here.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, ReviewId};
use oso_core::frame::{FrameSize, Tiling, Way};
use oso_core::selection::SelectionSet;
use oso_core::window::WindowId;
use proptest::prelude::*;

use super::{
    Key, back, carried, enter, in_slot, jump_slot, press, restore, show_asked, show_in_slot,
    show_laid,
};
use crate::sessions::spaces::kept::{self, Kept};

/// Where a jump's source goes, as `review_jumped` chooses: the slot, else step 3's rule.
fn jump_window(editor: &mut Editor, kept: &mut [Kept], review: ReviewId) -> Result<(), String> {
    if jump_slot(editor, kept) {
        return Ok(());
    }
    super::super::visit::select_source_window(editor, review)
}

/// An editor of a project's file, 120 columns wide, with a second file and a review open.
fn editor() -> (Editor, ReviewId, BufferId, BufferId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 120,
    });
    let file = editor.tiled().0.buffer();
    // Judged as a load judges it: a file not judged yet may hold secrets.
    let path = std::path::PathBuf::from("/home/me/src/oso/a.rs");
    (editor.buffer_mut(file).unwrap()).judge_secret(Some(std::slice::from_ref(&path)));
    let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let other = editor.add_buffer(Buffer::from_text("b.rs", &text));
    let (review, _) = editor.add_review(project, "*review: uncommitted oso*".into());
    (editor, review, file, other)
}

/// Every tiled window of the tree in its order, with what it shows and its primary cursor.
fn windows(editor: &Editor) -> Vec<(WindowId, BufferId, usize)> {
    let side = editor.side_window();
    (editor.frame().windows().iter())
        .filter(|(id, _)| Some(*id) != side)
        .map(|(id, _)| {
            let window = editor.window_by_id(*id).unwrap();
            (*id, window.buffer(), window.selections.primary().cursor)
        })
        .collect()
}

/// What the frame looks like: its windows, their tree and which one is selected.
fn look(editor: &Editor) -> (Vec<(WindowId, BufferId, usize)>, Tiling, WindowId) {
    (windows(editor), editor.tiling(), editor.window().id())
}

/// The columns window `id` takes, its divider's too.
fn columns(editor: &Editor, id: WindowId) -> usize {
    let (_, regions) = (editor.frame().windows().iter())
        .find(|(window, _)| *window == id)
        .unwrap();
    regions.modeline.columns + regions.divider.map_or(0, |divider| divider.columns)
}

/// The rows window `id` takes, its modeline's too.
fn rows(editor: &Editor, id: WindowId) -> usize {
    let (_, regions) = (editor.frame().windows().iter())
        .find(|(window, _)| *window == id)
        .unwrap();
    regions.text.rows + regions.modeline.rows
}

/// A buffer standing for a session's terminal.
fn terminal(editor: &mut Editor, name: &str) -> BufferId {
    editor.add_buffer(Buffer::from_text(name, "> "))
}

/// Two windows side by side, the second showing `other` at its line 30, the first selected.
fn two_windows(editor: &mut Editor, other: BufferId) {
    let second = editor.split_window(Way::Across, false).unwrap();
    editor.show_in(second, other);
    let at = editor.buffer(other).unwrap().text().line_start(30);
    editor.restore_positions_in(
        second,
        SelectionSet::new(at),
        editor.window_by_id(second).unwrap().scroll(),
    );
}

#[test]
fn the_layout_shows_the_review_beside_the_slot_and_restores_the_windows_exactly() {
    let (mut editor, review, file, other) = editor();
    two_windows(&mut editor, other);
    let before = look(&editor);
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let shown = windows(&editor);
    assert_eq!(shown.len(), 2, "the review and the slot");
    assert_eq!(Some(shown[0].1), editor.review_buffer(review));
    assert_eq!(shown[1].1, file, "the slot shows what the window showed");
    assert_eq!(editor.window().id(), shown[0].0, "the review is selected");
    assert_eq!(
        columns(&editor, shown[1].0),
        48,
        "two fifths of 120 columns"
    );
    assert!(kept::review_in_frame(&kept));
    let old = restore(&mut editor, &mut kept).unwrap();
    assert_eq!(old.len(), 1, "the layout's tree is given back to be freed");
    assert_eq!(look(&editor), before);
    assert!(kept.is_empty());
}

#[test]
fn each_jump_replaces_the_slot_s_buffer_and_the_way_back_lands_where_it_was_asked() {
    let (mut editor, review, _, other) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    assert!(in_slot(&editor, &kept));
    editor.show(other);
    assert_eq!(back(&mut editor, &mut kept).unwrap().map(|_| ()), None);
    assert_eq!(editor.window().id(), left);
    // A side, then the message: still two windows, the slot taking each.
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    editor.select_window(left);
    let message = editor.add_buffer(Buffer::from_text("*review message*", "x"));
    assert!(show_in_slot(&mut editor, &mut kept, message, true));
    assert_eq!(editor.window().id(), slot);
    assert_eq!(windows(&editor).len(), 2);
    assert_eq!(windows(&editor)[1].1, message);
    back(&mut editor, &mut kept).unwrap();
    assert_eq!(editor.window().id(), left);
}

#[test]
fn a_slot_deleted_or_moved_by_hand_is_found_or_made_again_at_the_review_s_right() {
    let (mut editor, review, _, _) = editor();
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    // Split by hand: the slot keeps its name, and the jump goes there.
    editor.select_window(slot);
    let below = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(left);
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    // Deleted: made again at the review's right, two fifths wide.
    editor.delete_window().unwrap();
    editor.select_window(below);
    editor.delete_window().unwrap();
    assert_eq!(windows(&editor).len(), 1);
    jump_window(&mut editor, &mut kept, review).unwrap();
    let shown = windows(&editor);
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0].0, left);
    assert_eq!(editor.window().id(), shown[1].0);
    assert_eq!(columns(&editor, shown[1].0), 48);
    // Outside the layout, a jump goes as before: another window, else one split below.
    let mut none = Vec::new();
    editor.select_window(left);
    jump_window(&mut editor, &mut none, review).unwrap();
    assert_eq!(editor.window().id(), shown[1].0);
}

#[test]
fn the_key_opens_the_menu_enters_goes_back_and_restores() {
    let (mut editor, review, _, other) = editor();
    let mut kept = Vec::new();
    assert_eq!(press(&mut editor, &mut kept, true).0, Key::Menu, "C-u");
    let before = look(&editor);
    assert_eq!(press(&mut editor, &mut kept, false).0, Key::Entered);
    let left = editor.window().id();
    jump_window(&mut editor, &mut kept, review).unwrap();
    editor.show(other);
    assert_eq!(press(&mut editor, &mut kept, false).0, Key::Back);
    assert_eq!(editor.window().id(), left);
    assert_eq!(
        press(&mut editor, &mut kept, true).0,
        Key::Menu,
        "C-u in the layout"
    );
    assert_eq!(press(&mut editor, &mut kept, false).0, Key::Restored);
    assert_eq!(look(&editor), before);
    // The layout's review closed while it could not restore: the key restores.
    assert_eq!(press(&mut editor, &mut kept, false).0, Key::Entered);
    let project = editor.reviews().next().unwrap().1;
    editor.add_review(project, "*review: staged oso*".into());
    editor.remove_review(review);
    assert_eq!(press(&mut editor, &mut kept, false).0, Key::Restored);
    assert_eq!(look(&editor).1, before.1);
    let (other, _) = editor.reviews().next().unwrap();
    editor.remove_review(other);
    assert_eq!(
        press(&mut editor, &mut kept, false).0,
        Key::Menu,
        "no review open"
    );
}

#[test]
fn opening_another_review_in_the_layout_shows_it_at_the_left_even_from_the_slot() {
    let (mut editor, review, _, _) = editor();
    let project = editor.reviews().next().unwrap().1;
    let (second, buffer) = editor.add_review(project, "*review: staged oso*".into());
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let [(left, ..), (slot, ..)] = windows(&editor)[..] else {
        panic!("two windows");
    };
    editor.select_window(slot);
    assert!(show_laid(&mut editor, &mut kept, second).0);
    assert_eq!(editor.window().id(), left);
    assert_eq!(editor.window().buffer(), buffer);
    assert_eq!(kept::review(&kept).unwrap().review, second);
    assert_eq!(
        windows(&editor).len(),
        2,
        "the tree from before is kept, not taken again"
    );
    assert!(matches!(&kept[..], [Kept::Review(_)]));
}

#[test]
fn a_zoom_inside_the_layout_nests_and_the_restore_drops_it() {
    let (mut editor, review, _, other) = editor();
    two_windows(&mut editor, other);
    let before = look(&editor);
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    let left = editor.window().id();
    jump_window(&mut editor, &mut kept, review).unwrap();
    kept::zoom(&mut editor, &mut kept).unwrap();
    assert!(!kept::review_in_frame(&kept));
    // From the zoomed slot, the key zooms out and goes back to the review, giving the zoomed
    // tree back to be freed off the UI thread.
    let (key, old) = press(&mut editor, &mut kept, false);
    assert_eq!((key, old.len()), (Key::Back, 1));
    assert_eq!(editor.window().id(), left);
    assert!(kept::review_in_frame(&kept));
    // Zoomed on the review, the key restores the windows from before, the zoom dropped.
    // The layout's tree, kept by the zoom, and the zoomed tree both go to be freed.
    kept::zoom(&mut editor, &mut kept).unwrap();
    let (key, old) = press(&mut editor, &mut kept, false);
    assert_eq!((key, old.len()), (Key::Restored, 2));
    assert_eq!(look(&editor), before);
    assert!(kept.is_empty());
}

#[test]
fn a_layout_a_restart_carried_restores_to_the_review_s_window_burying_the_review() {
    let (mut editor, review, file, _) = editor();
    let buffer = editor.review_buffer(review).unwrap();
    let left = editor.tiled().0.id();
    editor.show_in(left, buffer);
    let slot = editor.split_window(Way::Across, false).unwrap();
    editor.show_in(slot, file);
    let mut kept = Vec::new();
    carried(&editor, &mut kept, review, left, slot, None, None);
    assert!(kept::review_in_frame(&kept));
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_eq!(editor.window().id(), slot);
    editor.select_window(left);
    assert!(restore(&mut editor, &mut kept).unwrap().is_empty());
    let shown = windows(&editor);
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].0, left);
    assert_eq!(shown[0].1, file, "what it showed behind the review");
    // A second carried layout is not taken over the first.
    let mut held = Vec::new();
    carried(&editor, &mut held, review, left, slot, None, None);
    assert!(held.is_empty(), "the slot is gone");
}

/// What the person may do with the windows inside the layout.
#[derive(Clone, Debug)]
enum Act {
    Split(Way),
    Delete,
    Other,
    Show(bool),
    Jump,
    Back,
    Resize(i8),
    Zoom,
    Ask,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        Just(Act::Split(Way::Down)),
        Just(Act::Split(Way::Across)),
        Just(Act::Delete),
        Just(Act::Other),
        any::<bool>().prop_map(Act::Show),
        Just(Act::Jump),
        Just(Act::Back),
        (-8i8..8).prop_map(Act::Resize),
        Just(Act::Zoom),
        Just(Act::Ask),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn whatever_is_done_inside_the_layout_restoring_gives_back_the_tree_from_before(
        split in any::<bool>(),
        acts in prop::collection::vec(act(), 0..24),
    ) {
        let (mut editor, review, file, other) = editor();
        if split {
            two_windows(&mut editor, other);
        }
        let before = look(&editor);
        let mut kept = Vec::new();
        prop_assert!(enter(&mut editor, &mut kept, review));
        let buffer = editor.review_buffer(review).unwrap();
        let session = terminal(&mut editor, "*claude: repo*");
        for act in acts {
            match act {
                Act::Split(way) => {
                    let _ = editor.split_window(way, false);
                }
                Act::Delete => {
                    let _ = editor.delete_window();
                }
                Act::Other => editor.other_window(1),
                Act::Show(review_buffer) => {
                    editor.show(if review_buffer { buffer } else { file });
                }
                Act::Jump => {
                    if kept::review_in_frame(&kept) {
                        let _ = jump_window(&mut editor, &mut kept, review);
                    }
                }
                Act::Back => {
                    let _ = back(&mut editor, &mut kept);
                }
                Act::Resize(cells) => {
                    let _ = editor.resize_window(Way::Across, isize::from(cells));
                }
                Act::Zoom => {
                    let _ = kept::zoom(&mut editor, &mut kept);
                }
                Act::Ask => {
                    show_asked(&mut editor, &mut kept, session);
                }
            }
        }
        restore(&mut editor, &mut kept).unwrap();
        prop_assert_eq!(look(&editor), before);
        prop_assert!(kept.is_empty());
    }
}

#[test]
fn the_slot_never_brings_back_a_buffer_that_may_hold_secrets() {
    let (mut editor, review, file, _) = editor();
    let env = editor.add_buffer(Buffer::visiting(std::path::PathBuf::from("/home/me/.env")));
    assert!(editor.buffer(env).unwrap().is_secret());
    // The window showed a.rs, then `.env`, then the review: `.env` is the last it showed.
    editor.show(env);
    editor.show(editor.review_buffer(review).unwrap());
    let mut kept = Vec::new();
    assert!(enter(&mut editor, &mut kept, review));
    assert_eq!(windows(&editor)[1].1, file, "a.rs, not .env");
    // A slot made again by a jump passes over it too.
    let slot = windows(&editor)[1].0;
    editor.select_window(slot);
    editor.delete_window().unwrap();
    let left = windows(&editor)[0].0;
    editor.select_window(left);
    jump_window(&mut editor, &mut kept, review).unwrap();
    assert_ne!(editor.window().buffer(), env);
}

#[path = "slot_ask_tests.rs"]
mod ask;
