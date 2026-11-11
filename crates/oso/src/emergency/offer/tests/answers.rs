//! What comes of an answer, by what a copy's file and its verdict are (ADR 0064).
//!
//! Must not depend on: `app` or winit.

use std::time::Instant;

use super::*;

#[test]
fn a_file_changed_since_is_recovered_with_a_word_about_saving() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let stem_meta = fixture.directory.join(format!("{STEM}.meta"));
    let said = fs::read_to_string(&stem_meta).unwrap();
    let said: String = said
        .lines()
        .filter(|line| !line.starts_with("file-"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&stem_meta, said).unwrap();
    recover(&mut fixture);
    assert!(
        fixture
            .last_message()
            .contains("replaces what is there now")
    );
    assert_eq!(fixture.file_text(), "the copy\n");
}

#[test]
fn a_file_that_has_gone_comes_back_private_with_the_format_the_copy_says() {
    let mut fixture = Fixture::new("on disk\n", "the copy\r\n");
    fs::remove_file(&fixture.file).unwrap();
    let id = fixture.buffer();
    let location = fixture.file.clone();
    let buffer = fixture.editor.buffer_mut(id).unwrap();
    buffer.set_baseline(Baseline {
        location,
        file: None,
    });
    recover(&mut fixture);
    assert!(
        fixture.last_message().contains("mode 0600"),
        "{}",
        fixture.last_message()
    );
    let buffer = fixture.editor.buffer(id).unwrap();
    assert_eq!(buffer.baseline().unwrap().file.unwrap().mode, 0o600);
    assert!(buffer.file_info().bom);
    assert_eq!(buffer.file_info().line_ending, LineEnding::Crlf);
}

#[test]
fn a_file_still_there_keeps_its_own_format() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    let buffer = fixture.editor.buffer(id).unwrap();
    assert!(!buffer.file_info().bom, "the metadata says otherwise");
    assert_eq!(buffer.file_info().line_ending, LineEnding::Lf);
}

#[test]
fn a_view_of_a_path_that_would_count_as_secret_now_is_withheld() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let secret = "oso-emergency-copy 1\nname .env\nwithheld false\nbytes 9\npath /home/me/.env\nlocation /home/me/.env\n";
    fs::write(&meta, secret).unwrap();
    fixture.scan();
    fixture.open_list();
    assert_eq!(fixture.state(), RowState::NotOpen);
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::SecretName);
    assert!(fixture.editor.shows_secret());
}

#[test]
fn a_view_of_what_nothing_says_anything_of_is_withheld() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fs::write(
        fixture.directory.join(format!("{STEM}.meta")),
        "not metadata\n",
    )
    .unwrap();
    fixture.scan();
    fixture.open_list();
    assert_eq!(fixture.state(), RowState::Unknown);
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::Unresolved);
}

#[test]
fn a_view_of_a_copy_of_a_secret_buffer_is_withheld_as_that_buffer_is() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let id = fixture.buffer();
    let env = PathBuf::from("/tmp/.env");
    fixture
        .editor
        .buffer_mut(id)
        .unwrap()
        .judge_secret(Some(&[env]));
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::SecretName);
}

