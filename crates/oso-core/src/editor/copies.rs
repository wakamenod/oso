//! The buffers that show emergency copies, and putting a copy's text in a buffer (ADR 0064).
//! The way back from them is the one in `messages`, which all internal buffers share.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O. The host
//! reads the directory and the copies, and holds what it found. A command only queues what
//! a person answered, as it queues a save.

use super::Editor;
use crate::buffer::{Buffer, BufferId, EditError, Mode};
use crate::load::LoadSnapshot;
use crate::selection::SelectionSet;

/// The name of the internal buffer that lists the emergency copies.
const COPIES: &str = "*Emergency Copies*";

/// What a person asked of the emergency copies, for the host to do. A number is a line of
/// `*Emergency Copies*`, and means nothing in a buffer showing one copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Show the text.
    View(usize),
    /// Show how it differs from its file.
    Compare(usize),
    /// Show the text, or if it is shown, put it in its file's buffer.
    Recover(usize),
    /// A first `d`: forget what earlier ones marked.
    ForgetMarks,
    /// A first `d`: mark it, to say what a second one does.
    MarkForDelete(usize),
    /// A second `d`: delete it for good, if it is what the first one marked.
    Delete(usize),
    /// Read the directory again.
    Refresh,
    /// The list was opened or closed.
    Toggled,
    /// The window went back from a copy.
    WentBack,
}

impl Editor {
    /// Queues what a person asked of the emergency copies, for the host.
    pub fn queue_recovery_action(&mut self, action: RecoveryAction) {
        self.copy_actions.push(action);
    }

    /// What a person asked of the emergency copies since the last call.
    pub fn take_recovery_actions(&mut self) -> Vec<RecoveryAction> {
        std::mem::take(&mut self.copy_actions)
    }

    /// The list of copies, made the first time it is asked for.
    fn copies_buffer(&mut self) -> BufferId {
        if let Some(id) = self.copies_buffer {
            return id;
        }
        let id = self.add_internal_buffer(COPIES, Mode::CopyList);
        self.copies_buffer = Some(id);
        id
    }

    /// Whether the list of copies has been asked for yet. Until then a host need not fill it.
    pub fn has_copies_list(&self) -> bool {
        self.copies_buffer.is_some()
    }

    /// Adds an internal buffer, shown in no window yet.
    pub fn add_internal_buffer(&mut self, name: impl Into<String>, mode: Mode) -> BufferId {
        let mut buffer = Buffer::internal(name);
        buffer.set_mode(mode);
        self.add_buffer(buffer)
    }

    /// Takes an internal buffer out, unless the editor needs it, and hands it over so that
    /// the host can free a large text off the UI thread. A window showing it goes to the list.
    pub fn remove_internal_buffer(&mut self, id: BufferId) -> Option<Buffer> {
        let removable = self.buffer(id).is_some_and(Buffer::is_internal)
            && id != self.messages_buffer
            && Some(id) != self.copies_buffer;
        if !removable {
            return None;
        }
        if self.windows.iter().any(|window| window.buffer() == id) {
            let list = self.copies_buffer();
            self.window_moves_on();
            for index in 0..self.windows.len() {
                if self.windows[index].buffer() == id {
                    self.window_shows(index, list);
                }
            }
        }
        self.way_back.retain(|place| place.buffer != id);
        let position = self.buffers.iter().position(|(other, _)| *other == id)?;
        Some(self.buffers.remove(position).1)
    }

    /// Shows the list of copies in the selected window. From any buffer showing copies, goes
    /// back to where the window was. Does nothing while a prompt is open, as a prompt's
    /// window always shows its own buffer (ADR 0072).
    pub fn toggle_copies(&mut self) {
        if !self.prompts.is_empty() {
            return;
        }
        if self.current().1.mode().shows_copies() {
            self.leave_internal();
            return;
        }
        let list = self.copies_buffer();
        self.go_to_internal(list);
    }

    /// From a buffer showing one copy, goes to the list, and from the list, to where the
    /// window was.
    pub fn copies_back(&mut self) {
        match self.current().1.mode() {
            Mode::Fundamental => {}
            Mode::CopyList => self.leave_internal(),
            Mode::CopyView => {
                let list = self.copies_buffer();
                self.show(list);
            }
        }
    }

