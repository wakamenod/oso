//! Tests of a project's listing (ADR 0114): batches merged as they come, on the UI thread and
//! by jobs run on the test's thread past 5000, the order, the selection, stale walks and what
//! `RET` gives.
//!
//! Must not depend on: threads or the file system.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use super::{NotInProject, WalkEnd};
use crate::buffer::{Baseline, Buffer};
use crate::complete::WORKER_PAST;
use crate::editor::Editor;
use crate::editor::edit_tests::edit;
use crate::minibuffer::Outcome;
use crate::project::{Found, RootIdentity};

/// A buffer visiting `path`, whose path was found to lead there.
fn visited(path: &str) -> Buffer {
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.set_baseline(Baseline {
        location: PathBuf::from(path),
        file: None,
    });
    buffer.judge_secret(Some(&[PathBuf::from(path)]));
    buffer
}

/// An editor showing `/w/oso/src/main.rs`, in the project at `/w/oso`.
fn in_project() -> Editor {
    let mut editor = Editor::with_buffer(visited("/w/oso/src/main.rs"));
    let id = editor.tiled().0.buffer();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/w/oso"),
            identity: Some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.set_buffer_project(id, Some(found));
    editor
}

/// An editor with the listing of `/w/oso` open, and the generation of its walk.
fn listing() -> (Editor, u64) {
    let mut editor = in_project();
    let (root, name) = editor.project_files_root().unwrap();
    editor.open_project_files(root, &name).unwrap();
    let request = editor.take_walk_request().unwrap();
    (editor, request.generation)
}

fn paths(names: &[&str]) -> Vec<Box<str>> {
    names.iter().map(|&name| name.into()).collect()
}

/// `count` paths, `dir/file-00000.rs` and on, after `from`.
fn many(from: usize, count: usize) -> Vec<Box<str>> {
    (from..from + count)
        .map(|index| format!("dir/file-{index:05}.rs").into())
        .collect()
}

/// The rows the input matches, in the order they show.
fn rows(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

fn selected(editor: &Editor) -> Option<String> {
    let candidates = editor.prompt()?.candidates()?;
    Some(candidates.selected_candidate()?.text().to_owned())
}

fn total(editor: &Editor) -> usize {
    editor.prompt().unwrap().candidates().unwrap().total()
}

fn typing(editor: &mut Editor, text: &str) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    editor.match_completion();
}

/// Runs every job left, as the worker would, and hands each report back.
fn work(editor: &mut Editor) {
    for _ in 0..16 {
        let Some(job) = editor.take_completion_job() else {
            return;
        };
        if let Some(report) = job.run() {
            editor.completion_reported(report);
        }
    }
}

#[test]
fn the_root_is_the_projects_else_the_files_directory_but_never_home_or_the_top() {
    let editor = in_project();
    assert_eq!(
        editor.project_files_root(),
        Ok((PathBuf::from("/w/oso"), "oso".to_owned()))
    );
    let loose = Editor::with_buffer(visited("/w/notes/todo.md"));
    assert_eq!(
        loose.project_files_root(),
        Ok((PathBuf::from("/w/notes"), "notes".to_owned()))
    );
    let mut home = Editor::with_buffer(visited("/home/me/todo.md"));
    home.set_home(Some(PathBuf::from("/home/me")));
    assert_eq!(home.project_files_root(), Err(NotInProject));
    let top = Editor::with_buffer(visited("/todo.md"));
    assert_eq!(top.project_files_root(), Err(NotInProject));
    let mut above = Editor::with_buffer(visited("/home/x.txt"));
    above.set_home(Some(PathBuf::from("/home/me")));
    assert_eq!(
        above.project_files_root(),
        Err(NotInProject),
        "it holds home"
    );
    let unloaded = Editor::with_buffer(Buffer::visiting(PathBuf::from("/w/notes/a.md")));
    assert_eq!(
        unloaded.project_files_root(),
        Err(NotInProject),
        "not found yet"
    );
    assert_eq!(Editor::default().project_files_root(), Err(NotInProject));
    assert_eq!(NotInProject.to_string(), "Not in a project");
}

