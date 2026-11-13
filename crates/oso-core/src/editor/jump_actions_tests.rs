//! Tests of the actions on a jump's target (ADR 0116 step 6): what `SPC`, `W`, `K`, `Y`, `T`
//! and `c` do, with several cursors and in other windows, their undo steps, what they put in
//! the kill ring and on the clipboard, and what they refuse.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use super::test_buffers::secret_buffer;
use super::{Chose, CursorError, Editor, JumpActionError, Status, YankError};
use crate::buffer::{Buffer, BufferId, EditError, Mode, ReadOnly};
use crate::display_map::TabWidth;
use crate::frame::{FrameSize, Way};
use crate::jump::Kind;
use crate::load::decode_all;
use crate::selection::{MAX_CURSORS, MarkActive, Selection, SelectionSet};
use crate::window::WindowId;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
    editor.set_frame_size(FrameSize {
        rows: 16,
        columns: 40,
    });
    editor
}

fn text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

fn text_of(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

fn cursors(editor: &Editor) -> Vec<usize> {
    let set = &editor.window().selections;
    set.iter().map(|selection| selection.cursor).collect()
}

fn newest(editor: &Editor) -> Option<String> {
    editor.kill_ring().get(0).map(|entry| entry.joined())
}

fn line(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn at(editor: &mut Editor, offsets: &[usize]) {
    let parts = offsets.iter().map(|&at| Selection::at(at)).collect();
    editor.current_mut().0.selections = SelectionSet::from_parts(parts, 0).unwrap();
}

/// Types the label of the candidate at `start`, in `window` or the selected one, and gives
/// what its last key did.
fn choose(
    editor: &mut Editor,
    start: usize,
    window: Option<WindowId>,
) -> Result<Chose, JumpActionError> {
    let window = window.unwrap_or_else(|| editor.window().id());
    let jump = editor.jump().unwrap();
    let at = jump
        .candidates
        .iter()
        .position(|c| c.range.start == start && c.window == window)
        .unwrap_or_else(|| panic!("no candidate at {start}"));
    let label: Vec<char> = jump
        .labels
        .get(at)
        .unwrap()
        .iter()
        .map(|&key| char::from(jump.keys.byte(key)))
        .collect();
    let mut chose = Ok(Chose::NoSuch);
    for key in label {
        chose = editor.jump_choose(key);
    }
    chose
}

/// Starts a line jump, arms `action`, and types the label of the line starting at `start`.
fn act(editor: &mut Editor, action: char, start: usize) -> Result<Chose, JumpActionError> {
    assert!(editor.jump_begin(Kind::LineStart, false));
    assert_eq!(editor.jump_choose(action), Ok(Chose::Armed));
    choose(editor, start, None)
}

/// `begin` and `end` around `work`, as the dispatcher puts a command's undo group.
fn as_command<T>(editor: &mut Editor, work: impl FnOnce(&mut Editor) -> T) -> T {
    editor.begin_undo_group();
    let done = work(editor);
    editor.end_undo_group();
    editor.note_command_ended(false);
    done
}

#[test]
fn w_copies_the_targets_whole_line_to_the_ring_and_the_clipboard_and_stays() {
    let mut editor = editor("one\ntwo\nthree\n");
    assert!(editor.jump_begin(Kind::LineStart, false));
    assert_eq!(editor.jump_choose('W'), Ok(Chose::Armed));
    assert_eq!(line(&editor), "Jump to a line  [copy line]");
    assert_eq!(choose(&mut editor, 4, None), Ok(Chose::Jumped));
    assert!(editor.jump().is_none());
    assert_eq!(text(&editor), "one\ntwo\nthree\n");
    assert_eq!(cursors(&editor), [0]);
    assert_eq!(
        editor.window().selections.primary().mark,
        None,
        "no mark pushed"
    );
    assert_eq!(newest(&editor).as_deref(), Some("two\n"));
    let write = editor.take_clipboard_write().unwrap();
    assert_eq!((write.text(), write.secret), ("two\n".to_owned(), false));
}

#[test]
fn k_kills_the_line_in_one_undo_step_and_the_cursor_follows() {
    let mut editor = editor("one\ntwo\nthree\n");
    at(&mut editor, &[9]);
    let chose = as_command(&mut editor, |editor| act(editor, 'K', 4));
    assert_eq!(chose, Ok(Chose::Jumped));
    assert_eq!(text(&editor), "one\nthree\n");
    assert_eq!(cursors(&editor), [5]);
    assert_eq!(newest(&editor).as_deref(), Some("two\n"));
    editor.undo().unwrap();
    assert_eq!(text(&editor), "one\ntwo\nthree\n");
}

#[test]
fn an_actions_kill_is_an_entry_of_its_own() {
    let mut editor = editor("one\ntwo\n");
    as_command(&mut editor, |editor| {
        editor.kill(std::iter::once(0..1), crate::kill::Direction::Forward, true)
    })
    .unwrap();
    act(&mut editor, 'W', 4).unwrap();
    assert_eq!(newest(&editor).as_deref(), Some("two\n"));
    assert_eq!(editor.kill_ring().len(), 2);
}

#[test]
fn y_yanks_the_line_at_every_cursor_as_c_u_c_y() {
    let mut editor = editor("one\ntwo\n");
    at(&mut editor, &[0, 4]);
    assert_eq!(act(&mut editor, 'Y', 4), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "two\none\ntwo\ntwo\n");
    assert_eq!(cursors(&editor), [0, 8], "before what each yanked");
    let marks: Vec<_> = editor.window().selections.iter().map(|s| s.mark).collect();
    assert_eq!(marks, [Some(4), Some(12)]);
    assert!(!editor.window().selections.mark_active().is_active());
}

#[test]
fn y_replaces_a_region_shown_as_c_y_does() {
    let mut editor = editor("one\ntwo\n");
    let selections = &mut editor.current_mut().0.selections;
    *selections = SelectionSet::from_parts(
        vec![Selection {
            cursor: 3,
            mark: Some(0),
            goal: None,
        }],
        0,
    )
    .unwrap();
    selections.set_mark_active(MarkActive::Active);
    assert_eq!(act(&mut editor, 'Y', 4), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "two\n\ntwo\n");
}

#[test]
fn t_moves_the_line_to_the_cursor_in_one_undo_step() {
    let mut editor = editor("a\nb\nc\n");
    let chose = as_command(&mut editor, |editor| act(editor, 'T', 4));
    assert_eq!(chose, Ok(Chose::Jumped));
    assert_eq!(text(&editor), "c\na\nb\n");
    assert_eq!(cursors(&editor), [0]);
    editor.undo().unwrap();
    assert_eq!(
        text(&editor),
        "a\nb\nc\n",
        "the kill and the yank undo as one"
    );
}

#[test]
fn a_last_line_with_no_break_is_copied_alone_and_killed_with_the_break_before() {
    let mut editor = editor("a\nlast");
    act(&mut editor, 'W', 2).unwrap();
    assert_eq!(newest(&editor).as_deref(), Some("last"));
    act(&mut editor, 'K', 2).unwrap();
    assert_eq!(text(&editor), "a");
    assert_eq!(newest(&editor).as_deref(), Some("\nlast"));
}

#[test]
fn y_on_an_empty_last_line_yanks_nothing_older() {
    let mut editor = editor("a\n");
    act(&mut editor, 'W', 0).unwrap();
    assert!(editor.jump_begin(Kind::LineEnd, false));
    assert_eq!(editor.jump_choose('Y'), Ok(Chose::Armed));
    // The empty line after the last break has nothing to copy.
    assert_eq!(choose(&mut editor, 2, None), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "a\n", "the older entry did not go in");
}

#[test]
fn spc_sets_the_marks_at_the_cursors_and_the_region_runs_to_the_target() {
    let mut editor = editor("one\ntwo\n");
    at(&mut editor, &[1, 6]);
    // A region shown is left for a new one, as avy's `push-mark` leaves it.
    let selections = &mut editor.current_mut().0.selections;
    selections.update(|selection| {
        selection.mark = Some(0);
        Ok::<(), ()>(())
    });
    assert_eq!(act(&mut editor, ' ', 4), Ok(Chose::Jumped));
    let set = &editor.window().selections;
    assert!(set.mark_active().is_active());
    let parts: Vec<_> = set.iter().map(|s| (s.mark, s.cursor)).collect();
    assert_eq!(
        parts,
        [(Some(1), 4), (Some(6), 6)],
        "the primary went to the target"
    );
    assert_eq!(set.primary().cursor, 4);
    assert_eq!(
        set.mark_ring().iter().next(),
        Some(0),
        "its old mark in the ring"
    );
}

#[test]
fn c_adds_a_cursor_at_the_target_and_the_primary_stays() {
    let mut editor = editor("one\ntwo\nthree\n");
    assert_eq!(act(&mut editor, 'c', 8), Ok(Chose::Jumped));
    assert_eq!(cursors(&editor), [0, 8]);
    assert_eq!(editor.window().selections.primary().cursor, 0);
    let many: Vec<usize> = (0..MAX_CURSORS).collect();
    let mut full = self::editor(&"x".repeat(MAX_CURSORS + 10));
    at(&mut full, &many);
    assert!(full.jump_begin(Kind::LineStart, false));
    assert_eq!(full.jump_choose('c'), Ok(Chose::Armed));
    assert_eq!(
        choose(&mut full, 0, None),
        Err(JumpActionError::Cursor(CursorError::TooMany))
    );
    assert!(full.jump().is_none());
    assert_eq!(full.window().selections.len(), MAX_CURSORS);
}

#[test]
fn an_action_keeps_what_is_typed_of_a_label_and_another_takes_its_place() {
    let mut editor = editor("a1\na2\na3\n");
    editor.set_jump_keys(crate::jump::Keys::new("xy").unwrap());
    assert!(editor.jump_begin(Kind::Char, false));
    editor.jump_add("a");
    editor.jump_show_labels();
    // `x`, then `yx` and `yy`.
    assert_eq!(editor.jump_choose('y'), Ok(Chose::Narrowed));
    assert_eq!(editor.jump_choose('K'), Ok(Chose::Armed));
    assert_eq!(editor.jump_choose('W'), Ok(Chose::Armed));
    assert_eq!(line(&editor), "Jump: a  [copy line]");
    assert_eq!(editor.jump_choose('y'), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "a1\na2\na3\n", "copied, not killed");
    assert_eq!(newest(&editor).as_deref(), Some("a3\n"));
}

#[test]
fn an_armed_action_ends_with_nothing_done_at_a_key_no_label_takes() {
    let mut editor = editor("one\ntwo\n");
    assert!(editor.jump_begin(Kind::LineStart, false));
    assert_eq!(editor.jump_choose('K'), Ok(Chose::Armed));
    assert!(
        !editor.jump().unwrap().gives_goto_line('1'),
        "a digit is no line number now"
    );
    assert_eq!(editor.jump_choose('z'), Ok(Chose::NoSuch));
    assert_eq!(line(&editor), "No such candidate");
    assert_eq!(text(&editor), "one\ntwo\n");
    assert!(editor.kill_ring().is_empty());
    assert!(editor.jump_begin(Kind::LineStart, false));
    assert_eq!(editor.jump_choose('K'), Ok(Chose::Armed));
    assert!(editor.jump_end());
    assert_eq!(text(&editor), "one\ntwo\n");
}

#[test]
fn an_action_in_avy_isearch_ends_the_search_as_ret_then_acts() {
    let mut editor = editor("foo\nbar foo\n");
    editor.isearch_begin(false, false);
    editor.isearch_add("foo");
    let left = cursors(&editor);
    assert!(editor.jump_begin(Kind::Isearch, false));
    assert_eq!(editor.jump_choose('W'), Ok(Chose::Armed));
    assert_eq!(choose(&mut editor, 8, None), Ok(Chose::Jumped));
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), left, "where the search left it");
    assert_eq!(newest(&editor).as_deref(), Some("bar foo\n"));
}

