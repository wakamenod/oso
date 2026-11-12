//! Tests of undo and redo a slice at a time (ADR 0119): in slices of any size a crossing gives
//! what crossing at once gives, cancelling it at any slice changes nothing, a count stops at
//! the first step it cannot cross, a buffer changed under it abandons it, and the echo line.
//!
//! Must not depend on: `command` or `keymap`; these drive the editor directly, as the
//! dispatcher and the host would.

use std::ops::Range;

use proptest::prelude::*;

use super::super::edit_tests::{edit, editor_with, second_window, set_at, text};
use super::super::wrap_tests::settle;
use super::*;
use crate::buffer::{Buffer, ContentId, Mode, Replacement, SeenGroup};
use crate::editor::Status;
use crate::motion;
use crate::rope::tests::{ATOMS, floor_boundary, text_strategy};
use crate::syntax::{Highlight, Highlights};
use crate::window::ViewSize;

/// A command of the random histories: the selected window's cursors and a mark, then batches
/// of edits.
#[derive(Clone, Debug)]
struct Command {
    cursors: Vec<usize>,
    mark: Option<usize>,
    batches: Vec<Vec<(usize, usize, &'static str)>>,
}

/// An editor's state and history to cross, and how.
#[derive(Clone, Debug)]
struct Case {
    text: String,
    commands: Vec<Command>,
    /// How many of the last commands join one step, as `!` and a macro's replay join theirs.
    joined: usize,
    /// How many steps are undone before the crossing.
    undone: usize,
    count: isize,
    /// The second window's cursors.
    other: Vec<usize>,
    wraps: bool,
    highlighted: bool,
    /// What each slice does, and the bytes of a piece.
    work: usize,
    piece: usize,
}

fn command() -> impl Strategy<Value = Command> {
    let edits = prop::collection::vec((0usize..80, 0usize..12, prop::sample::select(ATOMS)), 1..5);
    let cursors = prop::collection::vec(0usize..80, 1..4);
    let batches = prop::collection::vec(edits, 1..4);
    let parts = (cursors, prop::option::of(0usize..80), batches);
    parts.prop_map(|(cursors, mark, batches)| Command {
        cursors,
        mark,
        batches,
    })
}

fn case() -> impl Strategy<Value = Case> {
    let history = (
        text_strategy(40),
        prop::collection::vec(command(), 1..6),
        (0usize..4, 0usize..3),
    );
    let crossing = (
        prop_oneof![1isize..4, -3isize..0],
        prop::collection::vec(0usize..80, 1..4),
        (any::<bool>(), any::<bool>()),
        (1usize..4000, 1usize..24),
    );
    let parts = (history, crossing);
    parts.prop_map(|((text, commands, (joined, undone)), crossing)| {
        let (count, other, (wraps, highlighted), (work, piece)) = crossing;
        Case {
            text,
            commands,
            joined,
            undone,
            count,
            other,
            wraps,
            highlighted,
            work,
            piece,
        }
    })
}

/// Where a cursor can be in `id` at or before `offset`: a display cluster's start.
fn cluster(editor: &Editor, id: BufferId, offset: usize) -> usize {
    let buffer = editor.buffer(id).expect("the buffer");
    let text = buffer.text().to_string();
    let offset = floor_boundary(&text, offset.min(text.len()));
    motion::cluster_start(&buffer.display(), offset)
}

/// Offsets where a cursor can be in `id`.
fn at(editor: &Editor, id: BufferId, offsets: &[usize]) -> Vec<usize> {
    let mut at: Vec<usize> = offsets
        .iter()
        .map(|&offset| cluster(editor, id, offset))
        .collect();
    at.sort_unstable();
    at.dedup();
    at
}

/// The editor `case` describes, before its crossing.
fn build(case: &Case) -> Editor {
    let mut buffer = Buffer::from_text("t.rs", &case.text);
    buffer.set_mode(Mode::Rust);
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(4, 12));
    let id = editor.window().buffer();
    if case.highlighted {
        let buffer = editor.buffer_mut(id).expect("the buffer");
        let len = buffer.text().len();
        let mut highlights = Highlights::new(len);
        highlights.replace(
            len / 3..len / 2,
            [(len / 2 - len / 3, Some(Highlight::String))],
        );
        assert!(buffer.take_highlights(buffer.revision(), highlights));
    }
    if case.wraps {
        editor.set_wraps(id, true);
    }
    let other = at(&editor, id, &case.other);
    second_window(&mut editor, id, &other);
    let first_joined = case.commands.len().saturating_sub(case.joined);
    let mut mark = None;
    for (index, command) in case.commands.iter().enumerate() {
        if index == first_joined {
            mark = editor.buffer(id).map(Buffer::undo_mark);
        }
        let mut set = set_at(&at(&editor, id, &command.cursors));
        if let Some(offset) = command.mark {
            set.push_primary_mark(cluster(&editor, id, offset));
        }
        editor.windows[editor.selected].selections = set;
        editor.begin_undo_group();
        for batch in &command.batches {
            let edits: Vec<Replacement<'_>> = batch
                .iter()
                .map(|&(start, len, new)| edit(start..start + len, new))
                .collect();
            editor.apply_edits(id, &edits).expect("an edit");
        }
        editor.end_undo_group();
    }
    if case.joined > 1
        && let Some(mark) = mark
        && let Some(buffer) = editor.buffer_mut(id)
    {
        buffer.join_undo_since(mark, None);
    }
    for _ in 0..case.undone {
        let _ = editor.undo();
    }
    editor
}