#[test]
fn a_compare_shows_the_two_sides_and_is_withheld_with_the_copy() {
    let mut fixture = Fixture::new("one\ntwo\n", "one\nthree\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    let diff = shown(&fixture.editor);
    assert_eq!(diff.name(), "*compare notes.txt*");
    assert_eq!(diff.mode(), Mode::CopyView);
    let text = diff.text().to_string();
    assert!(
        text.contains("-two\n") && text.contains("+three\n"),
        "{text}"
    );
    assert_eq!(fs::read(&fixture.file).unwrap(), b"one\ntwo\n");

    // From a compare, `r` looks first, as it does from the list.
    fixture.answer(RecoveryAction::Recover(0));
    assert_eq!(shown(&fixture.editor).name(), "*copy of notes.txt*");
}

#[test]
fn a_compare_leaves_out_a_bom_and_has_nothing_to_say_of_the_same_text() {
    let mut on_disk = BOM.to_vec();
    on_disk.extend(b"text\nmore\n");
    let mut fixture = Fixture::new("x", "text\nmore \n");
    fs::write(&fixture.file, &on_disk).unwrap();
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    let text = shown(&fixture.editor).text().to_string();
    assert!(
        text.contains("-more\n") && text.contains("+more \n"),
        "{text}"
    );
    assert!(!text.contains('\u{feff}'), "no BOM in it");
}

#[test]
fn with_no_file_to_compare_it_says_so() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fs::remove_file(&fixture.file).unwrap();
    let id = fixture.buffer();
    let location = fixture.file.clone();
    fixture
        .editor
        .buffer_mut(id)
        .unwrap()
        .set_baseline(Baseline {
            location,
            file: None,
        });
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        "notes.txt has no file open here to compare it with"
    );
}

#[test]
fn a_compare_that_ends_after_the_person_left_opens_nothing() {
    let mut fixture = Fixture::new("one\n", "two\n");
    fixture.scan();
    fixture.open_list();
    let work = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    fixture.editor.toggle_copies();
    let on = fixture.editor.window().buffer();
    fixture.run(work);
    assert_eq!(fixture.editor.window().buffer(), on);
    assert!(fixture.last_message().contains("is ready"));
}

#[test]
fn a_row_of_a_leftover_can_be_looked_at_withheld_and_deleted_but_not_recovered() {
    let mut fixture = Fixture::new("on disk\n", "1");
    fs::remove_file(fixture.directory.join(format!("{STEM}.txt"))).unwrap();
    fs::remove_file(fixture.directory.join(format!("{STEM}.meta"))).unwrap();
    let left = fixture.file.with_file_name(".oso-save-77-123-0.tmp");
    fs::write(&left, "part of it").unwrap();
    fixture.scan();
    fixture.open_list();
    assert_eq!(fixture.state(), RowState::LeftBySave);
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        "Nothing says where this text belongs"
    );
    fixture.answer(RecoveryAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::Unresolved);
    assert_eq!(shown(&fixture.editor).text().to_string(), "part of it");
    fixture.editor.copies_back();
    fixture.answer(RecoveryAction::WentBack);
    fixture.delete(HEADER_ROWS);
    assert!(!left.exists());
    assert_eq!(fs::read(&fixture.file).unwrap(), b"on disk\n");
}

#[test]
fn a_copy_too_large_to_read_says_where_its_text_is() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.scan();
    fixture.open_list();
    fixture.offer.rows[0].size = copies::MAX_LOOK;
    let work = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::View(HEADER_ROWS));
    assert!(work.is_empty());
    assert!(fixture.last_message().contains("too large"));
    assert!(fixture.last_message().contains(STEM));
}

#[test]
fn nothing_said_or_listed_holds_the_text() {
    let mut fixture = Fixture::new("on disk\n", "FAKETYPEDTEXT\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    fixture.editor.save_finished(id, content, stamp);
    for message in kept_messages(&fixture.editor) {
        assert!(!message.contains("FAKETYPEDTEXT"), "{message}");
    }
    let list = fixture.offer.list.as_ref().unwrap().text().to_string();
    assert!(!list.contains("FAKETYPEDTEXT"), "{list}");
}

#[test]
fn one_scan_runs_at_a_time_and_what_changed_meanwhile_gets_one_more() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let first = fixture.offer.ask_for_scan(&fixture.editor);
    assert_eq!(first.len(), 1);
    for _ in 0..5 {
        assert!(fixture.offer.ask_for_scan(&fixture.editor).is_empty());
    }
    let [CopyWork::Scan(request)] = &first[..] else {
        panic!("a scan");
    };
    let scan = copies::scan(request, SystemTime::now());
    let again = fixture.offer.take_scan(&mut fixture.editor, scan, false);
    assert_eq!(again.len(), 1, "one more, for all that came while it ran");
    let [CopyWork::Scan(request)] = &again[..] else {
        panic!("a scan");
    };
    let scan = copies::scan(request, SystemTime::now());
    let none = fixture.offer.take_scan(&mut fixture.editor, scan, false);
    assert!(none.is_empty());
}

