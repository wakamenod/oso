//! Docked windows on the editor (ADR 0170): where they go, how they are selected, and what
//! takes them away.
//!
//! Must not depend on: the host's window.

use crate::buffer::{Buffer, BufferId, Mode, Replacement};
use crate::editor::Editor;
use crate::frame::{FrameSize, Way};
use crate::window::{DockSlot, WindowId};

/// An editor showing a host buffer in a frame of `rows` by 80, with a composer docked under it.
fn docked(rows: usize, composer: &str) -> (Editor, BufferId, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\ntwo\n"));
    editor.set_frame_size(FrameSize { rows, columns: 80 });
    let host = editor.window().buffer();
    let input = editor.add_buffer(Buffer::from_text("*compose: s*", composer));
    editor.set_dock(host, DockSlot::Composer, Some(input));
    (editor, host, input)
}

fn rows(editor: &Editor, id: WindowId) -> usize {
    editor.regions(id).map_or(0, |regions| regions.text.rows)
}

#[test]
fn a_composer_takes_its_rows_under_the_host_and_grows_with_its_text() {
    let (mut editor, host, input) = docked(30, "");
    editor.set_wraps(input, true);
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let carrier = editor.window().id();
    assert_eq!(rows(&editor, composer), 1);
    let host_rows = rows(&editor, carrier);
    // Its window wraps and grows, up to its most.
    editor.select_window(composer);
    assert_eq!(
        editor.current().0.buffer(),
        input,
        "keys go to the composer"
    );
    let long = "word ".repeat(200);
    let typed = Replacement {
        range: 0..0,
        text: &long,
    };
    editor.apply_edits(input, &[typed]).unwrap();
    assert_eq!(rows(&editor, composer), super::MAX_COMPOSER);
    assert_eq!(rows(&editor, carrier), host_rows + 1 - super::MAX_COMPOSER);
    // Truncated, its one line is one row; wrapped again, as many as before.
    editor.set_wraps(input, false);
    assert_eq!(rows(&editor, composer), 1);
    editor.set_wraps(input, true);
    assert_eq!(rows(&editor, composer), super::MAX_COMPOSER);
}

#[test]
fn c_x_o_steps_through_the_docks_and_the_trees_commands_act_on_the_host() {
    let (mut editor, host, _) = docked(30, "");
    let carrier = editor.window().id();
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    editor.other_window(1);
    assert_eq!(editor.window().id(), composer);
    // A split from the composer splits its host.
    let split = editor.split_window(Way::Down, false).unwrap();
    assert!(editor.splits.contains(split));
    assert!(!editor.splits.contains(composer));
    assert_eq!(editor.window().id(), carrier);
    // The host was selected last: it keeps the docks.
    assert!(rows(&editor, composer) > 0);
    editor.select_window(composer);
    editor.other_window(1);
    assert_ne!(editor.window().id(), composer);
}

#[test]
fn a_host_not_shown_hides_its_docks_and_a_short_one_says_so() {
    let (mut editor, host, _) = docked(30, "");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let other = editor.add_buffer(Buffer::from_text("notes", "n\n"));
    editor.show(other);
    assert_eq!(rows(&editor, composer), 0, "hidden, but kept");
    assert_eq!(editor.dock_window(host, DockSlot::Composer), Some(composer));
    editor.show(host);
    assert_eq!(rows(&editor, composer), 1, "back as it was");
    editor.set_frame_size(FrameSize {
        rows: 4,
        columns: 80,
    });
    assert_eq!(rows(&editor, composer), 0);
    assert!(editor.docks_hidden(editor.window().id()));
    let mut status = crate::editor::Status::default();
    editor.status_into(editor.window().id(), &mut status);
    assert!(
        status.warnings.contains("[docked hidden]"),
        "{}",
        status.warnings
    );
}

#[test]
fn a_dock_goes_with_its_host_and_its_own_buffer() {
    let (mut editor, host, input) = docked(30, "");
    editor.set_dock(host, DockSlot::Composer, None);
    assert_eq!(editor.dock_window(host, DockSlot::Composer), None);
    editor.set_dock(host, DockSlot::Composer, Some(input));
    editor.kill_buffer(input);
    assert_eq!(editor.dock_window(host, DockSlot::Composer), None);
    let again = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    editor.set_dock(host, DockSlot::Composer, Some(again));
    editor.kill_buffer(host);
    assert_eq!(editor.dock_window(host, DockSlot::Composer), None);
}