/// What a person or the host can see of the editor, and its history.
#[derive(Debug, PartialEq)]
struct Seen {
    text: String,
    content: ContentId,
    revision: u64,
    windows: Vec<(SelectionSet, usize)>,
    rows: Vec<Option<Vec<(usize, bool)>>>,
    history: Vec<SeenGroup>,
    highlights: Option<Vec<Option<Highlight>>>,
    edited: Vec<Range<usize>>,
}

fn seen(editor: &mut Editor) -> Seen {
    let id = editor.windows[0].buffer();
    let windows = editor
        .windows
        .iter()
        .map(|window| (window.selections.clone(), window.scroll().start))
        .collect();
    settle(editor);
    let buffer = editor.buffer(id).expect("the buffer");
    let display = buffer.display();
    let rows = editor
        .windows
        .iter()
        .map(|window| {
            window.wrap().map(|wrap| {
                (0..wrap.row_count())
                    .map(|row| {
                        let row = wrap.row(&display, row);
                        (row.start, row.continues)
                    })
                    .collect()
            })
        })
        .collect();
    let len = buffer.text().len();
    Seen {
        text: buffer.text().to_string(),
        content: buffer.content_id(),
        revision: buffer.revision(),
        windows,
        rows,
        history: buffer.history_seen(),
        highlights: buffer
            .highlights()
            .map(|highlights| (0..len).map(|offset| highlights.at(offset)).collect()),
        edited: buffer.edited_ranges().to_vec(),
    }
}

/// Runs the crossing under way to its end, checking that its share done never goes back
/// within a step and that the steps left only go down. Returns its result.
fn finish(editor: &mut Editor, begun: Result<(), EditError>) -> Result<(), EditError> {
    let mut last = (usize::MAX, 0.0);
    let mut result = begun;
    while let Some(crossing) = editor.crossing() {
        let now = (crossing.left(), crossing.done());
        assert!(now.0 <= last.0, "steps left went up");
        assert!(
            now.0 < last.0 || now.1 >= last.1,
            "the share done went back"
        );
        assert!((0.0..=1.0).contains(&now.1));
        last = now;
        if let Some(ended) = editor.go_on_crossing() {
            result = ended;
        }
    }
    result
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A crossing in slices of any size, cut between batches, between splices and inside a
    /// splice's text, gives what one at once gives (ADR 0119).
    #[test]
    fn a_crossing_in_slices_gives_what_one_at_once_gives(case in case()) {
        let mut whole = build(&case);
        whole.set_crossing_slices(usize::MAX, PIECE);
        let at_once = whole.begin_crossing(case.count);
        prop_assert!(whole.crossing().is_none());
        let mut sliced = build(&case);
        sliced.set_crossing_slices(case.work, case.piece);
        let begun = sliced.begin_crossing(case.count);
        let result = finish(&mut sliced, begun);
        prop_assert_eq!(result, at_once);
        prop_assert_eq!(seen(&mut sliced), seen(&mut whole));
    }

    /// Cancelling a crossing at any slice leaves everything as it was, its history too.
    #[test]
    fn cancelling_at_any_slice_changes_nothing(
        case in case(),
        work in 1usize..1100,
        slices in 0usize..6,
    ) {
        let mut before = build(&case);
        let mut editor = build(&case);
        editor.set_crossing_slices(work, case.piece);
        // One step, as the steps of a count before the one cancelled stay crossed.
        let _ = editor.begin_crossing(case.count.signum());
        for _ in 0..slices {
            if editor.crossing().is_none() {
                break;
            }
            prop_assert!(editor.go_on_crossing().is_none() || editor.crossing().is_none());
        }
        // One that ended has nothing to cancel.
        if editor.crossing().is_none() {
            return Ok(());
        }
        prop_assert!(editor.cancel_crossing());
        prop_assert!(editor.crossing().is_none());
        prop_assert_eq!(seen(&mut editor), seen(&mut before));
    }
}