#[test]
fn a_scan_nobody_hears_from_does_not_hold_up_the_next_for_good() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.offer.scanning = Instant::now().checked_sub(PATIENCE);
    assert_eq!(fixture.offer.ask_for_scan(&fixture.editor).len(), 1);
}

#[test]
fn one_compare_runs_at_a_time_and_ending_lets_the_next_start() {
    let mut fixture = Fixture::new("one\n", "two\n");
    fixture.scan();
    fixture.open_list();
    let first = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    assert_eq!(first.len(), 1);
    let second = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    assert!(second.is_empty(), "the key can repeat as fast as it likes");
    fixture.run(first);
    assert_eq!(shown(&fixture.editor).name(), "*compare notes.txt*");
    fixture.editor.copies_back();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).name(), "*compare notes.txt*");
}

#[test]
fn several_copies_of_a_file_are_told_once_with_the_newest() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    fixture.copy("00000000000000bb-1790000001-77-0", "another\n", None);
    fixture.copy("00000000000000cc-1790000002-77-0", "and one\n", None);
    fixture.scan();
    let said = kept_messages(&fixture.editor);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("notes.txt has 3 emergency copies, the newest from"));
}

#[test]
fn several_copies_equal_to_their_files_are_said_at_once() {
    let mut fixture = Fixture::new("the same\n", "the same\n");
    fixture.copy("00000000000000bb-1790000001-77-0", "the same\n", None);
    fixture.scan();
    assert_eq!(
        fixture.last_message(),
        "Removed 2 emergency copies: their files hold the same text"
    );
    assert_eq!(fs::read_dir(&fixture.directory).unwrap().count(), 0);
}

/// Puts a second copy's text in, from the row of `stem`, by its two `r`s.
fn recover_row(fixture: &mut Fixture, stem: &str) {
    fixture.editor.toggle_copies();
    fixture.answer(RecoveryAction::Toggled);
    let row = fixture
        .offer
        .rows
        .iter()
        .position(|row| row.text.to_string_lossy().contains(stem))
        .unwrap();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS + row));
    fixture.answer(RecoveryAction::Recover(0));
}

/// A copy written at `seconds`, older or newer than the others.
fn copy_written_at(fixture: &Fixture, stem: &str, text: &str, seconds: u64) {
    fixture.copy(stem, text, None);
    let meta = fixture.directory.join(format!("{stem}.meta"));
    let said = fs::read_to_string(&meta)
        .unwrap()
        .replace("written 1790000000", &format!("written {seconds}"));
    fs::write(&meta, said).unwrap();
}

#[test]
fn a_copy_whose_recovery_was_undone_stays_when_a_later_one_is_saved() {
    let mut fixture = Fixture::new("on disk\n", "the first copy\n");
    let other = "00000000000000bb-1790000009-77-0";
    fixture.copy(other, "the second copy\n", None);
    fixture.scan();
    recover_row(&mut fixture, STEM);
    assert_eq!(fixture.file_text(), "the first copy\n");
    fixture.editor.undo().unwrap();
    let scan = fixture.offer.after_command(&fixture.editor);
    fixture.run(scan);
    recover_row(&mut fixture, other);
    assert_eq!(fixture.file_text(), "the second copy\n");

    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    fixture.editor.save_finished(id, content, stamp);
    let work = fixture.offer.save_ended(&mut fixture.editor, id, true);
    fixture.run(work);
    assert!(
        fixture.copy_is_on_disk(),
        "its text is in no buffer or file"
    );
    assert!(!fixture.directory.join(format!("{other}.txt")).exists());
}