/// An editor of two windows, the selected one over "here\n" and the other, below, over
/// `there`, with the other's id and its buffer's.
fn two(there: Buffer) -> (Editor, WindowId, BufferId) {
    let mut editor = editor("here\n");
    let other = editor.split_window(Way::Down, false).unwrap();
    let first = editor.window().id();
    let id = editor.add_buffer(there);
    editor.select_window(other);
    editor.show(id);
    editor.select_window(first);
    (editor, other, id)
}

#[test]
fn k_and_y_reach_another_window_and_the_selected_one_stays() {
    let (mut editor, other, id) = two(Buffer::from_text("b", "x\ny\n"));
    let first = editor.window().id();
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('K').unwrap();
    assert_eq!(choose(&mut editor, 2, Some(other)), Ok(Chose::Jumped));
    assert_eq!(editor.window().id(), first);
    assert_eq!(text_of(&editor, id), "x\n");
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('Y').unwrap();
    assert_eq!(choose(&mut editor, 0, Some(other)), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "x\nhere\n");
    assert_eq!(text_of(&editor, id), "x\n");
}

#[test]
fn t_across_two_buffers_is_one_undo_step_in_each() {
    let (mut editor, other, id) = two(Buffer::from_text("b", "x\ny\n"));
    as_command(&mut editor, |editor| {
        assert!(editor.jump_begin(Kind::LineStart, false));
        editor.jump_choose('T').unwrap();
        choose(editor, 2, Some(other))
    })
    .unwrap();
    assert_eq!(
        (text(&editor), text_of(&editor, id)),
        ("y\nhere\n".into(), "x\n".into())
    );
    editor.undo().unwrap();
    assert_eq!(text(&editor), "here\n");
    editor.select_window(other);
    editor.undo().unwrap();
    assert_eq!(text(&editor), "x\ny\n");
}