/// An editor whose buffer had `killed` killed from between `before` and `after`.
fn killed(before: &str, killed: &str, after: &str) -> (Editor, BufferId) {
    let (mut editor, id) = editor_with(&format!("{before}{killed}{after}"));
    editor.begin_undo_group();
    let range = before.len()..before.len() + killed.len();
    editor.apply_edits(id, &[edit(range, "")]).expect("a kill");
    editor.end_undo_group();
    (editor, id)
}

#[test]
fn a_kill_goes_back_in_pieces_over_turns_and_the_buffer_changes_once() {
    let kill = "日本語の text\r\n".repeat(30);
    let (mut editor, id) = killed("before ", &kill, " after");
    let revision = editor.buffer(id).map(Buffer::revision);
    editor.set_crossing_slices(64, 16);
    editor.begin_crossing(1).expect("an undo");
    let mut slices = 1;
    while editor.crossing().is_some() {
        // Until it commits, every reader sees the text as it was.
        assert_eq!(text(&editor, id), "before  after");
        assert_eq!(editor.buffer(id).map(Buffer::revision), revision);
        assert_eq!(editor.go_on_crossing().unwrap_or(Ok(())), Ok(()));
        slices += 1;
    }
    // Four pieces a slice of 64.
    assert!(slices > kill.len() / 64, "{slices} slices");
    assert_eq!(text(&editor, id), format!("before {kill} after"));
    assert_eq!(
        editor.buffer(id).map(Buffer::revision),
        revision.map(|r| r + 1)
    );
    // The selected window is back where the kill began, and redo takes it out again.
    assert_eq!(editor.window().selections.primary().cursor, 0);
    editor.redo().expect("a redo");
    assert_eq!(text(&editor, id), "before  after");
}

#[test]
fn a_count_stops_at_the_first_step_it_cannot_cross_and_keeps_those_before() {
    for work in [usize::MAX, SPLICE_WORK_IN_TESTS] {
        let (mut editor, id) = editor_with("abc");
        editor.set_crossing_slices(work, 4);
        for typed in ["x", "y"] {
            editor.begin_undo_group();
            editor
                .apply_edits(id, &[edit(0..0, typed)])
                .expect("an edit");
            editor.end_undo_group();
        }
        let begun = editor.begin_crossing(5);
        assert_eq!(finish(&mut editor, begun), Err(EditError::NoUndo));
        assert_eq!(text(&editor, id), "abc");
        assert_eq!(editor.buffer(id).map(Buffer::undo_lens), Some((0, 2)));
    }
    // The first step's error changes nothing.
    let (mut editor, _) = editor_with("abc");
    assert_eq!(editor.begin_crossing(-2), Err(EditError::NoRedo));
    assert!(editor.crossing().is_none());
}

/// A slice's work in the tests that cut a step between splices: one splice.
const SPLICE_WORK_IN_TESTS: usize = 1;