    /// Goes all the way back to where the window was before any internal buffer.
    pub fn leave_all_internal(&mut self) {
        // The first place is the user's, as showing one of theirs empties the way. If that buffer
        // has gone, this goes to `*scratch*`.
        let first = self.way_back.drain(..).next();
        self.go_back(first);
    }

    /// Fills the list of copies with `snapshot`, which holds its rows, with the cursor of each
    /// window that shows it at the start of `line`, and that of a window that will come back to
    /// it at the start of `saved_line`.
    pub fn set_copies_text(
        &mut self,
        snapshot: LoadSnapshot,
        line: usize,
        saved_line: Option<usize>,
    ) {
        let list = self.copies_buffer();
        let Some(buffer) = self.buffer_mut(list) else {
            return;
        };
        buffer.apply_load(snapshot);
        if let Some(saved_line) = saved_line {
            let start = buffer.text().line_start(saved_line);
            for place in &mut self.way_back {
                if place.buffer == list {
                    place.selections = SelectionSet::new(start);
                }
            }
        }
        self.set_copies_cursor(line);
    }

    /// The line of the list a window will come back to, if it has gone on from there. The rows
    /// can change behind it, and it must come back to the same copy.
    pub fn saved_copies_line(&self) -> Option<usize> {
        let list = self.copies_buffer?;
        let place = self.way_back.iter().find(|place| place.buffer == list)?;
        let cursor = place.selections.primary().cursor;
        Some(self.buffer(list)?.text().offset_to_point(cursor).row)
    }

    /// Puts the cursor of each window that shows the list of copies at the start of `line`.
    pub fn set_copies_cursor(&mut self, line: usize) {
        let Some(list) = self.copies_buffer else {
            return;
        };
        let Some(buffer) = self.buffer(list) else {
            return;
        };
        let start = buffer.text().line_start(line);
        // The tiled window shows the list, also beneath a prompt.
        let tiled = self.tiled_index();
        let shows_it = self.windows[tiled].buffer() == list;
        for window in &mut self.windows {
            if window.buffer() == list {
                window.selections = SelectionSet::new(start);
            }
        }
        if shows_it {
            self.follow_window(tiled);
        }
    }

    /// The buffer line the selected window's cursor is on, by which a host tells which row of
    /// the list it is on. A buffer line and not a display row, on purpose: a row of the list
    /// that wraps still means one copy.
    pub fn cursor_line(&self) -> usize {
        // The window beneath a prompt, which shows the list, also while a prompt is open.
        let (window, buffer) = self.tiled();
        let cursor = window.selections.primary().cursor;
        buffer.text().offset_to_point(cursor).row
    }

    /// Replaces the text of `target` with that of `source`, a copy that has finished loading,
    /// as one undo step, in O(1), with the cursors of each window showing it at the start.
    ///
    /// # Errors
    ///
    /// [`EditError::ReadOnly`] for a read-only buffer, and [`EditError::NoSuchBuffer`].
    /// Nothing changes.
    pub fn replace_text_from(
        &mut self,
        target: BufferId,
        source: BufferId,
    ) -> Result<(), EditError> {
        let from = self.buffer(source).ok_or(EditError::NoSuchBuffer)?;
        let buffer = self.buffer(target).ok_or(EditError::NoSuchBuffer)?;
        let staged = buffer.stage_replacement(from)?;
        let verdict = from.verdict();
        let opening = self.opening(target, buffer);
        let start = SelectionSet::new(0);
        let after = (self.window().buffer() == target).then(|| start.clone());

        // Nothing past this point can panic part way.
        if let Some(buffer) = self.buffer_mut(target) {
            buffer.commit_recovery(staged, opening, after, verdict);
        }
        for window in &mut self.windows {
            if window.buffer() == target {
                window.selections = start.clone();
            }
        }
        self.note_edited(target);
        self.follow_cursor();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Replacement, Verdict};
    use crate::display_map::TabWidth;
    use crate::editor::MESSAGES;
    use crate::load::decode_all;
    use crate::window::{Scroll, ViewSize};
    use std::path::PathBuf;

    /// An editor showing a file holding `text`, and an internal buffer holding `copy`.
    fn with_copy(text: &str, copy: &str) -> (Editor, BufferId, BufferId) {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", text));
        editor.set_view_size(ViewSize::new(10, 40));
        let file = editor.window().buffer();
        let view = editor.add_internal_buffer("*copy of notes.txt*", Mode::CopyView);
        let snapshot = decode_all(copy.as_bytes(), TabWidth::default());
        editor.buffer_mut(view).unwrap().apply_load(snapshot);
        (editor, file, view)
    }

