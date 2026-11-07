//! What comes of each answer, with real files and no window (ADR 0064). A [`CopyWork`] is done
//! here at once, as its thread would.
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::SystemTime;

use oso_core::buffer::{Baseline, Buffer, BufferId, Mode, Verdict};
use oso_core::decode::{BOM, LineEnding};
use oso_core::display_map::TabWidth;
use oso_core::editor::{Editor, RecoveryAction};
use oso_core::load::decode_all;

use super::super::copies::{self, HEADER_ROWS, LEFT_BEHIND, RowState};
use super::super::tests::Scratch;
use super::super::{Identity, Why, write};
use super::*;
use crate::commands::kept_messages;
use crate::loader::stamp_of;

const STEM: &str = "00000000000000aa-1790000000-77-0";

/// An editor on a file, an offer of what is in the directory, and the two paths involved.
struct Fixture {
    scratch: Scratch,
    directory: PathBuf,
    file: PathBuf,
    editor: Editor,
    offer: Offer,
}

impl Fixture {
    /// `notes.txt` holding `on_disk`, with a copy holding `copy` that records the file as it
    /// is now, read by a buffer, and the offer told.
    fn new(on_disk: &str, copy: &str) -> Self {
        let scratch = Scratch::new();
        let directory = oso_platform::privacy::PrivateDir::open(scratch.copies())
            .unwrap()
            .path()
            .to_owned();
        let file = scratch.0.join("notes.txt");
        fs::write(&file, on_disk).unwrap();
        let stamp = stamp_of(&fs::metadata(&file).unwrap()).unwrap();
        let mut buffer = Buffer::visiting(file.clone());
        buffer.apply_load(decode_all(on_disk.as_bytes(), TabWidth::default()));
        buffer.judge_secret(Some(std::slice::from_ref(&file)));
        buffer.set_baseline(Baseline {
            location: file.clone(),
            file: Some(stamp),
        });
        let editor = Editor::with_buffer(buffer);
        let fixture = Self {
            offer: Offer::new(Some(directory.clone())),
            scratch,
            directory,
            file,
            editor,
        };
        fixture.copy(STEM, copy, Some(stamp));
        fixture
    }

    fn copy(&self, stem: &str, text: &str, stamp: Option<oso_core::buffer::FileStamp>) {
        let identity = Identity {
            name: "notes.txt".to_owned(),
            path: Some(self.file.clone()),
            location: Some(self.file.clone()),
            withheld: false,
            bom: true,
            line_ending: LineEnding::Crlf,
        };
        let meta = write::meta_text(&identity, stamp, text.len(), Why::Panic, 1_790_000_000);
        fs::write(self.directory.join(format!("{stem}.meta")), meta).unwrap();
        fs::write(self.directory.join(format!("{stem}.txt")), text).unwrap();
    }

    fn buffer(&self) -> BufferId {
        self.editor
            .buffers()
            .find(|(_, buffer)| buffer.path().is_some())
            .unwrap()
            .0
    }

    /// Scans now, and takes the result in, as if the event had come.
    fn scan(&mut self) {
        let request = self.offer.scan_request(&self.editor);
        let scan = copies::scan(&request, SystemTime::now() + 2 * LEFT_BEHIND);
        let more = self.offer.take_scan(&mut self.editor, scan, false);
        self.run(more);
    }