#[test]
fn a_buffer_changed_under_a_crossing_abandons_it_and_nothing_crosses() {
    let (mut editor, id) = editor_with("one two three");
    editor.begin_undo_group();
    let edits = [edit(0..3, "1"), edit(4..7, "2"), edit(8..13, "3")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.crossing().is_some());
    editor.begin_undo_group();
    editor.apply_edits(id, &[edit(0..0, "!")]).expect("an edit");
    editor.end_undo_group();
    assert_eq!(editor.go_on_crossing(), Some(Err(EditError::Abandoned)));
    assert!(editor.crossing().is_none());
    assert_eq!(text(&editor, id), "!1 2 3");
    assert_eq!(editor.buffer(id).map(Buffer::undo_lens), Some((2, 0)));
    assert_eq!(EditError::Abandoned.to_string(), "Undo abandoned");
}

#[test]
fn wrapping_turned_on_under_a_crossing_abandons_it() {
    let (mut editor, id) = editor_with("one two three");
    editor.begin_undo_group();
    let edits = [edit(0..3, "1"), edit(4..7, "2"), edit(8..13, "3")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.set_wraps(id, true));
    assert_eq!(editor.go_on_crossing(), Some(Err(EditError::Abandoned)));
    assert_eq!(text(&editor, id), "1 2 3");
}

#[test]
fn a_call_that_cannot_begin_leaves_the_crossing_under_way() {
    let (mut editor, id) = editor_with("one two three");
    editor.begin_undo_group();
    let edits = [edit(0..3, "1"), edit(4..7, "2"), edit(8..13, "3")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    assert_eq!(editor.begin_crossing(0), Ok(()));
    assert_eq!(editor.redo(), Err(EditError::NoRedo));
    assert!(editor.crossing().is_some());
    finish(&mut editor, Ok(())).expect("the undo");
    assert_eq!(text(&editor, id), "one two three");
}

#[test]
fn a_crossing_drops_itself_if_its_window_is_no_longer_selected() {
    let (mut editor, id) = editor_with("one two three");
    editor.begin_undo_group();
    let edits = [edit(0..3, "1"), edit(4..7, "2"), edit(8..13, "3")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    second_window(&mut editor, id, &[0]);
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    editor.selected = 1;
    assert_eq!(editor.go_on_crossing(), Some(Err(EditError::Abandoned)));
    assert_eq!(text(&editor, id), "1 2 3");
}

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.window().id(), &mut status);
    status.echo
}

#[test]
fn the_echo_line_says_how_far_it_is_from_the_second_slice() {
    let (mut editor, id) = editor_with(&"word ".repeat(8));
    for typed in ["a", "b"] {
        editor.begin_undo_group();
        let edits: Vec<_> = (0..8).map(|word| edit(word * 5..word * 5, typed)).collect();
        editor.apply_edits(id, &edits).expect("an edit");
        editor.end_undo_group();
    }
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(2).expect("an undo");
    // A crossing that ends within two slices says nothing.
    assert_eq!(echo(&editor), "");
    assert_eq!(editor.go_on_crossing(), None);
    // Each splice puts back nothing, so each of the eight is an eighth of the work.
    assert_eq!(echo(&editor), "Undoing… 25% (1 more)");
    while editor
        .crossing()
        .is_some_and(|crossing| crossing.left() > 0)
    {
        editor.go_on_crossing();
    }
    editor.go_on_crossing();
    assert_eq!(echo(&editor), "Undoing… 12%");
    finish(&mut editor, Ok(())).expect("the undo");
    assert_eq!(echo(&editor), "");
    assert_eq!(text(&editor, id), "word ".repeat(8));
    editor.begin_crossing(-1).expect("a redo");
    editor.go_on_crossing();
    assert_eq!(echo(&editor), "Redoing… 25%");
    assert!(format!("{:?}", editor.crossing()).contains("redo: true"));
}

