//! What a window shows of a request's block, as it is laid out: which rows can be read, where, and
//! when none can.
//!
//! Must not depend on: the app; the tests lay out an editor's windows.

use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::frame::{FrameSize, Way};
use oso_core::window::{DockSlot, Screen, WindowId};
use serde_json::json;

use super::super::{put_title, runs};
use super::{View, view_of};
use crate::app::sessions::hosting::native::carrier::carrier;
use crate::app::sessions::hosting::native::markdown::tests::native;
use crate::app::sessions::hosting::native::{NativeSessions, put_tail};
use crate::sessions::native::request::reading::{READ_AFTER, Reading};
use crate::sessions::native::request::{Block, Request};
use crate::sessions::native::tail::Part;
use crate::sessions::worker::SessionKey;

const KEY: SessionKey = 7;

/// A transcript of a few rows in a window of its own, with a request's block and line, and a
/// composer.
struct Setup {
    editor: Editor,
    natives: NativeSessions,
    transcript: BufferId,
    /// What of the block was read.
    reading: Reading,
    now: Instant,
}

impl Setup {
    fn new(size: (usize, usize), command: &str) -> Self {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", "notes\n"));
        editor.set_frame_size(FrameSize {
            rows: size.0,
            columns: size.1,
        });
        let transcript = editor.add_native_session_buffer("*claude: s*");
        editor.set_wraps(transcript, true);
        editor.show(transcript);
        let line = editor.add_internal_buffer("*request: s*", Mode::Request);
        editor.set_internal_text(line, " y  allow");
        editor.set_dock(transcript, DockSlot::Card, Some(line));
        let composer = editor.add_internal_buffer("*compose: s*", Mode::Composer);
        editor.set_dock(transcript, DockSlot::Composer, Some(composer));
        let mut its = native(transcript);
        its.line = Some(line);
        its.composing.buffer = Some(composer);
        let input = json!({"command": command});
        let block = Block::call("Bash", Some(&input), Instant::now());
        editor.refill_wraps();
        put_title(&mut editor, &mut its, &block, ((1, 1), size.1 - 1));
        let body = block.body();
        put_tail(
            &mut editor,
            &mut its,
            Part::Body,
            body.text.clone(),
            runs(body),
        );
        let mut natives = NativeSessions::default();
        natives.map.insert(KEY, its);
        Self {
            editor,
            natives,
            transcript,
            reading: Request::new(block).reading,
            now: Instant::now(),
        }
    }

    fn view(&self, watched: bool) -> View {
        let mut view = View::default();
        let mut screen = Screen::default();
        view_of(
            &self.editor,
            &self.natives,
            (KEY, watched),
            (&mut screen, &mut view),
        );
        view
    }

    /// The frame under way is presented, and half a second goes by with nothing more.
    fn seen(&mut self) {
        let view = self.view(true);
        self.reading.look(&view, true, self.now);
        self.now += READ_AFTER;
        let view = self.view(true);
        self.reading.look(&view, false, self.now);
        self.now += Duration::from_millis(1);
    }

    fn carrier(&self) -> Option<WindowId> {
        carrier(&self.editor, self.transcript, DockSlot::Card).map(|(id, _)| id)
    }

    fn select(&mut self, slot: DockSlot) {
        let window = self.editor.dock_window(self.transcript, slot).unwrap();
        self.editor.select_window(window);
    }
}

#[test]
fn a_block_in_its_window_is_read_from_its_transcript_and_its_docks() {
    for slot in [None, Some(DockSlot::Card), Some(DockSlot::Composer)] {
        let mut setup = Setup::new((30, 80), "ls\npwd");
        if let Some(slot) = slot {
            setup.select(slot);
        }
        assert_eq!(setup.reading.lines(), (0, 2));
        setup.seen();
        assert!(setup.reading.allows(), "{slot:?}");
    }
}

#[test]
fn white_space_is_marked_where_a_block_is_and_only_there() {
    let mut setup = Setup::new((30, 80), "ls");
    let notes = setup.editor.add_buffer(Buffer::from_text("other", ""));
    assert!(setup.natives.shows_block(setup.transcript));
    assert!(!setup.natives.shows_block(notes));
    let its = setup.natives.map.get_mut(&KEY).unwrap();
    for part in [Part::Title, Part::Body] {
        put_tail(&mut setup.editor, its, part, String::new(), Vec::new());
    }
    assert!(!setup.natives.shows_block(setup.transcript));
}

#[test]
fn nothing_is_read_from_another_window_or_unwatched() {
    let mut setup = Setup::new((30, 80), "ls");
    let view = setup.view(false);
    assert!(!view.attended && view.rows.is_empty());
    let here = setup.carrier().unwrap();
    let beside = setup.editor.split_window(Way::Across, true).unwrap();
    // A second window on the session carries the docks once selected, and is the one that counts.
    assert_eq!(setup.carrier(), Some(beside));
    assert_eq!(setup.view(true).window, Some(beside));
    let notes = setup.editor.add_buffer(Buffer::from_text("other", ""));
    setup.editor.show(notes);
    assert_eq!(setup.carrier(), Some(here));
    assert!(!setup.view(true).attended, "another buffer is selected");
    setup.seen();
    assert_eq!(setup.reading.lines(), (0, 1));
    setup.editor.select_window(here);
    setup.seen();
    assert!(setup.reading.allows());
}