/// A slot that shows another buffer shows it in the same window, docked there still, and shows the
/// one before again where it was.
#[test]
fn another_buffer_in_a_slot_takes_the_same_window() {
    let (mut editor, host, input) = docked(30, "one\ntwo\nthree");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    editor.select_window(composer);
    editor.current_mut().0.selections.move_primary(6);
    let answer = editor.add_buffer(Buffer::from_text("answer", ""));
    for (buffer, at, its_rows) in [(answer, 0, 1), (input, 6, 3)] {
        editor.set_dock(host, DockSlot::Composer, Some(buffer));
        assert_eq!(editor.dock_window(host, DockSlot::Composer), Some(composer));
        let window = editor.window();
        assert_eq!((window.id(), window.buffer()), (composer, buffer));
        assert_eq!(window.docked(), Some((host, DockSlot::Composer)));
        assert_eq!(window.selections.primary().cursor, at);
        assert_eq!(rows(&editor, composer), its_rows);
        assert_eq!(editor.windows().count(), 2);
    }
}

#[test]
fn the_strip_and_the_card_stack_above_the_composer() {
    let (mut editor, host, _) = docked(30, "");
    let card = editor.add_buffer(Buffer::from_text("*card*", "a\nb\nc"));
    let strip = editor.add_buffer(Buffer::from_text("*strip*", "todo"));
    editor.set_dock(host, DockSlot::Strip, Some(strip));
    editor.set_dock(host, DockSlot::Card, Some(card));
    let at = |slot| {
        let id = editor.dock_window(host, slot).unwrap();
        editor.regions(id).unwrap().text
    };
    let (card, strip, composer) = (
        at(DockSlot::Card),
        at(DockSlot::Strip),
        at(DockSlot::Composer),
    );
    assert_eq!((card.rows, strip.rows, composer.rows), (3, 1, 1));
    assert!(strip.row < card.row && card.row < composer.row);
}

#[test]
fn a_requests_line_shows_all_its_rows_or_hides_with_every_dock() {
    let (mut editor, host, _) = docked(20, "a\nb\nc\nd\ne\nf");
    let line = editor.add_internal_buffer("*request: s*", Mode::Request);
    editor.set_internal_text(line, "y n N\nRET i\nBash");
    editor.set_dock(host, DockSlot::Card, Some(line));
    let window = editor.dock_window(host, DockSlot::Card).unwrap();
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    assert_eq!(rows(&editor, window), 3);
    for height in (4..20).rev() {
        editor.set_frame_size(FrameSize {
            rows: height,
            columns: 40,
        });
        // The composer gives its rows up first.
        let (line, composer) = (rows(&editor, window), rows(&editor, composer));
        assert!(
            line == 3 && composer >= 1 || line == 0 && composer == 0,
            "{height}: {line} {composer}"
        );
    }
    assert_eq!(rows(&editor, window), 0, "too short for it");
}

/// A card's rows are as wide as it is (ADR 0180): none counts as two.
#[test]
fn a_card_takes_a_row_a_line_however_wide() {
    let (mut editor, host, _) = docked(20, "");
    let card = editor.add_internal_buffer("*card: s*", Mode::Card);
    let wide = "─".repeat(400);
    editor.set_internal_text(card, &format!("{wide}\n{wide}\n{wide}"));
    editor.set_dock(host, DockSlot::Card, Some(card));
    let window = editor.dock_window(host, DockSlot::Card).unwrap();
    assert_eq!(rows(&editor, window), 3);
}

#[test]
fn showing_a_buffer_or_deleting_from_a_dock_acts_on_its_host() {
    let (mut editor, host, _) = docked(30, "");
    let carrier = editor.window().id();
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    editor.select_window(composer);
    let other = editor.add_buffer(Buffer::from_text("notes", "n\n"));
    editor.show(other);
    assert_eq!(editor.window().id(), carrier, "shown in the host");
    assert_eq!(editor.window().buffer(), other);
    assert_eq!(
        rows(&editor, composer),
        0,
        "the host no longer shows the session"
    );
    editor.show(host);
    let lower = editor.split_window(Way::Down, true).unwrap();
    editor.select_window(composer);
    // Each of the two shows the session; the one selected last carries the docks.
    assert!(rows(&editor, composer) > 0);
    editor.delete_window().unwrap();
    assert!(!editor.splits.contains(lower) || !editor.splits.contains(carrier));
    assert!(editor.splits.iter().count() == 1);
    assert!(
        rows(&editor, composer) > 0,
        "the docks go to the window left"
    );
}