    /// Does `work` here, and everything that leads to.
    fn run(&mut self, work: Vec<CopyWork>) {
        for work in work {
            match work {
                CopyWork::Scan(request) => {
                    let scan = copies::scan(&request, SystemTime::now() + 2 * LEFT_BEHIND);
                    let more = self.offer.take_scan(&mut self.editor, scan, false);
                    self.run(more);
                }
                CopyWork::Load {
                    buffer,
                    path,
                    limit,
                } => {
                    let mut bytes = Vec::new();
                    let file = crate::emergency::open_copy(&path).unwrap();
                    file.take(limit).read_to_end(&mut bytes).unwrap();
                    let snapshot = decode_all(&bytes, TabWidth::default());
                    self.editor.buffer_mut(buffer).unwrap().apply_load(snapshot);
                }
                CopyWork::Compare(comparing) => {
                    let event = CopiesEvent::Compared(Box::new(comparing.run()));
                    let more = self.offer.take_event(&mut self.editor, event, false);
                    self.run(more);
                }
                CopyWork::Delete(deleting) => {
                    let event = deleting.run();
                    let more = self.offer.take_event(&mut self.editor, event, false);
                    self.run(more);
                }
                CopyWork::Free(buffer) => drop(buffer),
            }
        }
    }

    /// Answers, and does all that leads to.
    fn answer(&mut self, action: RecoveryAction) {
        let work = self.offer.answer(&mut self.editor, action);
        self.run(work);
    }

    /// `d d` on `line`.
    fn delete(&mut self, line: usize) {
        self.answer(RecoveryAction::ForgetMarks);
        self.answer(RecoveryAction::MarkForDelete(line));
        self.answer(RecoveryAction::Delete(line));
    }

    /// `C-h r`.
    fn open_list(&mut self) {
        self.editor.toggle_copies();
        self.answer(RecoveryAction::Toggled);
    }

    fn line(&self) -> usize {
        self.editor.cursor_line()
    }

    fn file_text(&self) -> String {
        let id = self.buffer();
        self.editor.buffer(id).unwrap().text().to_string()
    }

    fn state(&self) -> RowState {
        self.offer.rows[0].state
    }

    fn last_message(&self) -> String {
        kept_messages(&self.editor).pop().unwrap_or_default()
    }

    fn copy_is_on_disk(&self) -> bool {
        self.directory.join(format!("{STEM}.txt")).exists()
    }
}

fn shown(editor: &Editor) -> &Buffer {
    editor.current().1
}

#[test]
fn a_copy_of_the_file_is_told_once_and_warns_until_it_is_answered() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    assert!(
        fixture
            .last_message()
            .starts_with("notes.txt has an emergency copy, from"),
        "{}",
        fixture.last_message()
    );
    assert!(fixture.last_message().ends_with(": C-h r"));
    let id = fixture.buffer();
    let mut status = oso_core::editor::Status::default();
    fixture.editor.status_into(&mut status);
    assert_eq!(status.warnings, "[emergency copy] ");
    let said = kept_messages(&fixture.editor).len();
    fixture.scan();
    assert_eq!(kept_messages(&fixture.editor).len(), said, "once");
    assert!(fixture.editor.buffer(id).is_some());
}

#[test]
fn others_are_counted_and_a_later_one_is_told_again() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let other = fixture.scratch.0.join("other.txt");
    let identity = Identity {
        name: "other.txt".to_owned(),
        path: Some(other.clone()),
        location: Some(other),
        withheld: false,
        bom: false,
        line_ending: LineEnding::Lf,
    };
    let write_other = |stem: &str| {
        let meta = write::meta_text(&identity, None, 0, Why::Exit, 1_790_000_000);
        fs::write(fixture.directory.join(format!("{stem}.meta")), meta).unwrap();
        fs::write(fixture.directory.join(format!("{stem}.txt")), "").unwrap();
    };
    write_other("00000000000000bb-1790000000-77-0");
    fixture.scan();
    assert_eq!(fixture.last_message(), "1 emergency copy waits: C-h r");
    let write_other2 = |stem: &str| {
        let meta = write::meta_text(&identity, None, 0, Why::Exit, 1_790_000_000);
        fs::write(fixture.directory.join(format!("{stem}.meta")), meta).unwrap();
        fs::write(fixture.directory.join(format!("{stem}.txt")), "").unwrap();
    };
    write_other2("00000000000000cc-1790000000-77-0");
    fixture.scan();
    assert_eq!(fixture.last_message(), "1 emergency copy waits: C-h r");
    assert_eq!(kept_messages(&fixture.editor).len(), 3);
}

