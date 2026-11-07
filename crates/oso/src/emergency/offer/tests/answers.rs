//! What comes of an answer, by what a copy's file and its verdict are (ADR 0064).
//!
//! Must not depend on: `app` or winit.

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
    let secret =
        "oso-emergency-copy 1\nname .env\nbytes 9\npath /home/me/.env\nlocation /home/me/.env\n";
    fs::write(&meta, secret).unwrap();
    fixture.scan();
    fixture.open_list();
    assert_eq!(fixture.word(), Word::NotOpen);
    fixture.answer(CopyAction::View(HEADER_ROWS));
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
    assert_eq!(fixture.word(), Word::Unknown);
    fixture.answer(CopyAction::View(HEADER_ROWS));
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
    fixture.answer(CopyAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::SecretName);
}

#[test]
fn a_compare_shows_the_two_sides_and_is_withheld_with_the_copy() {
    let mut fixture = Fixture::new("one\ntwo\n", "one\nthree\n");
    fixture.scan();
    fixture.open_list();
    fixture.answer(CopyAction::Compare(HEADER_ROWS));
    let diff = shown(&fixture.editor);
    assert_eq!(diff.name(), "*compare notes.txt*");
    assert_eq!(diff.mode(), Mode::EmergencyCopy);
    let text = diff.text().to_string();
    assert!(
        text.contains("-two\n") && text.contains("+three\n"),
        "{text}"
    );
    assert_eq!(fs::read(&fixture.file).unwrap(), b"one\ntwo\n");

    // From a compare, `r` looks first, as it does from the list.
    fixture.answer(CopyAction::Recover(0));
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
    fixture.answer(CopyAction::Compare(HEADER_ROWS));
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
    fixture.answer(CopyAction::Compare(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        "notes.txt has no file here to compare it with"
    );
}

#[test]
fn a_compare_that_ends_after_the_person_left_opens_nothing() {
    let mut fixture = Fixture::new("one\n", "two\n");
    fixture.scan();
    fixture.open_list();
    let work = fixture
        .offer
        .answer(&mut fixture.editor, CopyAction::Compare(HEADER_ROWS));
    fixture.editor.leave_copies();
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
    assert_eq!(fixture.word(), Word::LeftBySave);
    fixture.answer(CopyAction::Recover(HEADER_ROWS));
    assert_eq!(
        fixture.last_message(),
        "Nothing says where this text belongs"
    );
    fixture.answer(CopyAction::View(HEADER_ROWS));
    assert_eq!(shown(&fixture.editor).verdict(), Verdict::Unresolved);
    assert_eq!(shown(&fixture.editor).text().to_string(), "part of it");
    fixture.answer(CopyAction::Delete(0));
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
        .answer(&mut fixture.editor, CopyAction::View(HEADER_ROWS));
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
    let _ = Path::new("");
}