    fn text(editor: &Editor, id: BufferId) -> String {
        editor.buffer(id).unwrap().text().to_string()
    }

    fn type_text(editor: &mut Editor, id: BufferId, text: &str) {
        let edit = Replacement { range: 0..0, text };
        editor.begin_undo_group();
        editor.apply_edits(id, &[edit]).unwrap();
        editor.end_undo_group();
    }

    #[test]
    fn a_recovery_is_one_undo_step_and_leaves_the_buffer_modified() {
        let (mut editor, file, view) = with_copy("on disk\n", "the copy\nof the text\n");
        editor.current_mut().0.selections = SelectionSet::new(3);
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        assert_eq!(text(&editor, file), "the copy\nof the text\n");
        assert!(editor.buffer(file).unwrap().is_modified());
        assert_eq!(editor.window().selections.primary().cursor, 0);
        assert_eq!(
            editor.buffer(file).unwrap().display().row_count(),
            3,
            "the display map came with the text"
        );

        editor.undo().unwrap();
        assert_eq!(text(&editor, file), "on disk\n");
        assert!(!editor.buffer(file).unwrap().is_modified());
        assert_eq!(editor.window().selections.primary().cursor, 3);

        editor.redo().unwrap();
        assert_eq!(text(&editor, file), "the copy\nof the text\n");
        assert!(editor.buffer(file).unwrap().is_modified());
    }

    #[test]
    fn a_recovery_leaves_the_copy_as_it_was() {
        let (mut editor, file, view) = with_copy("", "the copy\n");
        let revision = editor.buffer(view).unwrap().revision();
        editor.replace_text_from(file, view).unwrap();
        assert_eq!(text(&editor, view), "the copy\n");
        assert_eq!(editor.buffer(view).unwrap().revision(), revision);
    }

    #[test]
    fn an_empty_buffer_recovers_and_undoes_to_empty() {
        let (mut editor, file, view) = with_copy("", "the copy\n");
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        assert_eq!(text(&editor, file), "the copy\n");
        editor.undo().unwrap();
        assert_eq!(text(&editor, file), "");
    }

    #[test]
    fn a_read_only_buffer_refuses_and_nothing_changes() {
        let (mut editor, _, view) = with_copy("", "the copy\n");
        let binary = editor.add_buffer(Buffer::from_bytes("b", b"\0\0"));
        assert_eq!(
            editor.replace_text_from(binary, view),
            Err(EditError::ReadOnly(crate::buffer::ReadOnly::Binary))
        );
        assert_eq!(text(&editor, binary), "\0\0");
        let missing = BufferId(9999);
        assert_eq!(
            editor.replace_text_from(missing, view),
            Err(EditError::NoSuchBuffer)
        );
    }

    #[test]
    fn undo_withdraws_a_recovery_and_redo_brings_it_back_and_edits_do_not_touch_it() {
        let (mut editor, file, view) = with_copy("on disk\n", "the copy\n");
        assert!(!editor.buffer(file).unwrap().recovery_stands());
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        assert!(editor.buffer(file).unwrap().recovery_stands());

        type_text(&mut editor, file, "more ");
        assert!(editor.buffer(file).unwrap().recovery_stands());
        editor.undo().unwrap();
        assert!(editor.buffer(file).unwrap().recovery_stands(), "the edit");
        editor.undo().unwrap();
        assert!(!editor.buffer(file).unwrap().recovery_stands(), "itself");
        type_text(&mut editor, file, "other ");
        assert!(
            !editor.buffer(file).unwrap().recovery_stands(),
            "typing after an undo does not bring it back"
        );
    }

    #[test]
    fn undo_and_redo_keep_a_secret_kill_yanked_into_a_recovery_withheld() {
        let (mut editor, file, view) = with_copy("on disk\n", "the copy\n");
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        editor.show(file);
        let piece = crate::kill::Piece::from(crate::rope::Rope::from("KEY"));
        editor.kills.ring.push(vec![piece], true);
        editor.begin_undo_group();
        editor.yank(0, false).unwrap();
        editor.end_undo_group();
        assert_eq!(editor.buffer(file).unwrap().verdict(), Verdict::Yanked);
        editor.undo().unwrap();
        assert!(editor.buffer(file).unwrap().recovery_stands());
        assert_eq!(
            editor.buffer(file).unwrap().verdict(),
            Verdict::Yanked,
            "back where the recovery left it"
        );
        editor.redo().unwrap();
        assert_eq!(editor.buffer(file).unwrap().verdict(), Verdict::Yanked);
    }