#[test]
fn while_a_file_loads_nothing_is_told_as_more_may_come_at_once() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let request = fixture.offer.scan_request(&fixture.editor);
    let scan = copies::scan(&request, SystemTime::now());
    let _ = fixture.offer.take_scan(&mut fixture.editor, scan, true);
    assert!(kept_messages(&fixture.editor).is_empty());
    fixture.scan();
    assert_eq!(kept_messages(&fixture.editor).len(), 1);
}

#[test]
fn an_older_scan_ending_late_changes_nothing() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let early = fixture.offer.scan_request(&fixture.editor);
    let early = copies::scan(&early, SystemTime::now());
    fixture.scan();
    let rows = fixture.offer.rows.len();
    fs::remove_file(fixture.directory.join(format!("{STEM}.meta"))).unwrap();
    let _ = fixture.offer.take_scan(&mut fixture.editor, early, false);
    assert_eq!(fixture.offer.rows.len(), rows);
}

#[test]
fn the_list_opens_on_the_row_of_the_file_and_goes_back_with_q() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    assert_eq!(shown(&fixture.editor).mode(), Mode::CopyList);
    assert_eq!(fixture.line(), HEADER_ROWS);
    assert!(
        shown(&fixture.editor)
            .text()
            .to_string()
            .contains("file unchanged")
    );
    fixture.editor.copies_back();
    fixture.answer(RecoveryAction::WentBack);
    assert_eq!(shown(&fixture.editor).path(), Some(fixture.file.as_path()));
}

#[test]
fn looking_shows_the_text_read_only_and_leaves_the_file_and_the_buffer_alone() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    let view = shown(&fixture.editor);
    assert_eq!(view.mode(), Mode::CopyView);
    assert_eq!(view.name(), "*copy of notes.txt*");
    assert_eq!(view.text().to_string(), "the copy\n");
    assert!(view.read_only().is_some());
    assert_eq!(fixture.file_text(), "on disk\n");
    assert_eq!(fs::read(&fixture.file).unwrap(), b"on disk\n");
    assert!(fixture.copy_is_on_disk());

    fixture.editor.copies_back();
    fixture.answer(RecoveryAction::WentBack);
    assert_eq!(shown(&fixture.editor).mode(), Mode::CopyList);
    assert_eq!(fixture.line(), HEADER_ROWS, "back on its row");
}

#[test]
fn a_line_of_any_other_buffer_means_no_copy_as_a_command_can_run_anywhere() {
    let mut fixture = Fixture::new("one\ntwo\nthree\n", "the copy\n");
    fixture.scan();
    for action in [
        RecoveryAction::View(HEADER_ROWS),
        RecoveryAction::Recover(HEADER_ROWS),
        RecoveryAction::Compare(HEADER_ROWS),
        RecoveryAction::Delete(HEADER_ROWS),
    ] {
        fixture.answer(action);
        assert_eq!(fixture.last_message(), "No emergency copy on this row");
    }
    assert!(fixture.copy_is_on_disk());
    assert_eq!(fixture.file_text(), "one\ntwo\nthree\n");
}

#[test]
fn no_row_says_so_and_does_nothing() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    for action in [
        RecoveryAction::View(0),
        RecoveryAction::Recover(1),
        RecoveryAction::Compare(99),
        RecoveryAction::Delete(99),
        RecoveryAction::MarkForDelete(99),
    ] {
        fixture.answer(action);
        assert_eq!(fixture.last_message(), "No emergency copy on this row");
    }
    assert!(fixture.copy_is_on_disk());
}

