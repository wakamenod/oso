//! Finding copies in a real directory, with no window (ADR 0064).
//!
//! Must not depend on: `app` or winit.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use oso_core::buffer::{Baseline, Buffer, FileStamp, Verdict};
use oso_core::editor::Editor;

use super::*;
use crate::emergency::tests::Scratch;
use crate::emergency::{Identity, Why, write};
use crate::loader::stamp_of;

const STEM: &str = "00000000000000aa-1790000000-77-0";

/// The time rows are judged at, an hour and a bit after the stem above, by the file's own time.
fn later() -> SystemTime {
    SystemTime::now() + 2 * LEFT_BEHIND
}

/// The directory, private as a real one is.
fn copies_of(scratch: &Scratch) -> PathBuf {
    oso_platform::privacy::PrivateDir::open(scratch.copies())
        .unwrap()
        .path()
        .to_owned()
}

/// A copy of a file at `path`, as the writer leaves it, with what `file` says of the file.
fn write_copy(directory: &Path, stem: &str, path: &Path, text: &str, file: Option<FileStamp>) {
    let identity = Identity {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path: Some(path.to_owned()),
        location: Some(path.to_owned()),
        withheld: false,
        bom: false,
        line_ending: oso_core::decode::LineEnding::Lf,
    };
    let meta = write::meta_text(&identity, file, text.len(), Why::Panic, 1_790_000_000);
    fs::write(directory.join(format!("{stem}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{stem}.txt")), text).unwrap();
}

/// What a scan is told of a file buffer that has read `path`.
fn target_for(path: &Path) -> Target {
    let mut buffer = Buffer::visiting(path.to_owned());
    buffer.finish_empty();
    buffer.judge_secret(Some(&[path.to_owned()]));
    let stamp = fs::metadata(path).ok().map(|m| stamp_of(&m).unwrap());
    buffer.set_baseline(Baseline {
        location: path.to_owned(),
        file: stamp,
    });
    let editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    let found = targets_of(&editor);
    found
        .into_iter()
        .find(|target| target.buffer == id)
        .unwrap()
}

fn request(directory: &Path, targets: Vec<Target>) -> ScanRequest {
    ScanRequest {
        id: 1,
        directory: Some(directory.to_owned()),
        targets,
        recovered: Vec::new(),
    }
}

fn words(scan: &Scan) -> Vec<RowState> {
    scan.rows.iter().map(|row| row.state).collect()
}

#[test]
fn a_directory_that_is_not_there_says_nothing() {
    let scratch = Scratch::new();
    let scan = scan(&request(&scratch.copies(), Vec::new()), later());
    assert!(scan.rows.is_empty());
    assert_eq!(scan.problem, None);
    assert!(
        scan.list
            .text()
            .to_string()
            .starts_with("0 emergency copies.")
    );
}

#[test]
fn a_copy_of_a_file_not_open_is_listed_and_that_file_is_not_touched() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    // Nothing is at this path, and a scan that looked would find out by reading it.
    let elsewhere = scratch.0.join("no-such-file.txt");
    write_copy(&directory, STEM, &elsewhere, "text", None);
    let scan = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&scan), [RowState::NotOpen]);
    let row = &scan.rows[0];
    assert_eq!(row.target, None);
    assert_eq!(row.shown, elsewhere.display().to_string());
    assert_eq!(row.size, 4);
    assert_eq!(row.at_least, Verdict::Ordinary);
    assert!(scan.list.text().to_string().contains("not open"));
}

#[test]
fn a_copy_of_an_open_file_says_how_the_file_compares_with_what_it_records() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("notes.txt");
    fs::write(&file, "on disk\n").unwrap();
    let stamp = stamp_of(&fs::metadata(&file).unwrap()).unwrap();

    write_copy(&directory, STEM, &file, "a copy", Some(stamp));
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::Unchanged]);
    assert!(found.rows[0].target.is_some());

    let older = FileStamp {
        modified: stamp.modified - Duration::from_secs(5),
        ..stamp
    };
    write_copy(&directory, STEM, &file, "a copy", Some(older));
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(
        words(&found),
        [RowState::Changed],
        "a change is any inequality"
    );

    write_copy(&directory, STEM, &file, "a copy", None);
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::Changed], "it appeared since");

    fs::remove_file(&file).unwrap();
    write_copy(&directory, STEM, &file, "a copy", Some(stamp));
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::Gone]);

    write_copy(&directory, STEM, &file, "a copy", None);
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::NoFile], "there never was one");
}

