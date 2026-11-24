//! A composer that gives its place to its call's line, and takes it back, in an editor's windows.
//!
//! Must not depend on: the app; the tests lay out an editor's windows.

use oso_core::buffer::{BufferId, Mode, Replacement};
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::window::DockSlot;

use super::{carrier, free, give, given, take, take_if_it_fits};
use crate::app::sessions::hosting::native::Native;
use crate::app::sessions::hosting::native::composer::new_docked_composer;
use crate::app::sessions::hosting::native::markdown::tests::native;

struct Setup {
    editor: Editor,
    native: Native,
    transcript: BufferId,
    line: BufferId,
    composer: BufferId,
}

/// A transcript in a window `rows` high, with a line of `line` rows and an empty composer.
fn setup(rows: usize, line_rows: usize) -> Setup {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows, columns: 60 });
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.show(transcript);
    let line = editor.add_internal_buffer("*request: s*", Mode::Request);
    editor.set_internal_text(line, &vec!["keys"; line_rows].join("\n"));
    editor.set_dock(transcript, DockSlot::Card, Some(line));
    let composer = new_docked_composer(&mut editor, transcript, "s");
    let mut its = native(transcript);
    its.line = Some(line);
    its.composing.buffer = Some(composer);
    Setup {
        editor,
        native: its,
        transcript,
        line,
        composer,
    }
}

impl Setup {
    /// The rows of the dock in `slot`.
    fn rows(&self, slot: DockSlot) -> Option<usize> {
        let window = self.editor.dock_window(self.transcript, slot)?;
        Some(self.editor.regions(window)?.text.rows)
    }

    fn write(&mut self, text: &str) {
        let len = self.editor.buffer(self.composer).unwrap().text().len();
        let edit = Replacement {
            range: 0..len,
            text,
        };
        self.editor.apply_edits(self.composer, &[edit]).unwrap();
    }
}

#[test]
fn the_line_stands_where_the_composer_was_and_the_composer_is_kept() {
    let mut s = setup(20, 1);
    let bottom = |s: &Setup| {
        let host = s.editor.window().id();
        let text = |id| s.editor.regions(id).unwrap().text;
        (
            text(host).rows,
            s.editor
                .dock_window(s.transcript, DockSlot::Card)
                .map(text)
                .unwrap()
                .row,
        )
    };
    let (rows, row) = bottom(&s);
    assert!(!given(&s.editor, &s.native));
    give(&mut s.editor, s.transcript, s.line, s.composer);
    assert!(given(&s.editor, &s.native));
    assert_eq!(bottom(&s), (rows + 1, row + 1));
    assert_eq!(s.rows(DockSlot::Composer), None);
    assert!(s.editor.buffer(s.composer).is_some());
    take(&mut s.editor, s.transcript, s.line, s.composer);
    assert!(!given(&s.editor, &s.native));
    assert_eq!(
        (bottom(&s), s.rows(DockSlot::Composer)),
        ((rows, row), Some(1))
    );
}

/// Where its place is given it shows nowhere, and is no buffer of its own.
#[test]
fn a_composer_that_gave_its_place_is_in_no_list_and_in_no_window() {
    let mut s = setup(20, 1);
    let host = s.editor.window().id();
    give(&mut s.editor, s.transcript, s.line, s.composer);
    assert_eq!(s.editor.buffer_named("*compose: s*"), None);
    let shows = |s: &Setup| {
        (s.editor.frame().windows().iter())
            .filter(|(_, regions)| regions.text.columns > 0 && regions.text.rows > 0)
            .filter_map(|(id, _)| s.editor.window_by_id(*id))
            .any(|window| window.buffer() == s.composer)
    };
    assert!(!shows(&s));
    s.editor.other_window(1);
    s.editor.other_window(1);
    assert!(s.editor.window().buffer() != s.composer);
    // Nor once the window is split.
    s.editor.select_window(host);
    s.editor.split_window(Way::Down, true).unwrap();
    assert!(!shows(&s));
}

#[test]
fn only_a_composer_with_nothing_in_it_is_free() {
    let mut s = setup(20, 1);
    assert!(free(&s.editor, &s.native));
    for text in ["a", " ", "\n"] {
        s.write(text);
        assert!(!free(&s.editor, &s.native), "{text:?}");
    }
    s.write("");
    assert!(free(&s.editor, &s.native));
    // A queued message is edited in it, its text taken out.
    s.native.composing.edits_a_queued();
    assert!(!free(&s.editor, &s.native));
    s.native.composing = Default::default();
    s.native.composing.buffer = Some(s.composer);
    assert!(free(&s.editor, &s.native));
    // Killed by the person.
    s.native.composing.buffer = None;
    assert!(!free(&s.editor, &s.native));
}

/// What is put in while its place is given is there once it has it again, undone as it was typed.
#[test]
fn a_text_put_in_meanwhile_is_kept() {
    let mut s = setup(20, 1);
    give(&mut s.editor, s.transcript, s.line, s.composer);
    s.write("a draft");
    assert!(!free(&s.editor, &s.native));
    take(&mut s.editor, s.transcript, s.line, s.composer);
    assert_eq!(
        s.editor.buffer(s.composer).unwrap().text().to_string(),
        "a draft"
    );
    assert_eq!(s.rows(DockSlot::Composer), Some(1));
}

/// A window too short for the line and the composer shows the line alone.
#[test]
fn a_short_window_shows_the_line_in_the_composers_place() {
    let mut s = setup(8, 3);
    let shown = |s: &Setup| carrier(&s.editor, s.transcript, DockSlot::Card).is_some();
    let rows = (4..14).find(|rows| {
        s.editor.set_frame_size(FrameSize {
            rows: *rows,
            columns: 60,
        });
        let both = shown(&s);
        give(&mut s.editor, s.transcript, s.line, s.composer);
        let alone = shown(&s);
        take(&mut s.editor, s.transcript, s.line, s.composer);
        alone && !both
    });
    assert!(rows.is_some());

    // Its composer is hidden then, and point stays on the line.
    give(&mut s.editor, s.transcript, s.line, s.composer);
    let line = s.editor.dock_window(s.transcript, DockSlot::Card).unwrap();
    s.editor.select_window(line);
    let back = take_if_it_fits(&mut s.editor, s.transcript, s.line, s.composer);
    assert!(!back && given(&s.editor, &s.native) && shown(&s));
    assert_eq!(s.editor.window().id(), line);
    // In a taller one it has its place.
    s.editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 60,
    });
    assert!(take_if_it_fits(
        &mut s.editor,
        s.transcript,
        s.line,
        s.composer
    ));
    assert!(!given(&s.editor, &s.native));
}