#[test]
fn the_first_r_looks_and_says_so_and_the_second_recovers_and_nothing_is_written() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).name(), "*copy of notes.txt*");
    assert_eq!(fixture.file_text(), "on disk\n");
    assert_eq!(
        fixture.last_message(),
        "Press r again to put this text in notes.txt"
    );

    fixture.answer(RecoveryAction::Recover(0));
    assert_eq!(fixture.file_text(), "the copy\n");
    assert_eq!(shown(&fixture.editor).path(), Some(fixture.file.as_path()));
    assert!(shown(&fixture.editor).is_modified());
    assert_eq!(fixture.editor.cursor_line(), 0);
    assert!(fixture.last_message().starts_with("Recovered notes.txt."));
    assert_eq!(fs::read(&fixture.file).unwrap(), b"on disk\n", "untouched");
    assert!(fixture.copy_is_on_disk(), "until it is saved");
    assert_eq!(fixture.state(), RowState::Recovered);
    let mut status = oso_core::editor::Status::default();
    fixture.editor.status_into(&mut status);
    assert_eq!(status.warnings, "", "it has been answered");
}

#[test]
fn a_view_leaves_the_editor_with_the_recovery() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    let view = fixture.editor.window().buffer();
    fixture.answer(RecoveryAction::Recover(0));
    assert!(fixture.editor.buffer(view).is_none());
    assert_eq!(fixture.file_text(), "the copy\n", "the text is shared");
    assert_eq!(fixture.editor.place_behind_internal(), None);
}

/// Recovers, with the two `r`s.
fn recover(fixture: &mut Fixture) {
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    fixture.answer(RecoveryAction::Recover(0));
}

#[test]
fn a_recovery_is_refused_with_the_reason_while_the_buffer_has_edits_of_its_own() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    let id = fixture.buffer();
    let edit = oso_core::buffer::Replacement {
        range: 0..0,
        text: "typed ",
    };
    fixture.editor.apply_edits(id, &[edit]).unwrap();
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    fixture.answer(RecoveryAction::Recover(0));
    assert!(
        fixture.last_message().contains("has unsaved edits"),
        "{}",
        fixture.last_message()
    );
    assert_eq!(fixture.file_text(), "typed on disk\n");
    assert_eq!(
        shown(&fixture.editor).mode(),
        Mode::CopyView,
        "nothing moved"
    );
}

#[test]
fn a_recovery_is_refused_while_the_copy_loads() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    let work = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Recover(HEADER_ROWS));
    assert!(
        work.iter()
            .any(|work| matches!(work, CopyWork::Load { .. }))
    );
    fixture.answer(RecoveryAction::Recover(0));
    assert_eq!(fixture.last_message(), "The copy is still loading");
    assert_eq!(fixture.file_text(), "on disk\n");
}

#[test]
fn a_copy_that_is_not_text_is_not_recovered() {
    for (test, bytes) in [("invalid", &b"\xff\xfe\n"[..]), ("binary", &b"a\0b\n"[..])] {
        let mut fixture = Fixture::new("on disk\n", "1234");
        fs::write(fixture.directory.join(format!("{STEM}.txt")), bytes).unwrap();
        let meta = fixture.directory.join(format!("{STEM}.meta"));
        let said = fs::read_to_string(&meta).unwrap();
        let said = said.replace("bytes 4", &format!("bytes {}", bytes.len()));
        fs::write(&meta, said).unwrap();
        recover(&mut fixture);
        assert!(fixture.last_message().contains("not text"), "{test}");
        assert_eq!(fixture.file_text(), "on disk\n", "{test}");
    }
}

#[test]
fn only_a_row_that_may_be_recovered_is() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fs::write(
        fixture.directory.join(format!("{STEM}.txt")),
        "far longer than it says it is",
    )
    .unwrap();
    fixture.scan();
    fixture.open_list();
    assert_eq!(fixture.state(), RowState::Damaged);
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        "This copy has been changed since it was written"
    );
    assert_eq!(shown(&fixture.editor).mode(), Mode::CopyList);
}