#[test]
fn a_docked_frame_lays_out_nothing_more_until_something_changes() {
    let (mut editor, _, _) = docked(30, "");
    assert!(!editor.fit_gutter(), "a dock's gutter is its own");
    editor.set_visual_aids(crate::editor::VisualAids {
        line_numbers: true,
        ..crate::editor::VisualAids::default()
    });
    editor.fit_gutter();
    assert!(!editor.fit_gutter(), "with line numbers too");
}

#[test]
fn a_docks_buffer_changes_only_by_set_dock() {
    let (mut editor, host, input) = docked(30, "");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let other = editor.add_buffer(Buffer::from_text("review", "r\n"));
    editor.show_in(composer, other);
    let window = editor.window_by_id(composer).unwrap();
    assert_eq!(window.buffer(), input, "the host showed it instead");
    // An internal buffer taken out takes its dock with it.
    editor.show(host);
    let card = editor.add_internal_buffer("*card*", crate::buffer::Mode::Fundamental);
    editor.set_dock(host, DockSlot::Card, Some(card));
    drop(editor.remove_internal_buffer(card));
    assert_eq!(editor.dock_window(host, DockSlot::Card), None);
    // Nor is a host docked under itself, or a buffer twice.
    editor.set_dock(host, DockSlot::Strip, Some(host));
    editor.set_dock(host, DockSlot::Strip, Some(input));
    assert_eq!(editor.dock_window(host, DockSlot::Strip), None);
}

#[test]
fn c_x_0_in_the_dock_of_the_one_window_changes_nothing() {
    let (mut editor, host, _) = docked(30, "");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    editor.select_window(composer);
    assert!(editor.delete_window().is_err());
    assert_eq!(editor.window().id(), composer, "still in the composer");
}

#[test]
fn of_two_windows_on_the_session_the_one_selected_last_carries_the_docks() {
    let (mut editor, host, _) = docked(40, "");
    let first = editor.window().id();
    let second = editor.split_window(Way::Down, false).unwrap();
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let inside = |editor: &Editor, window| {
        let host = *editor.regions(window).unwrap();
        let docked = editor.regions(composer).unwrap().text;
        docked.row >= host.text.row + host.text.rows && docked.row < host.modeline.row
    };
    assert!(inside(&editor, first));
    editor.select_window(second);
    assert!(inside(&editor, second), "it went with the selection");
    // Up from the composer is its host; down from the host, the composer.
    editor.select_window(composer);
    assert_eq!(editor.tree_window(), second);
    editor.windmove(crate::frame::Side::Up).unwrap();
    assert_eq!(editor.window().id(), second);
    editor.windmove(crate::frame::Side::Down).unwrap();
    assert_eq!(editor.window().id(), composer);
}

#[test]
fn a_dock_on_a_buffer_that_may_hold_secrets_counts_as_shown() {
    let (mut editor, host, _) = docked(30, "");
    let secret = editor.add_buffer(crate::editor::test_buffers::secret_buffer());
    editor.set_dock(host, DockSlot::Card, Some(secret));
    assert!(editor.shows_secret());
}

#[test]
fn docks_stay_with_the_frame_when_a_space_s_tree_comes_in() {
    let (mut editor, host, _) = docked(30, "");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let other = editor.add_buffer(Buffer::from_text("notes", "n\n"));
    let tree = editor.window_tree_showing(&[other]);
    let saved = editor.swap_window_tree(tree).unwrap();
    assert_eq!(editor.dock_window(host, DockSlot::Composer), Some(composer));
    assert_eq!(rows(&editor, composer), 0, "no window shows the session");
    editor.swap_window_tree(saved).unwrap();
    assert_eq!(rows(&editor, composer), 1, "back under its session");
}

#[test]
fn a_host_s_outer_rect_keeps_its_docks_so_floats_and_splits_see_it_whole() {
    let (editor, _, _) = docked(30, "");
    let plain = {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
        editor.set_frame_size(FrameSize {
            rows: 30,
            columns: 80,
        });
        editor.frame().tiled()
    };
    assert_eq!(editor.frame().tiled(), plain);
    let host = editor.regions(editor.window().id()).unwrap().outer();
    assert_eq!(host.rows, 29, "text, docks and modeline");
}