#[test]
fn undoing_a_recovery_brings_the_warning_and_the_row_back_at_once() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    assert_eq!(fixture.state(), RowState::Recovered);
    assert!(
        fixture.offer.after_command(&fixture.editor).is_empty(),
        "as it was"
    );
    fixture.editor.undo().unwrap();
    let scan = fixture.offer.after_command(&fixture.editor);
    assert_eq!(scan.len(), 1);
    fixture.run(scan);
    assert_eq!(fixture.state(), RowState::Unchanged);
    let mut status = oso_core::editor::Status::default();
    fixture
        .editor
        .status_into(fixture.editor.tiled().0.id(), &mut status);
    assert_eq!(status.warnings, "[emergency copy] ");
}

#[test]
fn a_second_d_takes_only_the_copy_the_first_marked_though_rows_changed_between() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let older = "00000000000000bb-1780000000-77-0";
    copy_written_at(&fixture, older, "older\n", 1_780_000_000);
    fixture.scan();
    fixture.open_list();
    assert!(fixture.offer.rows[0].text.to_string_lossy().contains(STEM));
    fixture.answer(RecoveryAction::ForgetMarks);
    fixture.answer(RecoveryAction::MarkForDelete(HEADER_ROWS));

    // Another instance took the marked one away, so the older one is on that line now.
    fs::remove_file(fixture.directory.join(format!("{STEM}.txt"))).unwrap();
    fs::remove_file(fixture.directory.join(format!("{STEM}.meta"))).unwrap();
    fixture.scan();
    fixture.answer(RecoveryAction::Delete(HEADER_ROWS));
    assert!(
        fixture.directory.join(format!("{older}.txt")).exists(),
        "nobody looked at it"
    );
}

#[test]
fn the_cursor_stays_on_its_copy_as_rows_change() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    copy_written_at(
        &fixture,
        "00000000000000bb-1780000000-77-0",
        "older\n",
        1_780_000_000,
    );
    fixture.scan();
    fixture.open_list();
    fixture.editor.set_copies_cursor(HEADER_ROWS + 1);
    copy_written_at(
        &fixture,
        "00000000000000cc-1800000000-77-0",
        "newer\n",
        1_800_000_000,
    );
    fixture.answer(RecoveryAction::Refresh);
    assert_eq!(
        fixture.line(),
        HEADER_ROWS + 2,
        "it moved down with its row"
    );
}

#[test]
fn the_cursor_stays_on_its_copy_while_a_prompt_is_open() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    copy_written_at(
        &fixture,
        "00000000000000bb-1780000000-77-0",
        "older\n",
        1_780_000_000,
    );
    fixture.scan();
    fixture.open_list();
    fixture.editor.set_copies_cursor(HEADER_ROWS + 1);
    let number = oso_core::minibuffer::Kind::Number { history: None };
    fixture.editor.open_prompt("Go to line: ", number).unwrap();
    copy_written_at(
        &fixture,
        "00000000000000cc-1800000000-77-0",
        "newer\n",
        1_800_000_000,
    );
    fixture.answer(RecoveryAction::Refresh);
    fixture.editor.quit();
    assert_eq!(
        fixture.line(),
        HEADER_ROWS + 2,
        "it moved down with its row"
    );
}

#[test]
fn a_text_that_changed_after_the_scan_is_not_recovered() {
    let mut fixture = Fixture::new("on disk\n", "the whole copy\n");
    fixture.scan();
    fixture.open_list();
    fs::write(fixture.directory.join(format!("{STEM}.txt")), "cut").unwrap();
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    fixture.answer(RecoveryAction::Recover(0));
    assert!(fixture.last_message().contains("changed as it was read"));
    assert_eq!(fixture.file_text(), "on disk\n");
}