/// An editor showing Rust source with highlights, three commands in its history, and a save
/// that took the lines they edited.
fn saving() -> (Editor, BufferId) {
    let mut buffer = Buffer::from_text("t.rs", "fn main() { let x = 1; }\n");
    buffer.set_mode(Mode::Rust);
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    let report = |editor: &mut Editor| {
        let buffer = editor.buffer_mut(id).expect("the buffer");
        let mut highlights = Highlights::new(buffer.text().len());
        highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
        assert!(buffer.take_highlights(buffer.revision(), highlights));
    };
    report(&mut editor);
    for edits in [[edit(3..7, "start")], [edit(16..17, "yy")]] {
        editor.begin_undo_group();
        editor.apply_edits(id, &edits).expect("an edit");
        editor.end_undo_group();
    }
    editor.begin_undo_group();
    let edits = [edit(0..2, "pub fn"), edit(21..22, "22")];
    editor.apply_edits(id, &edits).expect("an edit");
    editor.end_undo_group();
    let buffer = editor.buffer_mut(id).expect("the buffer");
    buffer.save_took_edits(Vec::new());
    (editor, id)
}

/// What a report and the end of a save do to `editor` while its buffer is at a revision.
fn meanwhile(editor: &mut Editor, id: BufferId, wrote: bool) {
    let buffer = editor.buffer_mut(id).expect("the buffer");
    let mut highlights = Highlights::new(buffer.text().len());
    highlights.replace(3..7, [(4, Some(Highlight::Function))]);
    assert!(buffer.take_highlights(buffer.revision(), highlights));
    buffer.save_ended_edits(wrote);
}

#[test]
fn a_report_and_a_save_that_end_meanwhile_are_taken_in_at_the_commit() {
    let mut edited = Vec::new();
    for wrote in [true, false] {
        let (mut whole, id) = saving();
        meanwhile(&mut whole, id, wrote);
        whole.undo().expect("an undo");
        let (mut sliced, _) = saving();
        sliced.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
        sliced.begin_crossing(1).expect("an undo");
        assert!(sliced.crossing().is_some());
        meanwhile(&mut sliced, id, wrote);
        finish(&mut sliced, Ok(())).expect("the undo");
        assert_eq!(seen(&mut sliced), seen(&mut whole), "wrote {wrote}");
        edited.extend(sliced.buffer(id).map(|buffer| buffer.edited_ranges().len()));
    }
    // A save that failed gives back the lines it took.
    assert!(edited[1] > edited[0], "{edited:?}");
}

/// An editor whose last two commands are under a crossing of the newest, in slices of one
/// splice, with a second window on the buffer.
fn crossing_under_way() -> (Editor, BufferId, (ContentId, u64)) {
    let (mut editor, id) = editor_with("one two three");
    let mark = editor
        .buffer(id)
        .map(Buffer::undo_mark)
        .expect("the buffer");
    for edits in [
        [edit(0..3, "1"), edit(8..13, "3")],
        [edit(2..2, "x"), edit(7..7, "y")],
    ] {
        editor.begin_undo_group();
        editor.apply_edits(id, &edits).expect("an edit");
        editor.end_undo_group();
    }
    second_window(&mut editor, id, &[0]);
    editor.set_crossing_slices(SPLICE_WORK_IN_TESTS, 4);
    editor.begin_crossing(1).expect("an undo");
    assert!(editor.crossing().is_some());
    (editor, id, mark)
}

#[test]
fn each_change_a_crossing_cannot_follow_abandons_it_and_changes_nothing() {
    type Change = fn(&mut Editor, BufferId, (ContentId, u64));
    let changes: [Change; 3] = [
        // The history joined, with no change to the text.
        |editor, id, mark| {
            let buffer = editor.buffer_mut(id).expect("the buffer");
            assert!(buffer.join_undo_since(mark, None));
        },
        // Another window's cursors moved.
        |editor, _, _| editor.windows[1].selections = set_at(&[2]),
        // A save took the edited lines.
        |editor, id, _| {
            let buffer = editor.buffer_mut(id).expect("the buffer");
            buffer.save_took_edits(Vec::new());
        },
    ];
    for change in changes {
        let (mut editor, id, mark) = crossing_under_way();
        change(&mut editor, id, mark);
        let before = seen(&mut editor);
        let result = finish(&mut editor, Ok(()));
        assert_eq!(result, Err(EditError::Abandoned));
        assert!(editor.crossing().is_none());
        assert_eq!(seen(&mut editor), before);
    }
}
