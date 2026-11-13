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
use crate::migemo::Held;
use crate::migemo::{Budget, Dictionary, MAX_LOOKUP_STEPS, Romaji, word_cost};
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

/// The lookup steps (see `MAX_LOOKUP_STEPS`) `string`'s pattern takes from `dictionary`.
fn steps(dictionary: &Dictionary, string: &str) -> usize {
    let romaji = Romaji::new(string).expect("romaji");
    let (_, spent) = dictionary.pattern_words_with_budget(&romaji, Budget::PATTERN);
    spent.used.lookups
}

#[test]
#[ignore = "a measure for the ADR's notes, which needs a dictionary"]
fn migemo_prompt_costs_of_lookup_steps() {
    let dictionary = dictionary();
    let most = (strings().iter())
        .map(|string| steps(&dictionary, string))
        .max();
    println!("lookup steps, the most of one string: {most:?} of {MAX_LOOKUP_STEPS}");
    let pastes = (0..WORDS.len()).map(|start| {
        (0..32)
            .map(|at| steps(&dictionary, WORDS[(start + at) % WORDS.len()]))
            .sum::<usize>()
    });
    println!(
        "lookup steps, the most of a paste of 32 words: {:?}",
        pastes.max()
    );
    let heavy: usize = (heaviest(&dictionary, 32).iter())
        .map(|string| steps(&dictionary, string))
        .sum();
    println!("lookup steps, the 32 heaviest: {heavy}");
    let long = WORDS.iter().map(|word| word.repeat(255 / word.len() + 1));
    let most = long
        .filter_map(|string| {
            let end = (0..=255)
                .rev()
                .find(|&end| Romaji::new(&string[..end]).is_some())?;
            Some(steps(&dictionary, &string[..end]))
        })
        .max();
    println!("lookup steps, the most of one word repeated to 255 letters: {most:?}");
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

/// A paste of the 32 heaviest strings, `prefix`ed, then an upper-case key at its end, which
/// makes case count and so compiles every component again: the time of each, over each
/// rotation of the strings.
fn heavy_pastes(editor: &mut Editor, heavy: &[String], prefix: &str) -> (Vec<f64>, Vec<f64>) {
    let (mut pastes, mut uppers) = (Vec::new(), Vec::new());
    for start in 0..heavy.len() {
        clear(editor);
        let input: Vec<String> = (0..heavy.len())
            .map(|at| format!("{prefix}{}", heavy[(start + at) % heavy.len()]))
            .collect();
        pastes.push(key(editor, &input.join(" ")));
        editor.take_search_request();
        // Onto the last component, as a 33rd would be past the most an input matches.
        uppers.push(key(editor, "X"));
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
fn narrowing(old: &str, new: &str, table: &Held, rounds: usize) -> (Vec<f64>, bool) {
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

    let table = Held {
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

/// A dictionary built to make a lookup slow, and the components a paste of it takes: every
/// reading is `bits` of either of two kana (`ず` or `づ`), `kept` × `あ` and two kana that
/// tell its `each` entries apart, so a lookup keeps the most ranges, each of `each`
/// entries, through every kana; each component then ends with a kana no reading has there,
/// so it finds no word and spends nothing. Built at run time, of no person's file.
fn slow_lookups(bits: usize, each: usize, kept: usize) -> (Dictionary, Vec<String>) {
    let apart: Vec<char> = "かきくけこさしすせそたちつてと".chars().collect();
    let mut text = String::new();
    for group in 0..1usize << bits {
        for entry in 0..each {
            (0..bits).for_each(|bit| text.push(if group >> bit & 1 == 0 { 'ず' } else { 'づ' }));
            (0..kept).for_each(|_| text.push('あ'));
            text.push(apart[entry / apart.len() % apart.len()]);
            text.push(apart[entry % apart.len()]);
            text.push_str("\t字\n");
        }
    }
    let ends = [
        "na", "ni", "nu", "ne", "no", "ha", "hi", "fu", "he", "ho", "ma", "mi", "mu", "me", "mo",
        "ya", "yu", "yo", "ra", "ri", "ru", "re", "ro", "wa",
    ];
    let stem = format!("{}{}", "zu".repeat(bits), "a".repeat(kept));
    let mut components = Vec::new();
    let mut bytes = 0;
    for end in ends {
        let component = format!("{stem}{end}");
        if bytes + component.len() + 1 > crate::complete::MAX_INPUT_BYTES || components.len() == 32
        {
            break;
        }
        bytes += component.len() + 1;
        components.push(component);
    }
    (Dictionary::new(text), components)
}

#[test]
#[ignore = "a measure, in a release build"]
fn migemo_prompt_costs_of_slow_lookups() {
    let report = |what: &str, mut times: Vec<f64>| {
        let (p50, p99, max) = spread(&mut times);
        println!(
            "{what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} times",
            times.len()
        );
    };
    for (bits, each, kept) in [(11, 1, 53), (11, 16, 40), (11, 128, 25), (10, 256, 25)] {
        let (dictionary, components) = slow_lookups(bits, each, kept);
        let shape = format!(
            "{bits} bits × {each} × {kept}: {} readings, {} skipped, {} components",
            dictionary.readings(),
            dictionary.skipped(),
            components.len()
        );
        let romaji = Romaji::new(&components[0]).expect("romaji");
        let mut times = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            assert!(std::hint::black_box(dictionary.pattern_words(&romaji)).is_empty());
            times.push(ms(started));
        }
        report(&format!("one lookup, {shape}"), times);
        let dictionary = Arc::new(dictionary);
        let input = components.join(" ");
        for (what, mut editor, prefix) in [
            ("project search", searching(&dictionary), ""),
            ("completion of 5000", completing(&dictionary), "%"),
        ] {
            let input = if prefix.is_empty() {
                input.clone()
            } else {
                (components.iter())
                    .map(|component| format!("{prefix}{component}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let mut times = Vec::new();
            for _ in 0..10 {
                clear(&mut editor);
                times.push(key(&mut editor, &input));
                editor.take_search_request();
            }
            report(&format!("{what}, a paste, {shape}"), times);
        }
    }
}

/// Pastes `input` in a project's search, ends the search it starts, as a walk that ran to
/// the end, and types an upper-case letter as a component of its own: the time of that key,
/// over `rounds`, and how many of those searches narrowed the one before.
fn upper_after_ended(editor: &mut Editor, input: &str, rounds: usize) -> (Vec<f64>, usize) {
    let (mut times, mut narrowed) = (Vec::new(), 0);
    for _ in 0..rounds {
        clear(editor);
        key(editor, input);
        let request = editor.take_search_request().expect("a search");
        editor.rows_found(request.generation, Vec::new(), Some(WalkEnd { cut: false }));
        times.push(key(editor, " X"));
        let request = editor.take_search_request().expect("a search");
        narrowed += usize::from(request.narrow.is_some());
    }
    (times, narrowed)
}

#[test]
#[ignore = "a measure for the ADR's notes, which needs a dictionary"]
fn migemo_prompt_costs_of_case_flips_and_long_components() {
    let dictionary = dictionary();
    let report = |what: &str, mut times: Vec<f64>| {
        let (p50, p99, max) = spread(&mut times);
        println!(
            "{what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} times",
            times.len()
        );
    };
    let mut editor = searching(&dictionary);
    // Realistic: 31 common words, and 31 of the heaviest, then an upper-case key.
    let words = WORDS.iter().cycle().take(31).copied().collect::<Vec<_>>();
    let (times, narrowed) = upper_after_ended(&mut editor, &words.join(" "), 20);
    report(
        &format!("project search, 31 words, ended, then X (narrowed {narrowed})"),
        times,
    );
    let heavy = heaviest(&dictionary, 31);
    let (times, narrowed) = upper_after_ended(&mut editor, &heavy.join(" "), 20);
    report(
        &format!("project search, the 31 heaviest, ended, then X (narrowed {narrowed})"),
        times,
    );
    // Crafted: 31 distinct components of either of two kana 8 times and a tail each.
    let tails: Vec<String> = ["a", "i", "u", "e", "o"]
        .iter()
        .flat_map(|vowel| ["", "k", "s", "t", "n", "m"].map(|c| format!("{c}{vowel}")))
        .chain(["ya".to_owned()])
        .collect();
    let crafted: Vec<String> = tails
        .iter()
        .map(|tail| format!("{}{tail}", "zu".repeat(8)))
        .collect();
    let (times, narrowed) = upper_after_ended(&mut editor, &crafted.join(" "), 20);
    report(
        &format!("project search, 31 × zu×8 and a tail, ended, then X (narrowed {narrowed})"),
        times,
    );
    // Three components of 255 letters, a word repeated, in either prompt.
    let long: Vec<String> = ["kanji", "henkan", "nihongo"]
        .iter()
        .map(|word| {
            let string = word.repeat(255 / word.len() + 1);
            let end = (0..=255)
                .rev()
                .find(|&end| Romaji::new(&string[..end]).is_some());
            string[..end.expect("romaji")].to_owned()
        })
        .collect();
    for (what, mut editor, prefix) in [
        ("project search", searching(&dictionary), ""),
        ("completion of 5000", completing(&dictionary), "%"),
    ] {
        let input: Vec<String> = long
            .iter()
            .map(|string| format!("{prefix}{string}"))
            .collect();
        let (mut pastes, mut nexts) = (Vec::new(), Vec::new());
        for _ in 0..20 {
            clear(&mut editor);
            pastes.push(key(&mut editor, &input.join(" ")));
            editor.take_search_request();
            nexts.push(key(&mut editor, "a"));
            editor.take_search_request();
        }
        report(&format!("{what}, a paste of three of 255 letters"), pastes);
        report(&format!("{what}, the key after it"), nexts);
    }
    let strings = [
        ("the words, typed", strings()),
        ("the 31 heaviest", heavy),
        ("31 × zu×8 and a tail", crafted),
        ("three of 255 letters", long),
    ];
    alternatives_costs(&dictionary, &strings);
}

/// What building the strings a pattern matches takes, which a project's search does for each
/// migemo component it compiles, for each set of `strings`.
fn alternatives_costs(dictionary: &Dictionary, strings: &[(&str, Vec<String>)]) {
    for (what, strings) in strings {
        let mut times = Vec::new();
        for string in strings {
            let romaji = Romaji::new(string).expect("romaji");
            let words = dictionary.pattern_words(&romaji);
            for _ in 0..3 {
                // The caps a project's search keeps them within.
                let caps = (1024, 64 * 1024);
                let started = Instant::now();
                std::hint::black_box(crate::migemo::alternatives(string, &words, caps));
                times.push(ms(started));
            }
        }
        let (p50, p99, max) = spread(&mut times);
        println!(
            "the strings of a pattern, {what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} times",
            times.len()
        );
    }
}