#[test]
fn the_listing_opens_empty_and_pending_and_asks_for_one_walk() {
    let mut editor = in_project();
    let id = editor
        .open_project_files(PathBuf::from("/w/oso"), "oso")
        .unwrap();
    assert_eq!(editor.prompt().unwrap().id(), id);
    assert_eq!(editor.prompt().unwrap().label(), "Find file in oso:");
    assert_eq!(total(&editor), 0);
    assert!(editor.files_pending(), "the count shows …");
    let request = editor.take_walk_request().unwrap();
    assert_eq!(request.root, PathBuf::from("/w/oso"));
    assert_eq!(request.live.load(Ordering::Relaxed), request.generation);
    assert!(editor.take_walk_request().is_none(), "asked once");
}

#[test]
fn batches_merge_in_the_order_of_history_then_length_then_name() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs", "README.md"]), None);
    assert_eq!(rows(&editor), ["README.md", "src/lib.rs"]);
    editor.files_walked(
        generation,
        paths(&["a.rs", "src/zz.rs", "Cargo.toml"]),
        None,
    );
    assert_eq!(
        rows(&editor),
        ["a.rs", "README.md", "src/zz.rs", "Cargo.toml", "src/lib.rs"]
    );
    assert!(editor.files_pending(), "the walk has not ended");
    editor.files_walked(generation, Vec::new(), Some(WalkEnd { cut: false }));
    assert!(!editor.files_pending());
    assert!(!editor.files_cut());
}

#[test]
fn a_batch_matches_only_its_own_paths_against_the_input() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs", "docs/ui.md"]), None);
    typing(&mut editor, "rs");
    assert_eq!(rows(&editor), ["src/lib.rs"]);
    editor.files_walked(generation, paths(&["a.rs", "b.md", "crates/x.rs"]), None);
    assert_eq!(rows(&editor), ["a.rs", "src/lib.rs", "crates/x.rs"]);
    assert_eq!(total(&editor), 5);
}

#[test]
fn the_selected_row_stays_selected_as_rows_come() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs", "src/main.rs"]), None);
    assert_eq!(selected(&editor).as_deref(), Some("src/lib.rs"));
    editor.move_completion(1).unwrap();
    assert_eq!(selected(&editor).as_deref(), Some("src/main.rs"));
    editor.files_walked(generation, paths(&["a.rs", "b.rs"]), None);
    assert_eq!(selected(&editor).as_deref(), Some("src/main.rs"));
    // Unmoved, the first row stays too: rows coming before it never change it.
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs"]), None);
    editor.files_walked(generation, paths(&["a.rs"]), None);
    assert_eq!(selected(&editor).as_deref(), Some("src/lib.rs"));
}

#[test]
fn a_stale_walk_and_a_closed_prompt_are_dropped_and_stop_the_walker() {
    let (mut editor, old) = listing();
    editor.quit();
    let (root, name) = editor.project_files_root().unwrap();
    editor.open_project_files(root, &name).unwrap();
    let request = editor.take_walk_request().unwrap();
    assert!(request.generation > old);
    editor.files_walked(old, paths(&["stale.rs"]), None);
    assert_eq!(
        total(&editor),
        0,
        "a walk not the newest listing's is dropped"
    );
    editor.files_walked(request.generation, paths(&["new.rs"]), None);
    assert_eq!(rows(&editor), ["new.rs"]);
    // Closing moves the live generation on, so the walker stops at its next directory.
    editor.quit();
    assert_ne!(request.live.load(Ordering::Relaxed), request.generation);
    editor.files_walked(request.generation, paths(&["late.rs"]), None);
    assert!(editor.prompt().is_none());
}

#[test]
fn a_listing_is_refused_while_one_is_open_and_a_new_one_stops_the_walk_before() {
    let (mut editor, _) = listing();
    let first = {
        editor.quit();
        let (root, name) = editor.project_files_root().unwrap();
        editor.open_project_files(root, &name).unwrap();
        editor.take_walk_request().unwrap()
    };
    // A listing over it, as from a prompt that covers it, would leave it without its own.
    assert_eq!(
        editor.open_project_files(PathBuf::from("/w/oso"), "oso"),
        Err(crate::minibuffer::PromptError::Invalid)
    );
    assert_eq!(first.live.load(Ordering::Relaxed), first.generation);
    editor.quit();
    editor
        .open_project_files(PathBuf::from("/w/oso"), "oso")
        .unwrap();
    let second = editor.take_walk_request().unwrap();
    assert_ne!(first.live.load(Ordering::Relaxed), first.generation);
    assert_eq!(second.live.load(Ordering::Relaxed), second.generation);
}