#[test]
fn a_location_finds_a_buffer_whatever_path_led_there_and_a_path_alone_may_not() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let real = scratch.0.join("real.txt");
    let meta = |path: &str, location: Option<&str>| Meta {
        path: Some(encoded(path.as_bytes())),
        location: location.map(|location| encoded(location.as_bytes())),
        ..Meta::default()
    };
    let target = Target {
        encoded_path: Some(encoded(b"/link/a.txt")),
        encoded_location: Some(encoded(b"/real/a.txt")),
        encoded_typed: Some(encoded(b"/typed/a.txt")),
        location: Some(real),
        ..target_for(&scratch.0.join("whatever"))
    };
    let targets = [target];
    assert!(matches!(
        matching(&meta("/other/a.txt", Some("/real/a.txt")), &targets),
        Matched::Target(_)
    ));
    assert!(matches!(
        matching(&meta("/other/a.txt", Some("/typed/a.txt")), &targets),
        Matched::Target(_)
    ));
    assert!(matches!(
        matching(&meta("/link/a.txt", None), &targets),
        Matched::Target(_)
    ));
    assert!(matches!(
        matching(&meta("/link/a.txt", Some("/moved/a.txt")), &targets),
        Matched::Elsewhere
    ));
    assert!(matches!(
        matching(&meta("/other/a.txt", Some("/moved/a.txt")), &targets),
        Matched::Nothing
    ));
    drop(directory);
}

#[test]
fn a_path_that_needs_encoding_is_found_by_its_bytes() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("メモ 1.txt");
    fs::write(&file, "disk").unwrap();
    write_copy(&directory, STEM, &file, "a copy", None);
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert!(found.rows[0].target.is_some());
    assert_eq!(found.rows[0].shown, file.display().to_string());
}

#[test]
fn a_copy_with_no_file_finds_an_empty_buffer_of_its_name_and_no_other() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let identity = Identity {
        name: "*scratch*".to_owned(),
        path: None,
        location: None,
        withheld: false,
        bom: false,
        line_ending: oso_core::decode::LineEnding::Lf,
    };
    let meta = write::meta_text(&identity, None, 4, Why::Exit, 1_790_000_000);
    fs::write(directory.join(format!("{STEM}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "text").unwrap();

    let empty = targets_of(&Editor::default());
    let found = scan(&request(&directory, empty), later());
    assert_eq!(words(&found), [RowState::NoFile]);
    assert!(found.rows[0].target.is_some());

    let full = targets_of(&Editor::with_buffer(Buffer::from_text("*scratch*", "mine")));
    let found = scan(&request(&directory, full), later());
    assert_eq!(words(&found), [RowState::NoFile]);
    assert_eq!(found.rows[0].target, None, "its text is not swapped out");
}

#[test]
fn a_copy_equal_to_its_file_goes_and_is_not_offered() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("notes.txt");
    fs::write(&file, "the same\n").unwrap();
    write_copy(&directory, STEM, &file, "the same\n", None);
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert!(found.rows.is_empty());
    assert_eq!(found.removed, [STEM]);
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
    assert_eq!(fs::read(&file).unwrap(), b"the same\n");
}

#[test]
fn an_equal_copy_of_a_file_not_open_stays_as_that_file_is_not_read() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("notes.txt");
    fs::write(&file, "the same\n").unwrap();
    write_copy(&directory, STEM, &file, "the same\n", None);
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&found), [RowState::NotOpen]);
}

#[test]
fn a_text_not_the_size_it_says_is_damaged_and_is_not_read() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("notes.txt");
    fs::write(&file, "on disk").unwrap();
    write_copy(&directory, STEM, &file, "small", None);
    fs::write(directory.join(format!("{STEM}.txt")), "x".repeat(4096)).unwrap();
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::Damaged]);
    assert_eq!(found.rows[0].size, 4096);
}

#[test]
fn what_cannot_be_parsed_is_unknown_and_says_nothing_of_where_it_belongs() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    fs::write(
        directory.join(format!("{STEM}.meta")),
        "oso-emergency-copy 2\n",
    )
    .unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "text").unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&found), [RowState::Unknown]);
    assert_eq!(found.rows[0].at_least, Verdict::Unresolved);
    assert!(found.rows[0].paths.is_empty());
}

#[test]
fn a_meta_over_the_limit_is_unknown() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let mut meta = b"oso-emergency-copy 1\nbytes 4\n".to_vec();
    meta.resize(MAX_META as usize + 1, b'x');
    fs::write(directory.join(format!("{STEM}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "text").unwrap();
    assert_eq!(
        words(&scan(&request(&directory, Vec::new()), later())),
        [RowState::Unknown]
    );
}

#[test]
fn a_text_with_no_meta_waits_as_a_writer_may_be_at_work_on_it() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    fs::write(directory.join(format!("{STEM}.txt")), "whole").unwrap();
    let found = scan(&request(&directory, Vec::new()), SystemTime::now());
    assert!(found.rows.is_empty(), "just written");
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&found), [RowState::Unknown]);
}