#[test]
fn a_docks_buffer_is_never_listed_and_selecting_the_dock_raises_its_host() {
    let (mut editor, host, input) = docked(30, "");
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let other = editor.add_buffer(Buffer::from_text("notes", ""));
    // The session in one window, `notes` in the other, selected.
    editor.split_window(Way::Down, false).unwrap();
    editor.show(other);
    assert_eq!(editor.buffers_by_recency().first(), Some(&other));
    // To the session by its dock: the host rises, as the window it is part of.
    editor.select_window(composer);
    assert_eq!(editor.window().id(), composer);
    assert_eq!(editor.buffers_by_recency().first(), Some(&host));
    assert!(!editor.buffers_by_recency().contains(&input));
    assert_eq!(editor.buffer_named("*compose: s*"), None);
    // `C-x b` to its name is refused, never a second buffer of that name.
    assert!(editor.switch_to_named("*compose: s*").is_err());
}

/// A native session's composer, `*compose: s*`, `rows` by `columns`, with a strip of one row over it.
fn native(rows: usize, columns: usize) -> (Editor, BufferId, BufferId) {
    let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\ntwo\n"));
    editor.set_frame_size(FrameSize { rows, columns });
    let host = editor.window().buffer();
    let input = editor.add_internal_buffer("*compose: s*".to_owned(), Mode::Composer);
    editor.set_wraps(input, true);
    editor.set_dock(host, DockSlot::Composer, Some(input));
    let strip = editor.add_internal_buffer("*todo: s*".to_owned(), Mode::Todo);
    editor.set_internal_text(strip, "Tasks");
    editor.set_dock(host, DockSlot::Strip, Some(strip));
    editor.set_footer(host, Some(String::new()));
    (editor, host, input)
}

/// The rows of the host, the strip and the composer, whether the composer is padded, and whether the
/// host shows its footer.
fn heights(editor: &Editor, host: BufferId) -> (usize, usize, usize, bool, bool) {
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let strip = editor.dock_window(host, DockSlot::Strip).unwrap();
    let carrier = editor.window().id();
    (
        rows(editor, carrier),
        rows(editor, strip),
        rows(editor, composer),
        editor.frame().padding(composer).is_some(),
        editor.frame().footer(carrier).is_some(),
    )
}

/// A host of each number of rows, at 20 columns as at 80: the footer is given up first, then the
/// padding, then the strip, and the composer's text last.
#[test]
fn a_hosts_rows_go_by_their_order_at_any_width() {
    for columns in [20, 80] {
        let (mut editor, host, _) = native(30, columns);
        // The host's whole text area, less its modeline and the echo area.
        let mut at = |whole: usize| {
            editor.set_frame_size(FrameSize {
                rows: whole + 2,
                columns,
            });
            heights(&editor, host)
        };
        assert_eq!(at(28), (23, 1, 1, true, true));
        assert_eq!(at(7), (2, 1, 1, true, true));
        assert_eq!(at(6), (2, 1, 1, true, false));
        assert_eq!(at(5), (3, 1, 1, false, false));
        assert_eq!(at(4), (2, 1, 1, false, false));
        assert_eq!(at(3), (2, 0, 1, false, false));
        assert_eq!(at(2), (2, 0, 0, false, false));
        assert_eq!(at(1), (1, 0, 0, false, false));
        assert_eq!(
            at(28),
            (23, 1, 1, true, true),
            "a frame that grows gives them back"
        );
    }
}

/// A draft's rows come before what is around them, and its padding is none of its ten.
#[test]
fn a_tall_draft_takes_the_paddings_rows_only_where_there_are_no_others() {
    for (whole, expected) in [
        (28, (14, 1, 10, true, true)),
        (16, (2, 1, 10, true, true)),
        (15, (2, 1, 10, true, false)),
        (14, (3, 1, 10, false, false)),
        (12, (2, 0, 10, false, false)),
        (6, (2, 0, 4, false, false)),
    ] {
        let (mut editor, host, input) = native(whole + 2, 20);
        let typed = "word ".repeat(200);
        editor.set_internal_text(input, &typed);
        assert_eq!(heights(&editor, host), expected, "{whole}");
    }
}

/// The padding is the composer's alone: no other buffer docked in its place, and no other dock, has it.
#[test]
fn only_a_composer_is_padded() {
    let (mut editor, host, _) = native(30, 80);
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    let strip = editor.dock_window(host, DockSlot::Strip).unwrap();
    assert_eq!(editor.frame().padding(strip), None);
    let other = editor.add_buffer(Buffer::from_text("other", ""));
    editor.set_dock(host, DockSlot::Composer, Some(other));
    assert_eq!(editor.dock_window(host, DockSlot::Composer), Some(composer));
    assert_eq!(rows(&editor, composer), 1);
    assert_eq!(editor.frame().padding(composer), None);
    assert!(editor.frame().footer(editor.window().id()).is_some());
}