    #[test]
    fn redo_brings_a_recovery_back() {
        let (mut editor, file, view) = with_copy("on disk\n", "the copy\n");
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        editor.undo().unwrap();
        editor.redo().unwrap();
        assert!(editor.buffer(file).unwrap().recovery_stands());
        editor.buffer_mut(file).unwrap().forget_recovery();
        assert!(!editor.buffer(file).unwrap().recovery_stands());
    }

    #[test]
    fn the_list_is_made_when_first_asked_for_and_is_nobodys_work() {
        let mut editor = Editor::default();
        let before = editor.buffers().count();
        editor.toggle_copies();
        assert_eq!(editor.current().1.name(), COPIES);
        assert_eq!(editor.current().1.mode(), Mode::CopyList);
        assert!(editor.current().1.is_internal());
        assert_eq!(editor.buffers().count(), before);
        editor.toggle_copies();
        assert_eq!(editor.current().1.mode(), Mode::Fundamental);
    }

    #[test]
    fn it_goes_back_through_a_view_and_the_list_to_where_the_window_was() {
        let (mut editor, file, view) = with_copy("abcdef", "x");
        editor.current_mut().0.selections = SelectionSet::new(3);
        editor.toggle_copies();
        let list = editor.window().buffer();
        editor.show(view);
        editor.copies_back();
        assert_eq!(editor.window().buffer(), list);
        editor.copies_back();
        assert_eq!(editor.window().buffer(), file);
        assert_eq!(editor.window().selections.primary().cursor, 3);
        editor.copies_back();
        assert_eq!(editor.window().buffer(), file, "nothing to go back from");
    }

    #[test]
    fn the_key_for_the_list_also_leaves_a_view() {
        let (mut editor, file, view) = with_copy("abc", "x");
        editor.toggle_copies();
        editor.show(view);
        editor.toggle_copies();
        assert_eq!(editor.window().buffer(), file);
    }

    #[test]
    fn it_goes_back_to_scratch_if_the_buffer_has_gone() {
        let (mut editor, file, _) = with_copy("abc", "x");
        editor.toggle_copies();
        editor.kill_buffer(file);
        assert_eq!(editor.place_behind_internal(), None);
        editor.toggle_copies();
        assert_eq!(editor.current().1.name(), crate::editor::SCRATCH);
    }

    #[test]
    fn a_handoff_finds_the_real_place_behind_the_list_and_messages_either_way() {
        let (mut editor, file, _) = with_copy("abcdef", "x");
        editor.current_mut().0.selections = SelectionSet::new(3);
        assert_eq!(
            editor.place_behind_internal(),
            None,
            "on no internal buffer"
        );
        let cursor = SelectionSet::new(3);
        editor.toggle_copies();
        assert_eq!(
            editor.place_behind_internal(),
            Some((file, &cursor, Scroll::default()))
        );
        editor.toggle_messages();
        assert_eq!(
            editor.place_behind_internal(),
            Some((file, &cursor, Scroll::default())),
            "through the list"
        );
        editor.toggle_copies();
        assert_eq!(
            editor.current().1.mode(),
            Mode::CopyList,
            "back to the list"
        );
        assert_eq!(
            editor.place_behind_internal(),
            Some((file, &cursor, Scroll::default())),
        );
    }

    #[test]
    fn going_between_the_list_and_messages_any_way_never_loses_the_file() {
        let (mut editor, file, _) = with_copy("abc", "x");
        editor.toggle_copies();
        editor.toggle_messages();
        editor.toggle_copies();
        assert_eq!(editor.current().1.mode(), Mode::CopyList);
        editor.toggle_messages();
        assert_eq!(editor.current().1.name(), MESSAGES);
        editor.toggle_messages();
        assert_eq!(editor.current().1.mode(), Mode::CopyList);
        editor.toggle_copies();
        assert_eq!(editor.window().buffer(), file);
        assert_eq!(editor.place_behind_internal(), None);

        editor.toggle_messages();
        editor.toggle_copies();
        editor.copies_back();
        assert_eq!(editor.current().1.name(), MESSAGES);
        editor.toggle_messages();
        assert_eq!(editor.window().buffer(), file);
    }

