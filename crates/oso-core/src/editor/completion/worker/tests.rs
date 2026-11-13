//! Tests of completion on a worker past 5000 candidates (ADR 0096), each job run on the
//! test's thread, with listings made up.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::sync::Arc;

use crate::complete::{Name, NameKind};
use crate::editor::edit_tests::edit;
use crate::editor::{Editor, Looked};
use crate::migemo::Dictionary;

/// A listing of `count` files, `file-0000.txt` and on.
fn files(count: usize) -> Option<Arc<[Name]>> {
    Some(
        (0..count)
            .map(|index| Name {
                name: format!("file-{index:04}.txt"),
                kind: NameKind::File,
            })
            .collect(),
    )
}

/// An editor with a prompt for a path open at `/big/`, listing `count` files, and `/small`
/// beside it with two.
fn prompt(count: usize) -> Editor {
    let mut editor = Editor::default();
    editor
        .open_file_prompt("Find file:", "file", "/big/", PathBuf::from("/"))
        .unwrap();
    editor.take_file_requests();
    let small = Some(Arc::from(
        [
            Name {
                name: "a.md".to_owned(),
                kind: NameKind::File,
            },
            Name {
                name: "b.md".to_owned(),
                kind: NameKind::File,
            },
        ]
        .as_slice(),
    ));
    let mut root: Vec<Name> = vec![
        Name {
            name: "big".to_owned(),
            kind: NameKind::Directory,
        },
        Name {
            name: "small".to_owned(),
            kind: NameKind::Directory,
        },
    ];
    root.sort_by(|a, b| a.name.cmp(&b.name));
    editor.file_listed(PathBuf::from("/"), Some(root.into()), false);
    editor.file_listed(PathBuf::from("/small"), small, false);
    editor.file_listed(PathBuf::from("/big"), files(count), false);
    editor
}

/// Runs every job left, as the worker would, and hands each report back.
fn work(editor: &mut Editor) {
    for _ in 0..8 {
        let Some(job) = editor.take_completion_job() else {
            return;
        };
        if let Some(report) = job.run() {
            editor.completion_reported(report);
        }
    }
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

fn deleting(editor: &mut Editor, bytes: usize) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor
        .apply_edits(id, &[edit(end - bytes..end, "")])
        .unwrap();
    editor.match_completion();
}

fn count(editor: &Editor) -> usize {
    editor.prompt().unwrap().candidates().unwrap().len()
}

fn first(editor: &Editor) -> String {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    candidates
        .get(0)
        .map(|c| c.text().to_owned())
        .unwrap_or_default()
}

#[test]
fn a_list_past_5000_is_built_on_the_worker_and_shows_once_it_comes() {
    let mut editor = prompt(6000);
    assert!(editor.completion_pending(), "left for the worker");
    assert!(editor.prompt().unwrap().candidates().unwrap().pending());
    assert_eq!(count(&editor), 0, "nothing built on the UI thread");
    work(&mut editor);
    assert!(!editor.completion_pending());
    assert!(!editor.prompt().unwrap().candidates().unwrap().pending());
    assert_eq!(count(&editor), 6000);
    assert_eq!(first(&editor), "file-0000.txt");
}

#[test]
fn a_list_of_at_most_5000_takes_no_job() {
    let mut editor = prompt(5000);
    assert!(!editor.completion_pending());
    assert!(editor.take_completion_job().is_none());
    assert_eq!(count(&editor), 5000);
    typing(&mut editor, "0042");
    assert!(editor.take_completion_job().is_none());
    assert_eq!(count(&editor), 1);
}

#[test]
fn a_key_past_5000_leaves_a_match_and_the_rows_shown_stay_until_it_comes() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "0042");
    assert!(editor.completion_pending());
    assert_eq!(count(&editor), 6000, "the rows of the input before stay");
    work(&mut editor);
    assert_eq!(count(&editor), 1);
    assert_eq!(first(&editor), "file-0042.txt");
}

#[test]
fn a_superseded_job_stops_and_an_old_report_is_dropped() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "1");
    let older = editor.take_completion_job().unwrap();
    let report = older.run().expect("it was the live one");
    typing(&mut editor, "2");
    assert!(
        !editor.completion_reported(report),
        "a newer job took its place"
    );
    let newer = editor.take_completion_job().unwrap();
    typing(&mut editor, "3");
    assert!(newer.run().is_none(), "it stops once superseded");
    work(&mut editor);
    // `123` of the newest: x123 for x of 0 to 5, and 1230 to 1239.
    assert_eq!(count(&editor), 16);
}