#[test]
fn what_depends_on_withheld_text_stays_out_of_the_messages() {
    let mut fixture = Fixture::new("on disk\n", "1234");
    let id = fixture.buffer();
    let env = PathBuf::from("/tmp/.env");
    fixture
        .editor
        .buffer_mut(id)
        .unwrap()
        .judge_secret(Some(&[env]));
    fs::write(
        fixture.directory.join(format!("{STEM}.txt")),
        b"\xff\xfe\xfd\xfc",
    )
    .unwrap();
    recover(&mut fixture);
    let kept = kept_messages(&fixture.editor);
    assert!(
        !kept.iter().any(|said| said.contains("not text")),
        "{kept:?}"
    );
    let mut status = oso_core::editor::Status::default();
    fixture
        .editor
        .status_into(fixture.editor.tiled().0.id(), &mut status);
    assert!(
        status.echo.contains("not text"),
        "for the person at the screen"
    );
}

#[test]
fn recovering_withheld_text_withholds_the_buffer_it_went_in_until_undone() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let said = fs::read_to_string(&meta)
        .unwrap()
        .replace("withheld false", "withheld true");
    fs::write(&meta, said).unwrap();
    recover(&mut fixture);
    let id = fixture.buffer();
    assert_eq!(fixture.file_text(), "the copy\n");
    assert_eq!(
        fixture.editor.buffer(id).unwrap().verdict(),
        Verdict::Unresolved
    );
    fixture.editor.undo().unwrap();
    assert_eq!(
        fixture.editor.buffer(id).unwrap().verdict(),
        Verdict::Ordinary
    );
}

#[test]
fn a_new_file_nobody_saved_has_no_file_to_compare_and_comes_back_private() {
    let mut fixture = Fixture::new("", "typed\n");
    fs::remove_file(&fixture.file).unwrap();
    let id = fixture.buffer();
    let location = fixture.file.clone();
    let buffer = fixture.editor.buffer_mut(id).unwrap();
    buffer.set_baseline(Baseline {
        location,
        file: None,
    });
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let said: String = fs::read_to_string(&meta)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("file-"))
        .map(|line| format!("{line}\n"))
        .collect();
    fs::write(&meta, said).unwrap();
    fixture.scan();
    assert_eq!(fixture.state(), RowState::NoFile);
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    assert!(fixture.last_message().contains("no file open here"));
    fixture.answer(RecoveryAction::Recover(HEADER_ROWS));
    fixture.answer(RecoveryAction::Recover(0));
    assert!(
        fixture.last_message().contains("mode 0600"),
        "{}",
        fixture.last_message()
    );
    let baseline = fixture
        .editor
        .buffer(id)
        .unwrap()
        .baseline()
        .unwrap()
        .clone();
    assert_eq!(baseline.file.unwrap().mode, 0o600);
    assert_eq!(baseline.file.unwrap().len, 0, "nothing from the .meta");
}

#[test]
fn a_redo_after_the_copy_was_deleted_brings_back_all_that_goes_with_the_text() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let said = fs::read_to_string(&meta)
        .unwrap()
        .replace("withheld false", "withheld true");
    fs::write(&meta, said).unwrap();
    recover(&mut fixture);
    fixture.editor.undo().unwrap();
    let scan = fixture.offer.after_command(&fixture.editor);
    fixture.run(scan);
    fixture.open_list();
    fixture.delete(HEADER_ROWS);
    fixture.editor.toggle_copies();
    fixture.editor.redo().unwrap();
    let id = fixture.buffer();
    assert_eq!(fixture.file_text(), "the copy\n");
    assert_eq!(
        fixture.editor.buffer(id).unwrap().verdict(),
        Verdict::Unresolved
    );
}

