//! Measures of migemo's dictionary lookups on crafted dictionaries (ADR 0116, Q7), ignored
//! by default: one lookup, and a paste of components that each look up, in a project's search
//! and in completion, with the steps an input's lookups share. The dictionaries are built at
//! run time, of no person's file. Run in a release build on an idle machine:
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