#[test]
fn ret_gives_the_whole_path_and_keeps_the_projects_own_history() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs", "Cargo.toml"]), None);
    assert_eq!(rows(&editor), ["Cargo.toml", "src/lib.rs"]);
    editor.move_completion(1).unwrap();
    editor.move_completion(-1).unwrap();
    editor.submit_prompt().unwrap();
    let Some(Outcome::Answered { text, .. }) = editor.take_prompt_outcome() else {
        panic!("answered");
    };
    assert_eq!(
        PathBuf::from(text),
        PathBuf::from("/w/oso").join("Cargo.toml")
    );
    let histories: Vec<(String, Vec<String>)> = editor
        .histories()
        .map(|(name, entries)| (name.to_owned(), entries.map(str::to_owned).collect()))
        .collect();
    assert_eq!(
        histories,
        [(
            "project-file:/w/oso".to_owned(),
            vec!["Cargo.toml".to_owned()]
        )]
    );
    // The next listing of it puts that file first; another project's does not.
    let mut editor2 = editor;
    let (root, name) = editor2.project_files_root().unwrap();
    editor2.open_project_files(root, &name).unwrap();
    let generation = editor2.take_walk_request().unwrap().generation;
    editor2.files_walked(generation, paths(&["a.rs", "Cargo.toml"]), None);
    assert_eq!(rows(&editor2), ["Cargo.toml", "a.rs"]);
    editor2.quit();
    editor2
        .open_project_files(PathBuf::from("/w/other"), "other")
        .unwrap();
    let generation = editor2.take_walk_request().unwrap().generation;
    editor2.files_walked(generation, paths(&["a.rs", "Cargo.toml"]), None);
    assert_eq!(rows(&editor2), ["a.rs", "Cargo.toml"]);
}

#[test]
fn ret_takes_the_selected_row_while_the_walk_goes_on() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["src/lib.rs"]), None);
    assert!(editor.files_pending());
    assert!(!editor.completion_pending(), "RET is not held");
    let submitted = editor.submit_prompt();
    assert_eq!(
        submitted,
        Ok(()),
        "{:?}",
        editor.prompt().and_then(|p| p.message())
    );
    assert!(matches!(
        editor.take_prompt_outcome(),
        Some(Outcome::Answered { .. })
    ));
}

#[test]
fn ret_with_no_row_says_no_match() {
    let (mut editor, _) = listing();
    assert!(editor.submit_prompt().is_err());
    assert!(editor.prompt().is_some());
}

#[test]
fn the_count_says_plus_once_a_walk_stops_at_its_limit() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["a.rs"]), Some(WalkEnd { cut: true }));
    assert!(editor.files_cut());
    assert!(!editor.files_pending());
}

#[test]
fn a_large_batch_goes_to_the_worker_even_under_5000() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, 1000), None);
    assert_eq!(total(&editor), 1000, "a batch of 1000 is merged here");
    editor.files_walked(generation, many(1000, 1001), None);
    assert_eq!(total(&editor), 1000, "a larger one is left for the worker");
    assert!(
        !editor.completion_pending(),
        "RET need not wait for rows to be added"
    );
    work(&mut editor);
    assert_eq!(total(&editor), 2001);
    assert_eq!(rows(&editor)[0], "dir/file-00000.rs");
}

#[test]
fn past_5000_batches_wait_for_an_eighth_of_the_list_or_the_walks_end() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, 8000), None);
    work(&mut editor);
    assert_eq!(total(&editor), 8000);
    editor.files_walked(generation, many(8000, 500), None);
    assert!(
        editor.take_completion_job().is_none(),
        "500 is under an eighth"
    );
    editor.files_walked(generation, many(8500, 500), None);
    work(&mut editor);
    assert_eq!(total(&editor), 9000, "1000 is an eighth");
    editor.files_walked(generation, many(9000, 10), Some(WalkEnd { cut: false }));
    work(&mut editor);
    assert_eq!(total(&editor), 9010, "the end takes in what waits");
    assert_eq!(rows(&editor).last().unwrap(), "dir/file-09009.rs");
    assert!(!editor.files_pending());
}

