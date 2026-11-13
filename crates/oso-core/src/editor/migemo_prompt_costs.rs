//! Measures of migemo in a project's search and in completion (ADR 0116, step 4), ignored by
//! default, for the ADR's notes: what a key costs on the UI thread as it parses the input, a
//! migemo component compiled with the dictionary a person names, a paste of several, the
//! heaviest strings pasted and an upper-case key after them, and whether a key narrows. The
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
use super::WalkEnd;
use super::edit_tests::edit;
use super::migemo_dictionary_costs::{WORDS, strings};
use crate::buffer::Buffer;
use crate::complete::{Candidate, WORKER_PAST};
use crate::migemo::Table;
use crate::migemo::{Dictionary, Romaji, word_cost};
use crate::project::LineQuery;

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

/// The `count` strings of one to three letters, or prefixes of [`WORDS`], whose patterns'
/// words cost the most.
fn heaviest(dictionary: &Dictionary, count: usize) -> Vec<String> {
    let mut strings: Vec<(usize, String)> = (strings().into_iter())
        .map(|string| {
            let romaji = Romaji::new(&string).expect("romaji");
            let words = dictionary.pattern_words(&romaji);
            (words.iter().map(|word| word_cost(word)).sum(), string)
        })
        .collect();
    strings.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.0));
    strings
        .into_iter()
        .take(count)
        .map(|(_, string)| string)
        .collect()
}

/// A paste of the 32 heaviest strings, `prefix`ed, then an upper-case key, which makes case
/// count and so compiles every component again: the time of each, over `rounds` rotations.
fn heavy_pastes(editor: &mut Editor, heavy: &[String], prefix: &str) -> (Vec<f64>, Vec<f64>) {
    let (mut pastes, mut uppers) = (Vec::new(), Vec::new());
    for start in 0..heavy.len() {
        clear(editor);
        let input: Vec<String> = (0..heavy.len())
            .map(|at| format!("{prefix}{}", heavy[(start + at) % heavy.len()]))
            .collect();
        pastes.push(key(editor, &input.join(" ")));
        editor.take_search_request();
        uppers.push(key(editor, " X"));
        editor.take_search_request();
    }
    (pastes, uppers)
}

/// Each word typed a letter at a time in a project's search whose every search ends, so each
/// key may narrow the one before: the time of each key, and how many narrowed of how many
/// started.
fn typed_ending(editor: &mut Editor) -> (Vec<f64>, usize, usize) {
    let (mut times, mut narrowed, mut started) = (Vec::new(), 0, 0);
    for _ in 0..3 {
        for word in WORDS {
            clear(editor);
            for letter in word.chars() {
                times.push(key(editor, &letter.to_string()));
                if let Some(request) = editor.take_search_request() {
                    started += 1;
                    narrowed += usize::from(request.narrow.is_some());
                    let end = Some(WalkEnd { cut: false });
                    editor.rows_found(request.generation, Vec::new(), end);
                }
            }
        }
    }
    (times, narrowed, started)
}

/// How long `new` narrowing `old` takes to tell, each parsed with `table` and `new` after
/// `old`, over `rounds`, and whether it narrows.
fn narrowing(old: &str, new: &str, table: &Table, rounds: usize) -> (Vec<f64>, bool) {
    let before = LineQuery::parse_after(old, &LineQuery::parse(""), true, Some(table));
    let query = LineQuery::parse_after(new, &before, true, Some(table));
    let mut times = Vec::new();
    let mut narrows = false;
    for _ in 0..rounds {
        // A clone has a cold cache, as a query the editor compiled for this key.
        let before = before.clone();
        let started = Instant::now();
        narrows = std::hint::black_box(query.narrows(&before));
        times.push(ms(started));
    }
    (times, narrows)
}

#[test]
#[ignore = "a measure for the ADR's notes, which needs a dictionary"]
fn migemo_prompt_costs_of_heavy_input_and_narrowing() {
    let dictionary = dictionary();
    let report = |what: &str, mut times: Vec<f64>| {
        let (p50, p99, max) = spread(&mut times);
        println!(
            "{what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} keys",
            times.len()
        );
    };
    let heavy = heaviest(&dictionary, 32);
    let mut editor = searching(&dictionary);
    let (pastes, uppers) = heavy_pastes(&mut editor, &heavy, "");
    report("project search, a paste of the 32 heaviest", pastes);
    report("project search, an upper-case key after them", uppers);
    let (times, narrowed, started) = typed_ending(&mut editor);
    report("project search, typing, each search ended", times);
    println!("  {narrowed} of {started} searches narrowed the one before");
    let mut editor = completing(&dictionary);
    let (pastes, uppers) = heavy_pastes(&mut editor, &heavy, "%");
    report("completion of 5000, a paste of the 32 heaviest", pastes);
    report("completion of 5000, an upper-case key after them", uppers);

    let table = Table {
        generation: 1,
        dictionary: Arc::clone(&dictionary),
    };
    // Realistic: each prefix of the words, and the next letter.
    let (mut times, mut narrowed, mut pairs) = (Vec::new(), 0, 0);
    for word in WORDS {
        for end in 1..word.len() {
            let (taken, narrows) = narrowing(&word[..end], &word[..=end], &table, 3);
            times.extend(taken);
            narrowed += usize::from(narrows);
            pairs += 1;
        }
    }
    report(
        "narrowing, a letter typed onto each prefix of the words",
        times,
    );
    println!("  {narrowed} of {pairs} narrowed");
    // Crafted: either of two kana up to the expansion's cap, and the heaviest strings and a
    // letter after each.
    let mut times = Vec::new();
    for count in 1..=10 {
        let old = "zu".repeat(count);
        let (taken, narrows) = narrowing(&old, &format!("{old}z"), &table, 5);
        let (p50, _, max) = spread(&mut taken.clone());
        println!(
            "  narrowing, {count} zu then z: {p50:.3} ms p50, {max:.3} ms most, narrows {narrows}"
        );
        times.extend(taken);
    }
    let (mut heavy_times, mut narrowed) = (Vec::new(), 0);
    for string in &heavy {
        for letter in ['a', 'i', 'u', 'k', 'n'] {
            let new = format!("{string}{letter}");
            let (taken, narrows) = narrowing(string, &new, &table, 3);
            heavy_times.extend(taken);
            narrowed += usize::from(narrows);
        }
    }
    report(
        "narrowing, a letter after each of the 32 heaviest",
        heavy_times,
    );
    println!("  {narrowed} of {} narrowed", heavy.len() * 5);
    // Thirty-two components each of the most strings under the cap, all changed at once.
    let old = vec!["zu".repeat(7); 32].join(" ");
    let new = vec!["zu".repeat(8); 32].join(" ");
    let (taken, narrows) = narrowing(&old, &new, &table, 5);
    report(
        &format!("narrowing, 32 × zu×7 to zu×8 (narrows {narrows})"),
        taken,
    );
    let pasted = heavy.join(" ");
    let (taken, narrows) = narrowing(&pasted, &format!("{pasted}a"), &table, 5);
    report(
        &format!("narrowing, the 32 heaviest and a letter (narrows {narrows})"),
        taken,
    );
}