#[test]
fn going_back_to_the_input_matched_last_drops_the_job_between() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "7");
    let job = editor.take_completion_job().unwrap();
    deleting(&mut editor, 1);
    assert!(
        !editor.completion_pending(),
        "the list shown is the one for this input"
    );
    assert!(job.run().is_none());
    assert_eq!(count(&editor), 6000);
}

#[test]
fn a_report_for_a_prompt_that_closed_is_dropped() {
    let mut editor = prompt(6000);
    let job = editor.take_completion_job().unwrap();
    editor.quit();
    assert!(editor.prompt().is_none());
    assert!(job.run().is_none(), "closing stops it");
    assert!(editor.take_completion_job().is_none());
}

#[test]
fn what_is_typed_while_the_list_is_built_is_matched_once_it_comes() {
    let mut editor = prompt(6000);
    typing(&mut editor, "0042");
    let build = editor.take_completion_job().unwrap();
    assert!(
        editor.take_completion_job().is_none(),
        "the build takes it in"
    );
    let report = build.run().unwrap();
    assert!(editor.completion_reported(report));
    assert!(
        editor.completion_pending(),
        "now a match of the newer input"
    );
    work(&mut editor);
    assert_eq!(first(&editor), "file-0042.txt");
}

#[test]
fn the_5000_line_is_crossed_both_ways_as_the_path_changes() {
    let mut editor = prompt(6000);
    work(&mut editor);
    deleting(&mut editor, "big/".len());
    work(&mut editor);
    typing(&mut editor, "small/");
    assert!(!editor.completion_pending(), "two names are built at once");
    assert_eq!(count(&editor), 2);
    deleting(&mut editor, "small/".len());
    work(&mut editor);
    typing(&mut editor, "big/");
    assert!(editor.completion_pending());
    work(&mut editor);
    assert_eq!(count(&editor), 6000);
}

#[test]
fn a_look_patches_its_row_without_a_job() {
    let mut editor = prompt(6000);
    work(&mut editor);
    let asked = editor.take_file_looks();
    assert_eq!(asked[0].0, PathBuf::from("/big"));
    let looked = Looked {
        annotation: "-rw-r--r--  12".to_owned(),
        bytes: 12,
        directory: false,
    };
    editor.file_looked(
        std::path::Path::new("/big"),
        vec![("file-0000.txt".to_owned(), looked)],
    );
    assert!(editor.take_completion_job().is_none(), "no build again");
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.get(0).unwrap().annotation(), "-rw-r--r--  12");
    assert_eq!(candidates.len(), 6000);
}

#[test]
fn a_prompt_covered_while_its_job_ran_is_matched_again_once_it_is_innermost() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "0042");
    let job = editor.take_completion_job().unwrap();
    editor.ask("Go on?", "keyboard-quit").unwrap();
    let report = job.run().unwrap();
    assert!(!editor.completion_reported(report), "not the innermost");
    editor.quit();
    assert!(
        editor.prompt().unwrap().candidates().is_some(),
        "back to it"
    );
    editor.match_completion();
    assert!(editor.completion_pending(), "asked for again");
    work(&mut editor);
    assert!(!editor.completion_pending());
    assert_eq!(first(&editor), "file-0042.txt");
}

#[test]
fn a_path_that_needs_a_listing_stops_the_build_for_the_path_before() {
    let mut editor = prompt(6000);
    let build = editor.take_completion_job().unwrap();
    typing(&mut editor, "x/");
    assert!(
        !editor.completion_pending(),
        "nothing pending is this path's"
    );
    assert!(build.run().is_none(), "the build for /big/ stops");
}

#[test]
fn a_candidate_moved_to_while_the_match_was_on_its_way_stays_selected() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "00");
    editor.move_completion(2).unwrap();
    let chosen = editor
        .prompt()
        .unwrap()
        .candidates()
        .unwrap()
        .selected_candidate()
        .map(|c| c.text().to_owned());
    assert_eq!(chosen.as_deref(), Some("file-0002.txt"));
    work(&mut editor);
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(
        candidates.selected_candidate().map(|c| c.text()),
        Some("file-0002.txt"),
        "it still matches 00"
    );
}