#[test]
fn past_5000_a_batch_that_comes_while_one_is_added_waits_for_the_next_job() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, WORKER_PAST + 1), None);
    let job = editor.take_completion_job().unwrap();
    editor.files_walked(generation, many(WORKER_PAST + 1, 5), None);
    assert!(
        editor.take_completion_job().is_none(),
        "the job running is not made to start over"
    );
    let report = job.run().unwrap();
    editor.completion_reported(report);
    assert_eq!(total(&editor), WORKER_PAST + 1);
    editor.files_walked(generation, Vec::new(), Some(WalkEnd { cut: false }));
    work(&mut editor);
    assert_eq!(total(&editor), WORKER_PAST + 6);
    assert!(!editor.files_pending());
}

#[test]
fn past_5000_typing_while_batches_are_added_matches_them_all() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, WORKER_PAST + 1), None);
    typing(&mut editor, "file-0500");
    assert!(
        editor.completion_pending(),
        "RET waits for the input's rows"
    );
    editor.files_walked(generation, many(WORKER_PAST + 1, 10), None);
    // A key takes in every batch waiting, with the input.
    typing(&mut editor, "");
    work(&mut editor);
    typing(&mut editor, "1");
    work(&mut editor);
    assert!(!editor.completion_pending());
    assert_eq!(total(&editor), WORKER_PAST + 11);
    assert_eq!(rows(&editor), ["dir/file-05001.rs"]);
    // A batch that comes after matches only its own paths.
    editor.files_walked(generation, paths(&["file-05001-new.rs"]), None);
    editor.files_walked(generation, Vec::new(), Some(WalkEnd { cut: false }));
    work(&mut editor);
    assert_eq!(rows(&editor), ["dir/file-05001.rs", "file-05001-new.rs"]);
}

#[test]
fn past_5000_a_moved_to_row_stays_selected_as_rows_come() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(10, WORKER_PAST + 1), None);
    work(&mut editor);
    editor.move_completion(3).unwrap();
    let before = selected(&editor);
    editor.files_walked(generation, many(0, 10), Some(WalkEnd { cut: false }));
    work(&mut editor);
    assert_eq!(selected(&editor), before);
    assert_eq!(rows(&editor)[0], "dir/file-00000.rs");
}

#[test]
fn a_merge_job_that_fails_leaves_its_batches_for_the_next() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, 2000), None);
    let _lost = editor.take_completion_job().unwrap();
    editor.completion_failed();
    work(&mut editor);
    assert_eq!(total(&editor), 2000);
}

#[test]
fn an_exact_match_that_comes_later_is_selected_unless_a_row_was_moved_to() {
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["a.rs", "src/a.rs"]), None);
    typing(&mut editor, "a.r");
    assert_eq!(selected(&editor).as_deref(), Some("a.rs"));
    editor.files_walked(generation, paths(&["a.r"]), None);
    assert_eq!(selected(&editor).as_deref(), Some("a.r"));
    let (mut editor, generation) = listing();
    editor.files_walked(generation, paths(&["a.rs", "src/a.rs"]), None);
    typing(&mut editor, "a.r");
    editor.move_completion(1).unwrap();
    editor.files_walked(generation, paths(&["a.r"]), None);
    assert_eq!(selected(&editor).as_deref(), Some("src/a.rs"));
}

