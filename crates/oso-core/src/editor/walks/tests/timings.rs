//! Timings of a project's listing, run by hand in a release build (ADR 0114): the UI thread's
//! share of a batch, up to 5000 rows and past them, in the worst cases tried.
//!
//! Must not depend on: threads or the file system.

use super::*;

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
                editor
                    .open_project_files(root, &name, "project-file")
                    .unwrap();
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
