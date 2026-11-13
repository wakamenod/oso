//! Measures of migemo in a project's search and in completion (ADR 0116, step 4), ignored by
//! default, for the ADR's notes: what a key costs on the UI thread as it parses the input, a
//! migemo component compiled with the dictionary a person names, and a paste of several. The
//! dictionary is read from the file `OSO_MIGEMO_DICTIONARY` names; only these measures read a
//! file, and they print no word of it. Run in a release build on an idle machine:
//! `OSO_MIGEMO_DICTIONARY=… cargo test -p oso-core --release --lib migemo_prompt_costs -- --ignored --nocapture --test-threads 1`.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

#![expect(clippy::disallowed_macros, reason = "the measures print their timings")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use super::Editor;
use super::edit_tests::edit;
use super::migemo_dictionary_costs::WORDS;
use crate::buffer::Buffer;
use crate::complete::{Candidate, WORKER_PAST};
use crate::migemo::Dictionary;

/// Milliseconds since `started`.
fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

/// The median, the 99th percentile and the largest of `times`, sorted.
fn spread(times: &mut [f64]) -> (f64, f64, f64) {
    times.sort_by(f64::total_cmp);
    let at = |share: f64| times[((times.len() - 1) as f64 * share) as usize];
    (at(0.5), at(0.99), at(1.0))
}

/// The dictionary `OSO_MIGEMO_DICTIONARY` names.
fn dictionary() -> Arc<Dictionary> {
    let path = std::env::var_os("OSO_MIGEMO_DICTIONARY").expect("OSO_MIGEMO_DICTIONARY");
    Arc::new(Dictionary::new(
        std::fs::read_to_string(path).expect("a UTF-8 dictionary"),
    ))
}

/// Types `text` at the end of the prompt's answer and gives how long matching it took.
fn key(editor: &mut Editor, text: &str) -> f64 {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(end..end, text)]).unwrap();
    let started = Instant::now();
    editor.match_completion();
    ms(started)
}

/// Clears the prompt's answer, untimed.
fn clear(editor: &mut Editor) {
    let id = editor.prompt().unwrap().buffer();
    let end = editor.buffer(id).unwrap().text().len();
    editor.apply_edits(id, &[edit(0..end, "")]).unwrap();
    editor.match_completion();
    editor.take_search_request();
}

/// A project's search with the dictionary.
fn searching(dictionary: &Arc<Dictionary>) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor.set_migemo_dictionary(Some(Arc::clone(dictionary)));
    editor
        .open_project_search(PathBuf::from("/w/oso"), "oso", "project-search")
        .unwrap();
    editor
}

/// A completion of [`WORKER_PAST`] names, the most matched on the UI thread, with the
/// dictionary.
fn completing(dictionary: &Arc<Dictionary>) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("t", ""));
    editor.set_migemo_dictionary(Some(Arc::clone(dictionary)));
    let items: Arc<[Candidate]> = (0..WORKER_PAST)
        .map(|index| match index % 3 {
            0 => Candidate::new(format!("src/module{index}/漢字の変換{index}.md"), "", ""),
            1 => Candidate::new(format!("forward-char-{index}-command"), "", ""),
            _ => Candidate::new(format!("docs/adr/{index:04}-かんじ-search.md"), "", ""),
        })
        .collect();
    editor.open_completion("Find:", "find", items).unwrap();
    editor
}

/// Each word typed a letter at a time after `prefix`, three times over: the time of each key.
fn typed(editor: &mut Editor, prefix: &str) -> Vec<f64> {
    let mut times = Vec::new();
    for _ in 0..3 {
        for word in WORDS {
            clear(editor);
            key(editor, prefix);
            for letter in word.chars() {
                times.push(key(editor, &letter.to_string()));
                editor.take_search_request();
            }
        }
    }
    times
}

/// Pastes of `count` of the words, each a component, at once: the time of the key.
fn pasted(editor: &mut Editor, prefix: &str, count: usize) -> Vec<f64> {
    let mut times = Vec::new();
    for start in 0..WORDS.len() {
        clear(editor);
        let input: Vec<String> = (0..count)
            .map(|at| format!("{prefix}{}", WORDS[(start + at) % WORDS.len()]))
            .collect();
        times.push(key(editor, &input.join(" ")));
        // The next key keeps every component but the one it changes.
        times.push(key(editor, "a"));
        editor.take_search_request();
    }
    times
}

#[test]
#[ignore = "a measure for the ADR's notes, which needs a dictionary"]
fn migemo_prompt_costs_of_a_key() {
    let dictionary = dictionary();
    let report = |what: &str, mut times: Vec<f64>| {
        let (p50, p99, max) = spread(&mut times);
        println!(
            "{what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} keys",
            times.len()
        );
    };
    let mut editor = searching(&dictionary);
    report("project search, typing", typed(&mut editor, ""));
    for count in [3, 8, 32] {
        let mut times = pasted(&mut editor, "", count);
        let next: Vec<f64> = times.iter().skip(1).step_by(2).copied().collect();
        let paste: Vec<f64> = times.iter_mut().step_by(2).map(|time| *time).collect();
        report(&format!("project search, a paste of {count}"), paste);
        report(
            &format!("project search, the key after a paste of {count}"),
            next,
        );
    }
    let mut editor = completing(&dictionary);
    report("completion of 5000, typing `%`", typed(&mut editor, "%"));
    for count in [3, 8, 32] {
        let times = pasted(&mut editor, "%", count);
        let next: Vec<f64> = times.iter().skip(1).step_by(2).copied().collect();
        let paste: Vec<f64> = times.iter().step_by(2).copied().collect();
        report(&format!("completion, a paste of {count}"), paste);
        report(
            &format!("completion, the key after a paste of {count}"),
            next,
        );
    }
}