#[test]
fn what_a_save_compares_does_not_go_back_with_an_undo_past_a_save() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    recover(&mut fixture);
    let id = fixture.buffer();
    let content = fixture.editor.buffer(id).unwrap().content_id();
    let mut stamp = stamp_of(&fs::metadata(&fixture.file).unwrap()).unwrap();
    stamp.len = 12_345;
    fixture.editor.save_finished(id, content, stamp);
    // The disk did not confirm that save, so the recovery is still remembered.
    let work = fixture.offer.save_ended(&mut fixture.editor, id, false);
    fixture.run(work);
    let edit = oso_core::buffer::Replacement {
        range: 0..0,
        text: "more ",
    };
    fixture.editor.begin_undo_group();
    fixture.editor.apply_edits(id, &[edit]).unwrap();
    fixture.editor.end_undo_group();
    fixture.editor.undo().unwrap();
    let baseline = fixture
        .editor
        .buffer(id)
        .unwrap()
        .baseline()
        .unwrap()
        .clone();
    assert_eq!(baseline.file, Some(stamp), "it would refuse the next save");
}

#[test]
fn a_recovery_that_would_strand_a_modified_file_is_refused() {
    let mut fixture = Fixture::new("on disk\n", "x");
    let identity = Identity {
        name: "*scratch*".to_owned(),
        path: None,
        location: None,
        withheld: false,
        bom: false,
        line_ending: LineEnding::Lf,
    };
    let stem = "00000000000000dd-1790000005-77-0";
    let meta = write::meta_text(&identity, None, 4, Why::Exit, 1_790_000_005);
    fs::write(fixture.directory.join(format!("{stem}.meta")), meta).unwrap();
    fs::write(fixture.directory.join(format!("{stem}.txt")), "text").unwrap();
    fixture.editor.add_buffer(Buffer::new("*scratch*"));
    let id = fixture.buffer();
    let edit = oso_core::buffer::Replacement {
        range: 0..0,
        text: "typed ",
    };
    fixture.editor.apply_edits(id, &[edit]).unwrap();
    fixture.scan();
    recover_row(&mut fixture, stem);
    assert!(
        fixture
            .last_message()
            .contains("nothing would lead back to it"),
        "{}",
        fixture.last_message()
    );
    assert_eq!(
        shown(&fixture.editor).mode(),
        Mode::CopyView,
        "nothing moved"
    );
}

#[test]
fn the_name_said_is_the_buffers_and_not_what_a_meta_calls_it() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let said = fs::read_to_string(&meta)
        .unwrap()
        .replace("name notes.txt", "name other.txt");
    fs::write(&meta, said).unwrap();
    fixture.scan();
    assert!(fixture.last_message().starts_with("notes.txt has"));
    assert_eq!(fixture.offer.rows[0].name, "notes.txt");
}

#[test]
fn a_scan_landing_late_does_not_move_a_cursor_the_person_moved() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    copy_written_at(
        &fixture,
        "00000000000000bb-1800000000-77-0",
        "newer\n",
        1_800_000_000,
    );
    fixture.open_list();
    assert_eq!(fixture.line(), HEADER_ROWS, "the newest of this file's");
    fixture.editor.set_copies_cursor(HEADER_ROWS + 1);
    fixture.offer.cursor_for = Some(fixture.buffer());
    fixture.scan();
    assert_eq!(fixture.line(), HEADER_ROWS + 1);
}

#[test]
fn a_failure_of_a_withheld_copy_says_only_what_the_socket_may_read_from_the_list() {
    let mut fixture = Fixture::new("one\n", "x");
    let meta = fixture.directory.join(format!("{STEM}.meta"));
    let said = fs::read_to_string(&meta)
        .unwrap()
        .replace("withheld false", "withheld true");
    fs::write(&meta, said).unwrap();
    fs::write(fixture.directory.join(format!("{STEM}.txt")), b"\xff").unwrap();
    fixture.scan();
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    let mut status = oso_core::editor::Status::default();
    fixture
        .editor
        .status_into(fixture.editor.tiled().0.id(), &mut status);
    assert_eq!(status.echo, "This copy cannot be compared here");
    assert!(
        !kept_messages(&fixture.editor)
            .iter()
            .any(|said| said.contains("not text"))
    );
}