    #[test]
    fn a_view_taken_out_on_the_way_does_not_cost_the_way_back() {
        let (mut editor, file, view) = with_copy("abc", "x");
        editor.toggle_copies();
        editor.show(view);
        editor.toggle_messages();
        editor.toggle_copies();
        assert_eq!(editor.current().1.mode(), Mode::CopyList);
        editor.toggle_copies();
        editor.remove_internal_buffer(view);
        editor.toggle_messages();
        assert_eq!(editor.window().buffer(), file, "not *scratch*");
    }

    #[test]
    fn a_window_moved_to_a_buffer_of_the_users_has_no_way_back_left_to_go_stale() {
        let (mut editor, _, _) = with_copy("abc", "x");
        let other = editor.add_buffer(Buffer::from_text("other", "y"));
        editor.toggle_copies();
        editor.show(other);
        assert!(editor.way_back.is_empty());
        assert_eq!(editor.place_behind_internal(), None);
        editor.toggle_copies();
        let place = editor.place_behind_internal().map(|(buffer, _, _)| buffer);
        assert_eq!(place, Some(other), "where it is now, not where it was");
    }

    #[test]
    fn however_often_it_goes_between_them_the_way_back_stays_short() {
        let (mut editor, _, _) = with_copy("abc", "x");
        for _ in 0..50 {
            editor.toggle_copies();
            editor.toggle_messages();
        }
        assert!(editor.way_back.len() <= 2, "{}", editor.way_back.len());
    }

    #[test]
    fn a_view_can_be_removed_and_the_list_and_messages_cannot() {
        let (mut editor, _, view) = with_copy("abc", "x");
        editor.toggle_copies();
        let list = editor.window().buffer();
        editor.show(view);
        assert!(editor.remove_internal_buffer(view).is_some());
        assert_eq!(
            editor.window().buffer(),
            list,
            "the window moved to the list"
        );
        assert!(editor.buffer(view).is_none());
        assert!(editor.remove_internal_buffer(list).is_none());
        let messages = editor.messages_buffer();
        assert!(editor.remove_internal_buffer(messages).is_none());
    }

    #[test]
    fn the_list_takes_its_rows_and_puts_the_cursor_on_a_line() {
        let mut editor = Editor::default();
        editor.toggle_copies();
        let snapshot = decode_all(b"header\n\nrow\nrow\n", TabWidth::default());
        editor.set_copies_text(snapshot, 3, None);
        assert_eq!(editor.cursor_line(), 3);
        editor.set_copies_cursor(2);
        assert_eq!(editor.cursor_line(), 2);
        assert!(!editor.current().1.is_modified());
    }

    #[test]
    fn a_window_that_went_on_from_the_list_comes_back_to_the_line_it_is_told() {
        let mut editor = Editor::default();
        editor.toggle_copies();
        let rows = |text: &str| decode_all(text.as_bytes(), TabWidth::default());
        editor.set_copies_text(rows("h\n\none\ntwo\n"), 3, None);
        editor.toggle_messages();
        assert_eq!(editor.saved_copies_line(), Some(3));
        editor.set_copies_text(rows("h\n\nnew\none\ntwo\n"), 0, Some(4));
        assert_eq!(editor.saved_copies_line(), Some(4));
        editor.toggle_messages();
        assert_eq!(editor.cursor_line(), 4);
        assert_eq!(editor.saved_copies_line(), None);
    }

    #[test]
    fn going_all_the_way_back_passes_every_internal_buffer_and_what_has_gone() {
        let (mut editor, file, view) = with_copy("abcdef", "x");
        editor.current_mut().0.selections = SelectionSet::new(3);
        editor.toggle_copies();
        editor.toggle_messages();
        editor.show(view);
        editor.leave_all_internal();
        assert_eq!(editor.window().buffer(), file);
        assert_eq!(editor.window().selections.primary().cursor, 3);
        assert!(editor.way_back.is_empty());
    }