#[test]
fn a_temporary_file_in_the_directory_waits_too_and_is_listed_as_left_by_a_save() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let path = directory.join(".oso-save-77-123-0.tmp");
    fs::write(&path, "part of a text").unwrap();
    let found = scan(&request(&directory, Vec::new()), SystemTime::now());
    assert!(found.rows.is_empty());
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(words(&found), [RowState::LeftBySave]);
    assert_eq!(found.rows[0].at_least, Verdict::Unresolved, "withheld");
    assert_eq!(found.rows[0].meta, None);
}

#[test]
fn what_is_left_beside_a_file_is_listed_and_only_there() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let project = scratch.0.join("project");
    fs::create_dir(&project).unwrap();
    let other = scratch.0.join("other");
    fs::create_dir(&other).unwrap();
    let file = project.join("notes.txt");
    fs::write(&file, "text").unwrap();
    fs::write(project.join(".oso-save-77-123-0.tmp"), "left").unwrap();
    fs::write(project.join(".oso-save-nothing.tmp"), "not of this form").unwrap();
    fs::write(project.join("oso-save-77-123-0.tmp"), "nor this").unwrap();
    fs::write(other.join(".oso-save-77-123-0.tmp"), "no file open here").unwrap();
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(words(&found), [RowState::LeftBySave]);
    assert_eq!(found.rows[0].text, project.join(".oso-save-77-123-0.tmp"));
    // Nothing removes one unasked.
    assert!(found.rows[0].text.exists());
    let young = scan(
        &request(&directory, vec![target_for(&file)]),
        SystemTime::now(),
    );
    assert!(young.rows.is_empty(), "a save may be running");
}

#[test]
fn a_leftover_under_a_secret_directory_says_so_by_its_path() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let ssh = scratch.0.join(".ssh");
    fs::create_dir(&ssh).unwrap();
    let file = ssh.join("config");
    fs::write(&file, "text").unwrap();
    fs::write(ssh.join(".oso-save-77-123-0.tmp"), "left").unwrap();
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    let mut buffer = Buffer::new("look");
    buffer.judge_copy(&found.rows[0].paths, found.rows[0].at_least);
    assert_eq!(buffer.verdict(), Verdict::SecretName);
}

#[test]
fn names_of_any_other_form_are_left_alone() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    for name in [
        "notes.txt",
        "notes.meta",
        "00000000000000AA-1-2-3.txt",
        "00000000000000aa-1-2.txt",
        "00000000000000aa-1-2-3-4.txt",
        "0000000000000aa-1-2-3.txt",
        "00000000000000aa-1-2-x.meta",
        ".oso-save-1-2.tmp",
        ".oso-save-1-2-3.tmp.bak",
    ] {
        fs::write(directory.join(name), "text").unwrap();
    }
    let found = scan(&request(&directory, Vec::new()), later());
    assert!(found.rows.is_empty(), "{:?}", words(&found));
    assert_eq!(fs::read_dir(&directory).unwrap().count(), 9, "still there");
}

#[cfg(unix)]
mod files;

#[test]
fn removing_goes_meta_first_and_what_has_gone_is_no_failure() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    write_copy(&directory, STEM, &scratch.0.join("a"), "text", None);
    let text = directory.join(format!("{STEM}.txt"));
    let meta = directory.join(format!("{STEM}.meta"));
    remove_copy(&text, Some(&meta)).unwrap();
    assert!(!text.exists() && !meta.exists());
    remove_copy(&text, Some(&meta)).unwrap();
}

#[test]
fn a_failure_removing_a_meta_keeps_the_text_so_that_nothing_is_lost() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let text = directory.join("t");
    fs::write(&text, "text").unwrap();
    // A directory cannot be removed as a file.
    let meta = directory.join("m");
    fs::create_dir(&meta).unwrap();
    assert!(remove_copy(&text, Some(&meta)).is_err());
    assert!(text.exists());
}

#[test]
fn the_newest_come_first_and_past_the_limit_a_row_says_how_many_more() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    for index in 0..MAX_ROWS + 2 {
        let stem = format!("00000000000000aa-{}-77-0", 1_000 + index);
        let meta = format!(
            "oso-emergency-copy 1\nname n{index}\nwithheld false\nbytes 0\nwritten {}\n",
            1_000 + index
        );
        fs::write(directory.join(format!("{stem}.meta")), meta).unwrap();
        fs::write(directory.join(format!("{stem}.txt")), "").unwrap();
    }
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(found.rows.len(), MAX_ROWS);
    assert_eq!(found.rows[0].name, format!("n{}", MAX_ROWS + 1));
    let list = found.list.text().to_string();
    assert!(list.starts_with(&format!("{} emergency copies.", MAX_ROWS + 2)));
    assert!(list.contains("and 2 more\n"));
}