#[test]
fn two_windows_on_the_session_read_only_what_carries_the_docks_shows() {
    let long: String = (0..100).map(|n| format!("echo {n}\n")).collect();
    let mut setup = Setup::new((40, 120), long.trim_end());
    let first = setup.carrier().unwrap();
    let second = setup.editor.split_window(Way::Down, true).unwrap();
    // The other shows the block's start, which is not read there.
    let start = setup.natives.map[&KEY].part_place(Part::Body).start;
    let window = setup.editor.window_by_id(first).unwrap();
    let mut selections = window.selections.clone();
    selections.move_primary(start);
    let top = oso_core::window::Scroll { start, left: 0 };
    (setup.editor).restore_positions_in(first, selections, top);
    assert_eq!(setup.carrier(), Some(second));
    setup.seen();
    assert!(setup.reading.lines().0 > 0, "its end, in the second");
    assert_eq!(setup.reading.unread(), Some(0));
}

#[test]
fn a_block_no_window_shows_wrapped_and_whole_is_not_read() {
    // Too narrow to wrap.
    let mut narrow = Setup::new((30, 14), "ls");
    assert!(!narrow.view(true).wraps);
    narrow.seen();
    assert!(!narrow.reading.may_ask());
    // Lines truncated, and then scrolled sideways.
    let mut cut = Setup::new((30, 40), &"x".repeat(100));
    cut.editor.set_wraps(cut.transcript, false);
    assert!(!cut.view(true).wraps);
    let window = cut.carrier().unwrap();
    cut.editor.scroll_view_sideways(window, 30);
    cut.seen();
    assert!(!cut.reading.may_ask());
    // Wrapped again, it is.
    cut.editor.set_wraps(cut.transcript, true);
    cut.editor.refill_wraps();
    cut.editor.follow_cursor();
    cut.seen();
    assert!(cut.reading.allows());
    // Too short for the docks, so that no line says what a key does.
    let mut short = Setup::new((6, 80), "ls");
    let shown = |setup: &Setup| setup.carrier().is_some();
    assert!(shown(&short));
    short.editor.set_frame_size(FrameSize {
        rows: 4,
        columns: 80,
    });
    assert!(!shown(&short));
    assert!(!short.view(true).attended);
}

#[test]
fn rows_of_a_wrap_not_made_yet_are_not_read() {
    // A window taken narrower, with no budget to wrap its rows again: they are the old width's.
    let words: String = (0..9000).map(|n| format!("{n} ")).collect();
    let mut setup = Setup::new((40, 200), words.trim_end());
    let window = setup.carrier().unwrap();
    // Every command's budget is spent in the windows before it.
    for _ in 0..4 {
        setup.editor.split_window(Way::Across, true).unwrap();
        let mut long = Buffer::from_text("long", &"あ ".repeat(200_000));
        let _ = long.set_mode(Mode::Fundamental);
        let id = setup.editor.add_buffer(long);
        setup.editor.set_wraps(id, true);
        setup.editor.show(id);
        setup
            .editor
            .current_mut()
            .0
            .selections
            .move_primary(300_000);
        setup.editor.follow_cursor();
    }
    setup.editor.select_window(window);
    setup.editor.balance_windows();
    let wide = |setup: &Setup| {
        let view = setup.view(true);
        assert!(view.attended && view.wraps);
        (view.rows.iter())
            .filter(|row| row.cells > view.columns)
            .count()
    };
    assert!(wide(&setup) > 0, "rows of the old width");
    setup.seen();
    assert!(!setup.reading.may_ask(), "{:?}", setup.reading.lines());
    // Wrapped, they are.
    setup.editor.refill_wraps();
    setup.editor.follow_cursor();
    assert_eq!(wide(&setup), 0);
    setup.seen();
    assert!(setup.reading.may_ask());
}

#[test]
fn what_streams_in_above_moves_no_reading_and_a_resize_forgets_none() {
    let long: String = (0..100).map(|n| format!("echo {n}\n")).collect();
    let mut setup = Setup::new((40, 80), long.trim_end());
    setup.seen();
    let (read, of) = setup.reading.lines();
    assert!(read > 0 && read < of);
    // A row above, of a subagent's call.
    let its = setup.natives.map.get_mut(&KEY).unwrap();
    let end = its.placed.end();
    let row = "▸ $ Bash · make\n";
    (setup.editor).replace_native_rows(setup.transcript, end..end, row, &[(row.len(), None)]);
    its.placed.put(0, end..end, row.len());
    setup.seen();
    assert_eq!(setup.reading.lines().0, read, "the same rows at its end");
    // Narrower, and shorter: what was read stays read.
    setup.editor.set_frame_size(FrameSize {
        rows: 20,
        columns: 50,
    });
    setup.seen();
    assert_eq!(setup.reading.lines().0, read);
    // Its start, then.
    let window = setup.carrier().unwrap();
    setup.editor.scroll_view(window, -200);
    for _ in 0..20 {
        setup.seen();
        setup.editor.scroll_view(window, 10);
    }
    setup.seen();
    assert!(setup.reading.allows(), "{:?}", setup.reading.lines());
}

#[test]
fn a_frame_not_presented_whole_reads_nothing() {
    let mut setup = Setup::new((30, 80), "ls");
    for _ in 0..10 {
        let view = setup.view(true);
        setup.reading.look(&view, false, setup.now);
        setup.now += READ_AFTER;
    }
    assert!(!setup.reading.allows() && !setup.reading.may_ask());
    setup.seen();
    assert!(setup.reading.allows());
    // Nothing open over it, but then: the wait starts again, and `y` finds no frame.
    let view = setup.view(false);
    setup.reading.look(&view, true, setup.now);
    assert!(!setup.reading.allows());
}