    #[test]
    fn actions_wait_for_the_host_in_order() {
        let mut editor = Editor::default();
        editor.queue_recovery_action(RecoveryAction::Refresh);
        editor.queue_recovery_action(RecoveryAction::Delete(3));
        assert_eq!(
            editor.take_recovery_actions(),
            [RecoveryAction::Refresh, RecoveryAction::Delete(3)]
        );
        assert_eq!(editor.take_recovery_actions(), []);
    }

    #[test]
    fn a_copy_judged_by_its_paths_is_withheld_if_any_says_so_or_none_is_known() {
        let mut buffer = Buffer::internal("c");
        assert_eq!(buffer.verdict(), Verdict::Ordinary);
        let judge = |buffer: &mut Buffer, path: &str, at_least| {
            buffer.judge_copy(&[PathBuf::from(path)], at_least);
            buffer.verdict()
        };
        assert_eq!(
            judge(&mut buffer, "/home/me/notes.txt", Verdict::Ordinary),
            Verdict::Ordinary
        );
        assert_eq!(
            judge(&mut buffer, "/home/me/.env", Verdict::Ordinary),
            Verdict::SecretName
        );
        assert_eq!(
            judge(&mut buffer, "/home/me/notes.txt", Verdict::SecretName),
            Verdict::SecretName,
            "its file's verdict counts"
        );
        assert_eq!(
            judge(&mut buffer, "/home/me/notes.txt", Verdict::Unresolved),
            Verdict::Unresolved
        );
        assert_eq!(
            judge(&mut buffer, "/home/me/notes.txt", Verdict::Unjudged),
            Verdict::Unresolved,
            "a file not yet judged tells nothing"
        );
        buffer.judge_copy(&[], Verdict::Ordinary);
        assert_eq!(buffer.verdict(), Verdict::Ordinary, "a copy of *scratch*");
        buffer.judge_copy(&[], Verdict::Unresolved);
        assert_eq!(
            buffer.verdict(),
            Verdict::Unresolved,
            "a copy nothing says of"
        );
    }

    #[test]
    fn a_copy_still_loading_cannot_be_recovered() {
        let (mut editor, file, view) = with_copy("on disk\n", "the copy\n");
        editor.buffer_mut(view).unwrap().begin_loading();
        assert_eq!(
            editor.replace_text_from(file, view),
            Err(EditError::ReadOnly(crate::buffer::ReadOnly::Loading))
        );
        assert_eq!(text(&editor, file), "on disk\n");
    }

    #[test]
    fn undoing_a_recovery_takes_back_what_a_save_would_write_and_expect() {
        let path = PathBuf::from("/tmp/notes.txt");
        let mut buffer = Buffer::visiting(path.clone());
        buffer.finish_empty();
        buffer.judge_secret(Some(std::slice::from_ref(&path)));
        buffer.set_baseline(crate::buffer::Baseline {
            location: path,
            file: None,
        });
        let mut editor = Editor::with_buffer(buffer);
        let file = editor.window().buffer();
        let view = editor.add_internal_buffer("v", Mode::CopyView);
        let snapshot = decode_all(b"text\r\n", TabWidth::default());
        let shown = editor.buffer_mut(view).unwrap();
        shown.apply_load(snapshot);
        shown.judge_copy(&[], Verdict::Unresolved);
        editor.begin_undo_group();
        editor.replace_text_from(file, view).unwrap();
        editor.end_undo_group();
        let shown = editor.buffer_mut(file).unwrap();
        shown.take_format_of_copy(true, crate::decode::LineEnding::Crlf, true);
        let stamp = shown.baseline().unwrap().file.unwrap();
        assert_eq!(stamp.mode, 0o600);
        assert!(shown.file_info().bom);
        assert_eq!(
            shown.verdict(),
            Verdict::Unresolved,
            "as the copy was withheld"
        );

        editor.undo().unwrap();
        let shown = editor.buffer(file).unwrap();
        assert_eq!(shown.baseline().unwrap().file, None, "there never was one");
        assert!(!shown.file_info().bom);
        assert_eq!(shown.file_info().line_ending, crate::decode::LineEnding::Lf);
        assert_eq!(
            shown.verdict(),
            Verdict::Ordinary,
            "the copy's text has gone"
        );

        editor.redo().unwrap();
        let shown = editor.buffer(file).unwrap();
        assert_eq!(shown.baseline().unwrap().file, Some(stamp));
        assert!(shown.file_info().bom);
        assert_eq!(shown.verdict(), Verdict::Unresolved);
    }
}