#[test]
fn spc_and_c_in_another_window_select_it() {
    let (mut editor, other, _) = two(Buffer::from_text("b", "x\ny\n"));
    let first = editor.window().id();
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose(' ').unwrap();
    choose(&mut editor, 2, Some(other)).unwrap();
    assert_eq!(editor.window().id(), other);
    let primary = *editor.window().selections.primary();
    assert_eq!((primary.mark, primary.cursor), (Some(0), 2));
    editor.select_window(first);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('c').unwrap();
    choose(&mut editor, 0, Some(other)).unwrap();
    assert_eq!(editor.window().id(), other);
    assert_eq!(editor.window().selections.len(), 2);
}

#[test]
fn a_read_only_buffer_refuses_k_t_and_y_with_nothing_changed() {
    let mut editor = editor("x\n");
    let id = editor.add_internal_buffer("*Report*", Mode::Fundamental);
    editor.set_internal_text(id, "one\ntwo\n");
    editor.show(id);
    let refused = Err(JumpActionError::Edit(EditError::ReadOnly(
        ReadOnly::Internal,
    )));
    for key in ['K', 'T', 'Y'] {
        assert_eq!(act(&mut editor, key, 4), refused, "{key}");
        assert!(editor.jump().is_none());
        assert_eq!(text(&editor), "one\ntwo\n");
        assert!(editor.kill_ring().is_empty(), "{key}");
    }
    assert_eq!(
        act(&mut editor, 'W', 4),
        Ok(Chose::Jumped),
        "a copy is no change"
    );
    assert_eq!(newest(&editor).as_deref(), Some("two\n"));
    assert_eq!(
        JumpActionError::Yank(YankError::TooMuch).to_string(),
        "Too much text to yank"
    );
}