#[test]
fn a_listing_beside_a_secret_reads_and_records_no_history() {
    let mut editor = in_project();
    editor.histories.record("project-file:/w/oso", "Cargo.toml");
    let secret = editor.add_buffer(crate::editor::test_buffers::secret_buffer());
    editor.show(secret);
    editor
        .open_project_files(PathBuf::from("/w/oso"), "oso")
        .unwrap();
    let generation = editor.take_walk_request().unwrap().generation;
    editor.files_walked(generation, paths(&["a.rs", "Cargo.toml"]), None);
    assert_eq!(
        rows(&editor),
        ["a.rs", "Cargo.toml"],
        "no history orders it"
    );
    editor.move_completion(1).unwrap();
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor
            .histories
            .entries("project-file:/w/oso")
            .collect::<Vec<_>>(),
        ["Cargo.toml"],
        "nothing recorded"
    );
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn the_ui_threads_share_of_a_batch_up_to_5000_rows_is_under_a_millisecond() {
    use std::time::{Duration, Instant};
    let mut slowest = Duration::ZERO;
    for input in ["", "rs", "f.*9"] {
        let (mut editor, generation) = listing();
        typing(&mut editor, input);
        // Batches of 250, as a walk of a fast disk sends every 50 ms, up to the line.
        for from in (0..WORKER_PAST).step_by(250) {
            let batch = many(from, 250);
            let started = Instant::now();
            editor.files_walked(generation, batch, None);
            slowest = slowest.max(started.elapsed());
        }
        assert_eq!(total(&editor), WORKER_PAST);
    }
    assert!(slowest < Duration::from_millis(1), "{slowest:?}");
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn the_ui_threads_share_of_a_batch_past_5000_rows_is_under_a_millisecond() {
    use std::time::{Duration, Instant};
    let (mut editor, generation) = listing();
    editor.files_walked(generation, many(0, 90_000), None);
    work(&mut editor);
    let mut slowest = Duration::ZERO;
    for from in (90_000..100_000).step_by(1000) {
        let started = Instant::now();
        let end = (from + 1000 == 100_000).then_some(WalkEnd { cut: false });
        editor.files_walked(generation, many(from, 1000), end);
        let mut share = started.elapsed();
        // Batches wait for an eighth of the list, and the end takes in the rest.
        while let Some(job) = editor.take_completion_job() {
            if let Some(report) = job.run() {
                let started = Instant::now();
                editor.completion_reported(report);
                share += started.elapsed();
            }
        }
        slowest = slowest.max(share);
    }
    assert_eq!(total(&editor), 100_000);
    assert!(slowest < Duration::from_millis(1), "{slowest:?}");
}

/// `count` paths in a scrambled order, mixed case or Japanese and accented, for the worst case.
fn scrambled(count: usize, japanese: bool) -> Vec<Box<str>> {
    (0..count)
        .map(|index| {
            let scrambled = (index * 7919) % count;
            if japanese {
                format!("資料/ファイル-é{scrambled:05}.md").into()
            } else {
                format!("src/Module{}/BarBaz{scrambled:05}.java", scrambled % 37).into()
            }
        })
        .collect()
}

#[test]
#[ignore = "a timing: run by hand in a release build"]
fn the_ui_threads_share_of_a_large_batch_with_history_is_under_a_millisecond() {
    use std::time::{Duration, Instant};
    let mut report = Vec::new();
    for japanese in [false, true] {
        for input in ["", "rs", "Bar 1"] {
            for split in [1, 2] {
                let mut editor = in_project();
                // A project history of 100 files listed before.
                for path in scrambled(5000, japanese).iter().take(100) {
                    editor.histories.record("project-file:/w/oso", path);
                }
                let (root, name) = editor.project_files_root().unwrap();
                editor.open_project_files(root, &name).unwrap();
                let generation = editor.take_walk_request().unwrap().generation;
                // 4000 rows in first, by the worker, then the largest batch merged here.
                editor.files_walked(generation, scrambled(4000, !japanese), None);
                work(&mut editor);
                typing(&mut editor, input);
                let all = scrambled(999, japanese);
                let mut slowest = Duration::ZERO;
                for part in all.chunks(999 / split + 1) {
                    let started = Instant::now();
                    editor.files_walked(generation, part.to_vec(), None);
                    slowest = slowest.max(started.elapsed());
                }
                report.push(format!(
                    "japanese={japanese} input={input:?} batches={split}: {slowest:?}"
                ));
            }
        }
    }
    // Each line names its case and the slowest batch, as the failure shows them.
    assert!(
        report
            .iter()
            .all(|line| line.ends_with("µs") || line.ends_with("ns")),
        "{report:#?}"
    );
}