#[test]
fn a_copy_starting_with_a_byte_order_mark_is_not_recovered_and_stays() {
    let mut text = oso_core::decode::BOM.to_vec();
    text.extend(b"text\n");
    let mut fixture = Fixture::new("on disk\n", "12345678");
    fs::write(fixture.directory.join(format!("{STEM}.txt")), &text).unwrap();
    recover(&mut fixture);
    assert!(
        fixture.last_message().contains("byte order mark"),
        "{}",
        fixture.last_message()
    );
    assert_eq!(fixture.file_text(), "on disk\n");
    assert!(fixture.copy_is_on_disk());
}

#[test]
fn undoing_a_recovery_returns_the_person_to_where_they_were() {
    let mut fixture = Fixture::new("one\ntwo\nthree\n", "the copy\n");
    fixture.editor.restore_positions(
        oso_core::selection::SelectionSet::new(6),
        Default::default(),
    );
    recover(&mut fixture);
    assert_eq!(fixture.file_text(), "the copy\n");
    assert_eq!(fixture.editor.window().selections.primary().cursor, 0);
    assert_eq!(fixture.editor.place_behind_internal(), None);
    fixture.editor.undo().unwrap();
    assert_eq!(fixture.editor.window().selections.primary().cursor, 6);
}

#[test]
fn a_list_come_back_to_from_messages_is_on_the_same_copy_though_rows_changed() {
    let mut fixture = Fixture::new("on disk\n", "the copy\n");
    copy_written_at(
        &fixture,
        "00000000000000bb-1780000000-77-0",
        "older\n",
        1_780_000_000,
    );
    fixture.scan();
    fixture.open_list();
    fixture.editor.set_copies_cursor(HEADER_ROWS + 1);
    fixture.editor.toggle_messages();
    copy_written_at(
        &fixture,
        "00000000000000cc-1800000000-77-0",
        "newer\n",
        1_800_000_000,
    );
    fixture.scan();
    fixture.editor.toggle_messages();
    assert_eq!(fixture.line(), HEADER_ROWS + 2, "still the older one");
}

#[test]
fn a_recovered_row_can_be_compared() {
    let mut fixture = Fixture::new("one\n", "two\n");
    recover(&mut fixture);
    assert_eq!(fixture.state(), RowState::Recovered);
    fixture.open_list();
    fixture.answer(RecoveryAction::Compare(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).name(), "*compare notes.txt*");
}

#[test]
fn a_compare_given_up_on_neither_shows_nor_lets_more_pile_up() {
    let mut fixture = Fixture::new("one\n", "two\n");
    fixture.scan();
    fixture.open_list();
    let old = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    // Nobody heard from it for so long that another was started.
    fixture.offer.comparing = fixture
        .offer
        .comparing
        .map(|(id, _)| (id, Instant::now().checked_sub(PATIENCE).unwrap()));
    let new = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    assert_eq!(new.len(), 1);
    fixture.run(old);
    assert_eq!(
        shown(&fixture.editor).mode(),
        Mode::CopyList,
        "the late one shows nothing"
    );
    let still = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    assert!(still.is_empty(), "the newer one still counts as running");
    fixture.run(new);
    assert_eq!(shown(&fixture.editor).name(), "*compare notes.txt*");
}

#[test]
fn a_compare_ending_takes_only_what_said_it_was_comparing_from_the_echo_area() {
    let mut fixture = Fixture::new("one\n", "two\n");
    fixture.scan();
    fixture.open_list();
    let work = fixture
        .offer
        .answer(&mut fixture.editor, RecoveryAction::Compare(HEADER_ROWS));
    fixture.answer(RecoveryAction::ForgetMarks);
    fixture.answer(RecoveryAction::MarkForDelete(HEADER_ROWS));
    fixture.run(work);
    let mut status = oso_core::editor::Status::default();
    fixture
        .editor
        .status_into(fixture.editor.tiled().0.id(), &mut status);
    assert!(status.echo.starts_with("Press d again"), "{}", status.echo);
}
