//! A session's footer as a frame draws it: where it is, in which faces, and in which windows.
//!
//! Must not depend on: the window or the renderer; a frame's panes are read.

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::frame::{CellRect, FrameSize, Way};
use oso_core::window::{DockSlot, WindowId};
use oso_render::{Face, PaneContent, PaneId};

use super::face;
use crate::app::draw::tiled::{Part, pane_id, statuses_into};
use crate::app::draw::{Cells, with_frame};
use crate::app::sessions::NativeSessions;
use crate::sessions::list::SpaceBands;
use crate::sessions::native::footer::{Footer, Said, Tone};
use crate::terminal::hosted::Terminals;

/// A session's buffer in a frame of `rows` by `columns`, its composer docked under it, with a footer.
fn session(rows: usize, columns: usize) -> (Editor, BufferId) {
    let mut editor = Editor::default();
    let host = editor.add_native_session_buffer("*claude: s*");
    editor.show(host);
    let composer = editor.add_internal_buffer("*compose: s*".to_owned(), Mode::Composer);
    editor.set_dock(host, DockSlot::Composer, Some(composer));
    editor.set_footer(host, Some(String::new()));
    editor.set_frame_size(FrameSize { rows, columns });
    (editor, host)
}

fn footer(mode: &str, strayed: bool, model: &str) -> Footer {
    let said = Said {
        runs: true,
        mode: Some(mode),
        strayed,
        ..Said::default()
    };
    Footer::of(said, model)
}

fn auto() -> Footer {
    footer("auto", false, "Opus 5.5")
}

/// A pane: which, where, and its text by face.
type Shown = (PaneId, CellRect, Vec<(String, Face)>);

/// The panes of `window`'s footer, where its session says `footer`.
fn shown(editor: &Editor, window: WindowId, (host, footer): (BufferId, Footer)) -> Vec<Shown> {
    let (terminals, bands) = (Terminals::default(), SpaceBands::default());
    let natives = NativeSessions::footed(host, footer);
    let mut statuses = Vec::new();
    statuses_into(editor, &mut statuses);
    let frame = (editor, &terminals, &bands, Some(&natives));
    let ids = [Part::Footer, Part::Model].map(|part| pane_id(window, part));
    with_frame(frame, None, &statuses, &mut Cells::default(), |panes| {
        (panes.iter())
            .filter(|pane| ids.contains(&pane.id))
            .map(|pane| {
                assert_eq!(pane.background, Some(Face::Text));
                let PaneContent::Lines([line]) = pane.content else {
                    panic!("a footer of one line");
                };
                let spans = line.iter().filter(|span| !span.text.is_empty());
                let spans = spans.map(|span| (span.text.to_owned(), span.face));
                (pane.id, pane.region, spans.collect())
            })
            .collect()
    })
}

fn said(panes: &[Shown]) -> Vec<(String, Face)> {
    panes.iter().flat_map(|pane| pane.2.clone()).collect()
}

fn dim(text: &str) -> (String, Face) {
    (text.to_owned(), Face::Prompt)
}

#[test]
fn the_mode_is_at_the_left_in_its_face_and_the_model_at_the_right() {
    for columns in [44, 60, 100, 120] {
        let (editor, host) = session(24, columns);
        let window = editor.window().id();
        let panes = shown(&editor, window, (host, auto()));
        let mode = ("▶▶ auto mode on".to_owned(), Face::Warning);
        let key = dim(" (S-TAB to cycle)");
        assert_eq!(said(&panes), [dim("  "), mode, key, dim("Opus 5.5")]);
        // Side by side, over the whole row under the docks, the model a column from the edge.
        let row = editor.frame().footer(window).unwrap();
        let [left, right] = [panes[0].1, panes[1].1];
        assert_eq!((left.row, right.row), (row.row, row.row));
        assert_eq!((left.column, left.column + left.columns), (0, right.column));
        assert_eq!(right.columns, "Opus 5.5".len() + 1);
        assert_eq!(right.column + right.columns, row.column + row.columns);
        // In line with the composer's text.
        let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
        assert_eq!(editor.regions(composer).unwrap().text.column, 2);
    }
}

