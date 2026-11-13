//! Measures of migemo's dictionary lookups on crafted dictionaries (ADR 0116, Q7), ignored
//! by default: one lookup, and a paste of components that each look up, in a project's search
//! and in completion, with the steps an input's lookups share; and a look at whether a
//! project's search narrows at its caps (Q8). The dictionaries are built at run time, of no
//! person's file. Run in a release build on an idle machine:
//! `cargo test -p oso-core --release --lib migemo_lookup_costs -- --ignored --nocapture --test-threads 1`.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

#![expect(clippy::disallowed_macros, reason = "the measures print their timings")]

use std::sync::Arc;
use std::time::Instant;

use super::migemo_prompt_costs::{clear, completing, key, ms, searching, spread};
use crate::migemo::{Dictionary, Romaji};

/// A dictionary built to make a lookup slow, and the components a paste of it takes: every
/// reading is `bits` of either of two kana (`ず` or `づ`), `kept` × `あ` and two kana that
/// tell its `each` entries apart, so a lookup keeps the most ranges, each of `each`
/// entries, through every kana; each component then ends with a kana no reading has there,
/// so it finds no word and spends nothing. Built at run time, of no person's file.
fn slow_lookups(
    (bits, each, kept): (usize, usize, usize),
    scatter: bool,
) -> (Dictionary, Vec<String>) {
    let apart: Vec<char> = "かきくけこさしすせそたちつてと".chars().collect();
    let mut lines = Vec::new();
    for group in 0..1usize << bits {
        for entry in 0..each {
            let mut line = String::new();
            (0..bits).for_each(|bit| line.push(if group >> bit & 1 == 0 { 'ず' } else { 'づ' }));
            (0..kept).for_each(|_| line.push('あ'));
            line.push(apart[entry / apart.len() % apart.len()]);
            line.push(apart[entry % apart.len()]);
            lines.push(line);
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
    (written(lines, scatter), components)
}

/// A dictionary of `readings`, each with one word: with `scatter`, a word of 60 bytes and the
/// lines in a fixed shuffled order, so a lookup's reads land all over a text near
/// `MAX_BYTES`.
fn written(mut readings: Vec<String>, scatter: bool) -> Dictionary {
    let mut text = String::new();
    if scatter {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        for at in (1..readings.len()).rev() {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            readings.swap(at, (seed >> 33) as usize % (at + 1));
        }
    }
    let word = if scatter {
        "字".repeat(20)
    } else {
        "字".to_owned()
    };
    for reading in readings {
        text.push_str(&format!("{reading}\t{word}\n"));
    }
    Dictionary::new(text)
}

/// A dictionary where each of 32 two-kana prefixes starts 2048 readings, and the 32 romaji
/// components that look them up: the first two spend the words, and every other still
/// chooses its entries.
fn many_entries(scatter: bool) -> (Dictionary, Vec<String>) {
    let kana: Vec<char> = ('ぁ'..='ゖ').collect();
    let firsts = [
        ("ka", 'か'),
        ("ki", 'き'),
        ("ku", 'く'),
        ("ke", 'け'),
        ("ko", 'こ'),
        ("sa", 'さ'),
        ("shi", 'し'),
        ("su", 'す'),
    ];
    let seconds = [("ta", 'た'), ("chi", 'ち'), ("tsu", 'つ'), ("te", 'て')];
    let mut lines = Vec::new();
    let mut components = Vec::new();
    for (first, one) in firsts {
        for (second, two) in seconds {
            let component = format!("{first}{second}");
            let stem = format!("{one}{two}");
            for n in 0..2048 {
                lines.push(format!("{stem}{}{}", kana[n / 86], kana[n % 86]));
            }
            components.push(component);
        }
    }
    (written(lines, scatter), components)
}

#[test]
#[ignore = "a measure, in a release build"]
fn migemo_lookup_costs_of_crafted_dictionaries() {
    let report = |what: &str, mut times: Vec<f64>| {
        let (p50, p99, max) = spread(&mut times);
        println!(
            "{what}: {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most, {} times",
            times.len()
        );
    };
    let shapes = [(11, 16, 40), (11, 128, 25), (10, 256, 25), (11, 128, 25)];
    let dictionaries = (shapes.iter().enumerate())
        .map(|(at, &shape)| {
            let scatter = at == 3;
            let (bits, each, kept) = shape;
            let name = format!("{bits} bits × {each} × {kept}, scattered {scatter}");
            (name, slow_lookups(shape, scatter))
        })
        .chain([false, true].map(|scatter| {
            let name = format!("32 prefixes × 2048, scattered {scatter}");
            (name, many_entries(scatter))
        }));
    for (name, (dictionary, components)) in dictionaries {
        let shape = format!(
            "{name}: {} readings, {} skipped, {} components",
            dictionary.readings(),
            dictionary.skipped(),
            components.len()
        );
        let romaji = Romaji::new(&components[0]).expect("romaji");
        let mut times = Vec::new();
        for _ in 0..20 {
            let started = Instant::now();
            std::hint::black_box(dictionary.pattern_words(&romaji));
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

/// A dictionary built to make a look at whether a project's search narrows slow (ADR 0116,
/// Q8), and the components it takes: each of `count` components is `ず` `pairs` times,
/// `tail` × `あ` and a kana of its own, so that their patterns match as many strings as
/// narrowing reads, of about as many bytes, and finds 128 words of 60 bytes, the input's word
/// budget over 4: kanji spread over their block, or with `kana`, `ず` or `づ` along the
/// strings the pattern matches. Built at run time, of no person's file.
fn slow_narrowing(
    (count, pairs, tail): (usize, usize, usize),
    kana: bool,
) -> (Dictionary, Vec<String>) {
    let ends = [
        ("ka", 'か'),
        ("ki", 'き'),
        ("ku", 'く'),
        ("ke", 'け'),
        ("ko", 'こ'),
    ];
    // Kanji spread over the block, so a pattern's words share few prefixes.
    let kanji = |n: usize| {
        let code = u32::try_from(0x4e00 + (n * 7919) % 20_000).expect("in range");
        char::from_u32(code).expect("a kanji")
    };
    let mut text = String::new();
    let mut components = Vec::new();
    for (at, &(end, last)) in ends.iter().take(count).enumerate() {
        let reading = format!("{}{}{last}", "ず".repeat(pairs), "あ".repeat(tail));
        for n in 0..128 {
            let word: String = if kana {
                let sounds = (0..20).map(|bit| {
                    if n >> (bit % 8) & 1 == 0 {
                        'ず'
                    } else {
                        'づ'
                    }
                });
                sounds.collect()
            } else {
                (0..20).map(|c| kanji(at * 4096 + n * 20 + c)).collect()
            };
            text.push_str(&format!("{reading}{}\t{word}\n", ['ん', 'の'][n % 2]));
        }
        components.push(format!("{}{}{end}", "zu".repeat(pairs), "a".repeat(tail)));
    }
    (Dictionary::new(text), components)
}

#[test]
#[ignore = "a measure, in a release build"]
fn migemo_narrowing_costs_of_crafted_dictionaries() {
    use crate::migemo::Table;
    use crate::project::LineQuery;
    // Past the strings at the first component, then at the totals with each component within
    // them, of letters and kana alone or with words along them.
    let shapes = [(4, 8, 12), (5, 6, 13), (5, 6, 13), (5, 6, 0)];
    for (at, shape) in shapes.into_iter().enumerate() {
        let kana = at == 2;
        let (dictionary, components) = slow_narrowing(shape, kana);
        let (count, pairs, tail) = shape;
        let input = components.join(" ");
        let (strings, bytes) = (components.iter()).fold((0, 0), |(strings, bytes), component| {
            let all = crate::migemo::alternatives(component, &[], (usize::MAX, usize::MAX));
            let all = all.expect("romaji");
            (
                strings + all.len(),
                bytes + all.iter().map(|s| s.len()).sum::<usize>(),
            )
        });
        let shape = format!(
            "{count} components of ず×{pairs} あ×{tail}, kana words {kana}: {strings} strings of {bytes} bytes besides words"
        );
        let dictionary = Arc::new(dictionary);
        // The same dictionary read again, as when the file is: every component looks again.
        let tables = [1, 2].map(|generation| Table {
            generation,
            dictionary: Arc::clone(&dictionary),
        });
        let mut times = Vec::new();
        let mut narrowed = false;
        for _ in 0..20 {
            // Compiled anew each time, so their matchers' caches start cold.
            let before =
                LineQuery::parse_after(&input, &LineQuery::empty(), true, Some(&tables[0]));
            let after = LineQuery::parse_after(&input, &before, true, Some(&tables[1]));
            let started = Instant::now();
            narrowed = std::hint::black_box(after.narrows(&before));
            times.push(ms(started));
        }
        let (p50, p99, max) = spread(&mut times);
        println!(
            "narrows, {shape}: {narrowed}, {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most"
        );
        // The whole event on the UI thread: the dictionary read again under an ended search.
        let mut editor = searching(&dictionary);
        let mut times = Vec::new();
        for _ in 0..10 {
            clear(&mut editor);
            key(&mut editor, &input);
            let request = editor.take_search_request().expect("a search");
            editor.rows_found(
                request.generation,
                Vec::new(),
                Some(super::WalkEnd { cut: false }),
            );
            let started = Instant::now();
            editor.set_migemo_dictionary(Some(Arc::clone(&dictionary)));
            times.push(ms(started));
            let again = editor.take_search_request().expect("searched again");
            narrowed = again.narrow.is_some();
        }
        let (p50, p99, max) = spread(&mut times);
        println!(
            "dictionary again, {shape}: narrowed {narrowed}, {p50:.3} ms p50, {p99:.3} ms p99, {max:.3} ms most"
        );
    }
}