#[test]
fn a_line_from_a_buffer_that_may_hold_secrets_is_secret_and_marks_where_it_goes() {
    let mut secret = secret_buffer();
    secret.apply_load(decode_all(b"token=x\n", TabWidth::default()));
    let (mut editor, other, _) = two(secret);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('W').unwrap();
    choose(&mut editor, 0, Some(other)).unwrap();
    assert!(editor.kill_ring().get(0).unwrap().is_secret());
    assert!(
        editor.take_clipboard_write().unwrap().secret,
        "marked concealed"
    );
    assert!(!editor.current().1.is_secret());
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('Y').unwrap();
    choose(&mut editor, 0, Some(other)).unwrap();
    assert_eq!(text(&editor), "token=x\nhere\n");
    assert!(editor.current().1.is_secret(), "the buffer it went into");
}

#[test]
fn y_and_t_past_what_a_yank_may_insert_at_every_cursor_change_nothing() {
    use crate::kill::MAX_YANK;
    // A line that, with its break, fills a yank at every cursor exactly, then one byte more.
    let fits = MAX_YANK / MAX_CURSORS;
    for (line, refused) in [(fits + 1, true), (fits, false)] {
        let start = line;
        let text = format!("{}\n{}\n", "x".repeat(line - 1), "y".repeat(MAX_CURSORS));
        let mut editor = editor(&text);
        let many: Vec<usize> = (start..start + MAX_CURSORS).collect();
        at(&mut editor, &many);
        for key in ['T', 'Y'] {
            let chose = act(&mut editor, key, 0);
            if !refused {
                assert_eq!(chose, Ok(Chose::Jumped), "{key} at the limit");
                break;
            }
            assert_eq!(
                chose,
                Err(JumpActionError::Yank(YankError::TooMuch)),
                "{key}"
            );
            assert_eq!(text_of(&editor, editor.window().buffer()), text, "{key}");
            assert!(editor.kill_ring().is_empty(), "{key}");
        }
    }
}

