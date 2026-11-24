//! Tests of a composer's rules (ADR 0170): on its own panes, from its window's edge to its edge,
//! whatever else is docked, and on nothing else.
//!
//! Must not depend on: the window or the renderer; a frame's panes are read.

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::frame::{CellRect, FrameSize, Way};

use oso_core::window::{DockSlot, WindowId};
use oso_render::{Face, PaneId, Rules};

use super::RULES;
use crate::app::draw::tiled::{Part, pane_id, statuses_into};
use crate::app::draw::{Cells, with_frame};
use crate::app::sessions::NativeSessions;
use crate::sessions::list::SpaceBands;
use crate::terminal::hosted::Terminals;

/// An editor on a session's buffer, with nothing docked, and that buffer.
fn session(rows: usize, columns: usize) -> (Editor, BufferId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows, columns });
    let host = editor.add_native_session_buffer("*claude: s*");
    editor.show(host);
    (editor, host)
}

/// `host`'s `slot` shows a buffer of `mode` with `text`: its window.
fn dock(
    editor: &mut Editor,
    (host, slot): (BufferId, DockSlot),
    mode: Mode,
    text: &str,
) -> WindowId {
    let id = editor.add_internal_buffer(format!("*{slot:?} {mode:?} {}*", text.len()), mode);
    editor.set_internal_text(id, text);
    editor.set_dock(host, slot, Some(id));
    editor.dock_window(host, slot).unwrap()
}

/// The panes of a frame that have rules: which, where, their background and rules.
fn ruled(editor: &Editor) -> Vec<(PaneId, CellRect, Option<Face>, Rules)> {
    let (terminals, bands) = (Terminals::default(), SpaceBands::default());
    let natives = NativeSessions::saying(["Write to Claude"; 3], "", None);
    let mut statuses = Vec::new();
    statuses_into(editor, &mut statuses);
    let shown = (editor, &terminals, &bands, Some(&natives));
    with_frame(shown, None, &statuses, &mut Cells::default(), |panes| {
        (panes.iter())
            .filter(|pane| pane.rules != Rules::NONE)
            .map(|pane| (pane.id, pane.region, pane.background, pane.rules))
            .collect()
    })
}

/// Whether `window`'s panes alone have rules, its text's and its gutter's, both on the float's
/// background, and its placeholder's if it is `empty`.
fn assert_ruled(editor: &Editor, window: WindowId, empty: bool) {
    let regions = *editor.regions(window).unwrap();
    let id = |part| pane_id(window, part);
    let float = Some(Face::Float);
    let mut wanted = vec![(id(Part::Text), regions.text, float, RULES)];
    if empty {
        let region = CellRect {
            column: regions.text.column + 1,
            columns: regions.text.columns - 1,
            ..regions.text
        };
        wanted.push((id(Part::Placeholder), region, None, RULES));
    }
    wanted.push((id(Part::Gutter), regions.gutter, float, RULES));
    assert_eq!(ruled(editor), wanted);
    // Side by side from its window's edge to its edge.
    assert_eq!(
        regions.gutter.column + regions.gutter.columns,
        regions.text.column
    );
    let across = (regions.gutter.column, regions.area().columns);
    let host = (editor.windows())
        .filter(|window| window.docked().is_none())
        .filter_map(|window| editor.regions(window.id()))
        .map(|regions| (regions.area().column, regions.area().columns));
    assert!(host.into_iter().any(|host| host == across), "{across:?}");
}

#[test]
fn a_composer_has_rules_over_its_gutter_and_its_text_at_every_width() {
    for columns in [20, 60, 100, 120] {
        let (mut editor, host) = session(24, columns);
        assert!(ruled(&editor).is_empty(), "no composer");
        let composer = dock(&mut editor, (host, DockSlot::Composer), Mode::Composer, "");
        assert_ruled(&editor, composer, true);
        editor.select_window(composer);
        assert_ruled(&editor, composer, true);
        let buffer = editor.window_by_id(composer).unwrap().buffer();
        editor.set_internal_text(buffer, "one\ntwo\nthree");
        editor.set_frame_size(FrameSize { rows: 24, columns });
        assert_eq!(editor.regions(composer).unwrap().text.rows, 3);
        assert_ruled(&editor, composer, false);
    }
}