#[test]
fn a_copy_of_a_file_not_open_says_how_to_open_it_and_opens_nothing() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let elsewhere = fixture.scratch.0.join("elsewhere.txt");
    fs::write(&elsewhere, "keep me").unwrap();
    let identity = Identity {
        name: "elsewhere.txt".to_owned(),
        path: Some(elsewhere.clone()),
        location: Some(elsewhere.clone()),
        withheld: false,
        bom: false,
        line_ending: LineEnding::Lf,
    };
    let stem = "00000000000000bb-1790000001-77-0";
    let meta = write::meta_text(&identity, None, 2, Why::Exit, 1_790_000_001);
    fs::write(fixture.directory.join(format!("{stem}.meta")), meta).unwrap();
    fs::write(fixture.directory.join(format!("{stem}.txt")), "xx").unwrap();
    fixture.scan();
    fixture.open_list();
    let buffers = fixture.editor.buffers().count();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        format!("Open {} first, with oso PATH", elsewhere.display())
    );
    assert_eq!(fixture.editor.buffers().count(), buffers);
    assert_eq!(fs::read(&elsewhere).unwrap(), b"keep me");
}

#[test]
fn a_save_the_disk_confirmed_removes_the_copy() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let mut stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    stamp.len += 1;
    fixture.editor.save_finished(id, content, stamp);
    let work = fixture.offer.save_ended(&mut fixture.editor, id, true);
    fixture.run(work);
    assert!(!fixture.copy_is_on_disk());
    assert_eq!(
        fixture.last_message(),
        "Deleted the emergency copy of notes.txt"
    );
}

#[test]
fn a_save_the_disk_did_not_confirm_or_that_failed_keeps_the_copy() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    fixture.editor.save_finished(id, content, stamp);
    let work = fixture.offer.save_ended(&mut fixture.editor, id, false);
    fixture.run(work);
    assert!(fixture.copy_is_on_disk());
}

#[test]
fn a_save_of_edits_made_after_it_started_keeps_the_copy_until_the_next() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    // The buffer is still modified, as when a save ends with text typed since.
    let work = fixture.offer.save_ended(&mut fixture.editor, id, true);
    fixture.run(work);
    assert!(fixture.copy_is_on_disk());
}

#[test]
fn undoing_a_recovery_withdraws_it_and_a_later_save_keeps_the_copy() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    fixture.editor.undo().unwrap();
    assert_eq!(fixture.file_text(), "on disk\n");
    fixture.scan();
    assert_eq!(fixture.state(), RowState::Unchanged, "offered again");
    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    fixture.editor.save_finished(id, content, stamp);
    let work = fixture.offer.save_ended(&mut fixture.editor, id, true);
    fixture.run(work);
    assert!(fixture.copy_is_on_disk());
}

#[test]
fn deleting_removes_both_files_and_the_row() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.delete(HEADER_ROWS);
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 0);
    assert!(fixture.offer.rows.is_empty());
    assert!(
        fixture
            .last_message()
            .starts_with("Deleted the emergency copy")
    );
    let mut status = oso_core::editor::Status::default();
    let list = fixture.editor.window().buffer();
    fixture.editor.leave_copies();
    fixture.editor.status_into(&mut status);
    assert_eq!(status.warnings, "", "nothing waits any more");
    assert!(fixture.editor.buffer(list).is_some());
}

#[test]
fn deleting_what_is_gone_is_no_failure_and_a_view_of_it_goes_too() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    let view = fixture.editor.window().buffer();
    fixture.editor.copies_back();
    fixture.answer(RecoveryAction::WentBack);
    fs::remove_file(fixture.directory.join(format!("{STEM}.txt"))).unwrap();
    fs::remove_file(fixture.directory.join(format!("{STEM}.meta"))).unwrap();
    fixture.delete(HEADER_ROWS);
    assert!(
        fixture.last_message().starts_with("Deleted"),
        "{}",
        fixture.last_message()
    );
    assert!(fixture.editor.buffer(view).is_none());
}

mod answers;