#[test]
fn a_list_replaced_or_closed_is_freed_by_a_job_of_its_own() {
    let mut editor = prompt(6000);
    work(&mut editor);
    deleting(&mut editor, "big/".len());
    let jobs: Vec<_> = std::iter::from_fn(|| editor.take_completion_job()).collect();
    assert!(jobs.iter().any(crate::complete::CompletionJob::frees_only));
    assert!(jobs.into_iter().all(|job| job.run().is_none()));
    typing(&mut editor, "big/");
    work(&mut editor);
    editor.quit();
    let job = editor
        .take_completion_job()
        .expect("the list goes on the worker");
    assert!(job.frees_only());
}

#[test]
fn a_job_dropped_before_the_host_took_it_is_freed_on_the_worker() {
    let mut editor = prompt(6000);
    work(&mut editor);
    let old = Arc::new(Dictionary::new("か\t蚊\n".to_owned()));
    let kept = Arc::downgrade(&old);
    editor.set_migemo_dictionary(Some(old));
    typing(&mut editor, "%ka");
    // Once another is given, the job for `%ka` holds the last copy, and the next key drops it.
    editor.set_migemo_dictionary(Some(Arc::new(Dictionary::new("き\t木\n".to_owned()))));
    typing(&mut editor, "a");
    assert!(
        kept.upgrade().is_some(),
        "the dropped job is not freed here"
    );
    let jobs: Vec<_> = std::iter::from_fn(|| editor.take_completion_job()).collect();
    // The new match first, so it waits for no drop.
    assert!(!jobs[0].frees_only() && jobs[1].frees_only());
    jobs.into_iter().for_each(|job| drop(job.run()));
    assert!(kept.upgrade().is_none());
}

#[test]
fn neither_a_job_nor_a_report_says_what_it_lists_or_what_is_typed() {
    let mut editor = prompt(6000);
    work(&mut editor);
    typing(&mut editor, "0042");
    let job = editor.take_completion_job().unwrap();
    let shown = format!("{job:?}");
    let report = job.run().unwrap();
    let shown = format!("{shown} {report:?}");
    assert!(
        !shown.contains("0042") && !shown.contains("file-"),
        "{shown}"
    );
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn the_ui_threads_share_of_a_key_at_100000_names_is_under_a_millisecond() {
    use std::time::{Duration, Instant};
    let mut editor = prompt(100_000);
    work(&mut editor);
    let mut slowest = Duration::ZERO;
    // Each keeps most names matching, then `DEL` goes back to them all.
    for key in ["f", "i", "l", "e", ".", "*", "9"] {
        let started = Instant::now();
        typing(&mut editor, key);
        let job = editor.take_completion_job().unwrap();
        let mut share = started.elapsed();
        let report = job.run().unwrap();
        let started = Instant::now();
        editor.completion_reported(report);
        share += started.elapsed();
        slowest = slowest.max(share);
    }
    assert!(slowest < Duration::from_millis(1), "{slowest:?}");
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn taking_in_a_list_of_100000_in_place_of_another_is_under_a_millisecond() {
    use std::time::{Duration, Instant};
    let mut editor = prompt(100_000);
    work(&mut editor);
    let other: Arc<[Name]> = (0..100_000)
        .map(|index| Name {
            name: format!("ファイル-{index:05}.txt"),
            kind: NameKind::File,
        })
        .collect();
    let root: Arc<[Name]> = ["big", "other", "small"]
        .map(|name| Name {
            name: name.to_owned(),
            kind: NameKind::Directory,
        })
        .into();
    editor.file_listed(PathBuf::from("/"), Some(root), false);
    let mut slowest = Duration::ZERO;
    let mut built = 0;
    for _ in 0..4 {
        // Into `/other/`, a directory of its own, and back into `/big/`, each list taking the
        // place of the other's.
        for (to, from) in [("other/", "big/"), ("big/", "other/")] {
            deleting(&mut editor, from.len());
            work(&mut editor);
            let started = Instant::now();
            typing(&mut editor, to);
            let mut share = started.elapsed();
            if to == "other/" {
                editor.file_listed(PathBuf::from("/other"), Some(Arc::clone(&other)), false);
            }
            let jobs: Vec<_> = std::iter::from_fn(|| editor.take_completion_job()).collect();
            for job in jobs {
                if let Some(report) = job.run() {
                    let started = Instant::now();
                    editor.completion_reported(report);
                    share += started.elapsed();
                    built += 1;
                }
            }
            assert_eq!(count(&editor), 100_000, "{to}");
            slowest = slowest.max(share);
        }
    }
    assert_eq!(built, 8, "each list built on the worker");
    assert!(slowest < Duration::from_millis(1), "{slowest:?}");
}