#[test]
fn a_composer_has_its_rows_as_before() {
    let (mut editor, host) = session(24, 80);
    let tree = editor.window().id();
    let before = editor.regions(tree).unwrap().text.rows;
    dock(&mut editor, (host, DockSlot::Composer), Mode::Composer, "");
    assert_eq!(editor.regions(tree).unwrap().text.rows, before - 1);
}

#[test]
fn a_card_a_line_and_the_strip_have_none_beside_the_composers() {
    for above in [Mode::Card, Mode::Request] {
        let (mut editor, host) = session(24, 80);
        dock(&mut editor, (host, DockSlot::Card), above, "y  allow");
        dock(
            &mut editor,
            (host, DockSlot::Strip),
            Mode::Fundamental,
            "▸ Tasks (1/2)",
        );
        assert!(ruled(&editor).is_empty(), "no composer");
        let composer = dock(&mut editor, (host, DockSlot::Composer), Mode::Composer, "");
        assert_ruled(&editor, composer, true);
    }
}

#[test]
fn a_line_in_the_composers_place_has_none_and_the_composer_has_them_again() {
    let (mut editor, host) = session(24, 80);
    let line = dock(
        &mut editor,
        (host, DockSlot::Card),
        Mode::Request,
        "y  allow",
    );
    let line = editor.window_by_id(line).unwrap().buffer();
    let composer = dock(&mut editor, (host, DockSlot::Composer), Mode::Composer, "");
    let buffer = editor.window_by_id(composer).unwrap().buffer();
    // As `request::place` gives it, and takes it back.
    editor.set_dock(host, DockSlot::Composer, None);
    editor.set_dock(line, DockSlot::Composer, Some(buffer));
    assert!(ruled(&editor).is_empty());
    editor.set_dock(line, DockSlot::Composer, None);
    editor.set_dock(host, DockSlot::Composer, Some(buffer));
    let back = editor.dock_window(host, DockSlot::Composer).unwrap();
    assert_ruled(&editor, back, true);
}

#[test]
fn another_buffer_in_the_composers_place_has_none() {
    for mode in [Mode::Request, Mode::Card, Mode::Fundamental] {
        let (mut editor, host) = session(24, 80);
        dock(&mut editor, (host, DockSlot::Composer), mode, "");
        assert!(ruled(&editor).is_empty(), "{mode:?}");
    }
}

#[test]
fn a_secret_composer_has_them_with_no_placeholder() {
    let (mut editor, host) = session(24, 80);
    let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
    secret.set_mode(Mode::Composer);
    let secret = editor.add_buffer(secret);
    editor.set_dock(host, DockSlot::Composer, Some(secret));
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    assert_ruled(&editor, composer, false);
}

/// Of a session in two windows only the one that carries its docks has rules, and a host too short
/// for them has none.
#[test]
fn only_the_window_that_carries_the_docks_has_them_and_a_short_one_none() {
    let (mut editor, host) = session(24, 120);
    let composer = dock(&mut editor, (host, DockSlot::Composer), Mode::Composer, "");
    let first = editor.window().id();
    editor.split_window(Way::Across, false).unwrap();
    assert_ruled(&editor, composer, true);
    let other = (editor.frame().windows().iter())
        .map(|(id, _)| *id)
        .find(|&id| id != first && id != composer)
        .unwrap();
    editor.select_window(other);
    let column = |id| editor.regions(id).unwrap().area().column;
    assert_eq!(column(composer), column(other));
    assert_ruled(&editor, composer, true);
    editor.set_frame_size(FrameSize {
        rows: 4,
        columns: 120,
    });
    assert!(editor.regions(composer).is_none(), "hidden");
    assert!(ruled(&editor).is_empty());
}

/// A host that gives its composer fewer rows than its text: the rules are at the rows that show.
#[test]
fn a_composer_cut_by_a_short_host_is_ruled_at_the_rows_that_show() {
    let (mut editor, host) = session(7, 80);
    let composer = dock(
        &mut editor,
        (host, DockSlot::Composer),
        Mode::Composer,
        "1\n2\n3\n4\n5\n6\n7\n8\n9",
    );
    assert!(editor.regions(composer).unwrap().text.rows < 9);
    assert_ruled(&editor, composer, false);
}