/// Narrow, it leaves out the key and then the model; its mode is cut by its pane at worst.
#[test]
fn a_narrow_window_keeps_the_mode() {
    for (columns, parts) in [(40, 3), (28, 3), (26, 2), (20, 2), (10, 2)] {
        let (editor, host) = session(24, columns);
        let panes = shown(&editor, editor.window().id(), (host, auto()));
        let said = said(&panes);
        assert_eq!(said.len(), parts, "{columns}: {said:?}");
        assert_eq!(said[1].0, "▶▶ auto mode on");
        let cells: usize = panes.iter().map(|pane| pane.1.columns).sum();
        assert_eq!(cells, columns);
    }
}

/// Each kind of mode has a face, and none but the mode's is in it.
#[test]
fn each_kind_of_mode_has_a_face_of_its_own() {
    let tones = [
        Tone::Quiet,
        Tone::Edits,
        Tone::Plan,
        Tone::Auto,
        Tone::Alarm,
    ];
    let faces = tones.map(face);
    for (at, face) in faces.iter().enumerate() {
        assert!(!faces[..at].contains(face));
    }
    let (editor, host) = session(24, 100);
    for mode in ["acceptEdits", "plan", "auto", "bypassPermissions", "what"] {
        let footer = footer(mode, true, "auto mode on");
        let said = said(&shown(&editor, editor.window().id(), (host, footer)));
        let (mode, others) = (&said[1], [&said[..1], &said[2..]].concat());
        assert_ne!(mode.1, Face::Prompt);
        assert!(others.iter().all(|(_, face)| *face == Face::Prompt));
    }
}

/// While a request's line has the composer's place the key does nothing there, and is not named.
#[test]
fn in_the_lines_place_it_names_no_key() {
    let (mut editor, host) = session(24, 100);
    let line = editor.add_internal_buffer("*request: s*".to_owned(), Mode::Request);
    editor.set_internal_text(line, "y  allow");
    editor.set_dock(host, DockSlot::Card, Some(line));
    let window = editor.window().id();
    let with = said(&shown(&editor, window, (host, auto())));
    assert!(with.contains(&dim(" (S-TAB to cycle)")), "above a draft");
    editor.set_dock(host, DockSlot::Composer, None);
    let without = said(&shown(&editor, window, (host, auto())));
    assert_eq!(without.len(), with.len() - 1);
    assert!(!without.contains(&dim(" (S-TAB to cycle)")));
}

/// Of a session in two windows, the one that carries its docks has it; a short one has none.
#[test]
fn only_the_window_that_carries_the_docks_has_it() {
    let (mut editor, host) = session(24, 120);
    let first = editor.window().id();
    let second = editor.split_window(Way::Across, false).unwrap();
    let carries =
        |editor: &Editor| [first, second].map(|id| !shown(editor, id, (host, auto())).is_empty());
    let before = carries(&editor);
    assert_eq!(before.iter().filter(|has| **has).count(), 1);
    let other = if before[0] { second } else { first };
    editor.select_window(other);
    assert_eq!(carries(&editor), before.map(|has| !has));
    editor.set_frame_size(FrameSize {
        rows: 6,
        columns: 120,
    });
    assert_eq!(carries(&editor), [false, false]);
    // Its session handed to the TUI: no row.
    editor.set_frame_size(FrameSize {
        rows: 24,
        columns: 120,
    });
    editor.set_footer(host, None);
    assert_eq!(carries(&editor), [false, false]);
}

/// It is drawn, and is no window that a person goes to, and no text of the host's.
#[test]
fn nothing_reads_it_or_goes_to_it() {
    let (mut editor, host) = session(24, 100);
    let window = editor.window().id();
    assert!(!shown(&editor, window, (host, auto())).is_empty());
    assert!(editor.buffer(host).unwrap().text().is_empty());
    let mut seen = vec![window];
    for _ in 0..4 {
        editor.other_window(1);
        seen.push(editor.window().id());
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 2, "the transcript and the composer");
}

/// Another buffer in the composer's place, as a question's answer will be, has not its keys.
#[test]
fn another_buffer_in_the_composers_place_names_no_key() {
    let (mut editor, host) = session(24, 100);
    let other = editor.add_internal_buffer("*answer: s*".to_owned(), Mode::Fundamental);
    editor.set_dock(host, DockSlot::Composer, Some(other));
    let docked = editor.dock_window(host, DockSlot::Composer).unwrap();
    assert_eq!(editor.window_by_id(docked).unwrap().buffer(), other);
    let said = said(&shown(&editor, editor.window().id(), (host, auto())));
    assert!(!said.contains(&dim(" (S-TAB to cycle)")), "{said:?}");
}