#[test]
fn a_row_escapes_a_name_stays_one_row_and_is_cut() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let meta = format!(
        "oso-emergency-copy 1\nname a\nbytes 0\npath /a%0Ab%1B[2J%E2%80%AE{}\n",
        "x".repeat(4000)
    );
    fs::write(directory.join(format!("{STEM}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "").unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    let list = found.list.text().to_string();
    assert_eq!(list.lines().count(), HEADER_ROWS + 1, "{list}");
    let row = list.lines().last().unwrap();
    assert!(row.contains("/a\\nb\\u{1b}[2J\\u{202e}"), "{row}");
    assert!(
        row.ends_with('…') && row.len() < MAX_SHOWN + 100,
        "{}",
        row.len()
    );
}

#[test]
fn ages_read_for_a_person() {
    let at = |ago: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + ago);
    assert_eq!(age(Some(1_000_000), at(5)), "just now");
    assert_eq!(age(Some(1_000_000), at(720)), "12 min ago");
    assert_eq!(age(Some(1_000_000), at(3600)), "1 hour ago");
    assert_eq!(age(Some(1_000_000), at(7200)), "2 hours ago");
    assert_eq!(age(Some(1_000_000), at(200_000)), "2 days ago");
    assert_eq!(age(Some(2_000_000), at(0)), "in the future");
    assert_eq!(age(Some(u64::MAX), at(0)), "in the future");
    assert_eq!(age(None, at(0)), "?");
}

#[test]
fn leftovers_stop_at_the_limit_and_copies_come_first_for_it() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let project = scratch.0.join("project");
    fs::create_dir(&project).unwrap();
    let file = project.join("notes.txt");
    fs::write(&file, "text").unwrap();
    for index in 0..MAX_ROWS + 5 {
        fs::write(project.join(format!(".oso-save-77-{index}-0.tmp")), "").unwrap();
    }
    write_copy(&directory, STEM, &scratch.0.join("a"), "text", None);
    let found = scan(&request(&directory, vec![target_for(&file)]), later());
    assert_eq!(found.rows.len(), MAX_ROWS);
    assert!(
        found.rows.iter().any(|row| row.state == RowState::NotOpen),
        "the copy"
    );
    let list = found.list.text().to_string();
    assert!(
        list.starts_with(&format!("{} emergency copies.", MAX_ROWS + 6)),
        "all counted"
    );
    assert!(list.contains("and 6 more\n"));
}

#[test]
fn a_copy_withheld_when_it_was_written_stays_withheld_with_no_buffer_to_ask() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let meta = "oso-emergency-copy 1\nname a\npath /a/b.txt\nwithheld true\nbytes 0\n";
    fs::write(directory.join(format!("{STEM}.meta")), meta).unwrap();
    fs::write(directory.join(format!("{STEM}.txt")), "").unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(found.rows[0].at_least, Verdict::Unresolved);
    let says_nothing = "oso-emergency-copy 1\nname a\npath /a/b.txt\nbytes 0\n";
    fs::write(directory.join(format!("{STEM}.meta")), says_nothing).unwrap();
    let found = scan(&request(&directory, Vec::new()), later());
    assert_eq!(found.rows[0].at_least, Verdict::Unresolved, "fails closed");
}

#[test]
fn a_copy_finds_its_file_by_the_files_own_spelling() {
    let scratch = Scratch::new();
    let directory = copies_of(&scratch);
    let file = scratch.0.join("Notes.txt");
    fs::write(&file, "disk").unwrap();
    let mut buffer = Buffer::visiting(scratch.0.join("NOTES.txt"));
    buffer.finish_empty();
    buffer.judge_secret(Some(std::slice::from_ref(&file)));
    buffer.set_baseline(Baseline {
        location: scratch.0.join("NOTES.txt"),
        file: fs::metadata(&file).ok().map(|m| stamp_of(&m).unwrap()),
    });
    buffer.set_own_spelling(file.clone());
    let targets = targets_of(&Editor::with_buffer(buffer));
    write_copy(&directory, STEM, &file, "a copy", None);
    let found = scan(&request(&directory, targets), later());
    assert!(found.rows[0].target.is_some(), "{:?}", words(&found));
}