#[test]
fn t_into_a_read_only_buffer_kills_nothing_in_the_other_window() {
    let (mut editor, other, id) = two(Buffer::from_text("b", "x\ny\n"));
    let report = editor.add_internal_buffer("*Report*", Mode::Fundamental);
    editor.set_internal_text(report, "r\n");
    editor.show(report);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('T').unwrap();
    assert_eq!(
        choose(&mut editor, 2, Some(other)),
        Err(JumpActionError::Edit(EditError::ReadOnly(
            ReadOnly::Internal
        )))
    );
    assert_eq!(text_of(&editor, id), "x\ny\n", "the source kept its line");
    assert!(editor.kill_ring().is_empty());
    assert!(editor.take_clipboard_write().is_none());
}

#[test]
fn k_in_a_buffer_still_loading_is_refused() {
    let mut loading = Buffer::visiting(std::path::PathBuf::from("/nowhere/notes.txt"));
    assert!(loading.loading().is_some());
    loading.judge_secret(Some(&[std::path::PathBuf::from("/nowhere/notes.txt")]));
    let (mut editor, other, _) = two(loading);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('K').unwrap();
    assert_eq!(
        choose(&mut editor, 0, Some(other)),
        Err(JumpActionError::Edit(EditError::ReadOnly(
            ReadOnly::Loading
        )))
    );
    assert!(editor.kill_ring().is_empty());
}

#[test]
fn t_from_a_buffer_that_may_hold_secrets_is_concealed_and_marks_where_it_goes() {
    let mut secret = secret_buffer();
    secret.apply_load(decode_all(b"token=x\nrest\n", TabWidth::default()));
    let (mut editor, other, id) = two(secret);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('T').unwrap();
    choose(&mut editor, 0, Some(other)).unwrap();
    assert_eq!(text_of(&editor, id), "rest\n");
    assert_eq!(text(&editor), "token=x\nhere\n");
    assert!(
        editor.take_clipboard_write().unwrap().secret,
        "marked concealed"
    );
    assert!(editor.current().1.is_secret(), "the buffer it went into");
}

/// Shows a region from `mark` to `cursor` in the selected window.
fn region(editor: &mut Editor, mark: usize, cursor: usize) {
    let part = Selection {
        cursor,
        mark: Some(mark),
        goal: None,
    };
    let selections = &mut editor.current_mut().0.selections;
    *selections = SelectionSet::from_parts(vec![part], 0).unwrap();
    selections.set_mark_active(MarkActive::Active);
}

#[test]
fn t_replaces_a_region_shown_from_its_own_buffer_and_from_another() {
    // From the buffer the region is in: the kill hides it, and the yank replaces it anyway.
    let mut editor = editor("a\nb\nc\nxyz\n");
    region(&mut editor, 6, 9);
    assert_eq!(act(&mut editor, 'T', 0), Ok(Chose::Jumped));
    assert_eq!(text(&editor), "b\nc\na\n\n");
    // From another buffer.
    let (mut editor, other, id) = two(Buffer::from_text("b", "x\ny\n"));
    region(&mut editor, 0, 4);
    assert!(editor.jump_begin(Kind::LineStart, false));
    editor.jump_choose('T').unwrap();
    assert_eq!(choose(&mut editor, 0, Some(other)), Ok(Chose::Jumped));
    assert_eq!(
        (text(&editor), text_of(&editor, id)),
        ("x\n\n".into(), "y\n".into())
    );
}

#[test]
fn k_and_t_in_avy_isearch_end_the_search_then_act() {
    for key in ['K', 'T'] {
        let mut editor = editor("foo\nbar foo\n");
        editor.isearch_begin(false, false);
        editor.isearch_add("foo");
        let left = cursors(&editor)[0];
        assert!(editor.jump_begin(Kind::Isearch, false));
        assert_eq!(editor.jump_choose(key), Ok(Chose::Armed));
        assert_eq!(choose(&mut editor, 8, None), Ok(Chose::Jumped), "{key}");
        assert!(editor.isearch().is_none(), "{key}");
        let mut expected = String::from("foo\n");
        if key == 'T' {
            expected.insert_str(left, "bar foo\n");
        }
        assert_eq!(text(&editor), expected, "{key}");
        assert_eq!(newest(&editor).as_deref(), Some("bar foo\n"), "{key}");
    }
}

#[test]
fn c_inside_a_region_shown_merges_into_it_and_the_primary_stays() {
    let mut editor = editor("one\ntwo\n");
    region(&mut editor, 0, 7);
    assert_eq!(act(&mut editor, 'c', 4), Ok(Chose::Jumped));
    let set = &editor.window().selections;
    assert_eq!(set.len(), 1, "merged into the region");
    assert!(set.mark_active().is_active());
    assert_eq!((set.primary().mark, set.primary().cursor), (Some(0), 7));
}
