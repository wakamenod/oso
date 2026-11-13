//! Tests of migemo's dictionary (ADR 0116): both formats, what each line leaves out, the
//! limits, readings made one, and the words a string's kana give, over synthetic text.
//!
//! Must not depend on: any module of this crate but its parent's.

use proptest::prelude::*;

use super::{
    Dictionary, Entry, MAX_BYTES, MAX_ENTRIES, MAX_PREFIX_ENTRIES, MAX_READING, MAX_WORD,
    MAX_WORDS, MAX_WORDS_A_READING, Span,
};
use crate::migemo::budget::{
    MAX_LOOKUP_STEPS, MAX_PATTERN_WORD_BYTES, MAX_PATTERN_WORDS, word_cost,
};
use crate::migemo::{Budget, Romaji};

/// The words `string`'s kana give from `dictionary`, sorted.
fn words(dictionary: &Dictionary, string: &str) -> Vec<String> {
    let romaji = Romaji::new(string).unwrap();
    let mut words: Vec<String> = (dictionary.pattern_words(&romaji).into_iter())
        .map(str::to_owned)
        .collect();
    words.sort();
    words
}

/// Each reading with its words, in the table's order.
fn table(dictionary: &Dictionary) -> Vec<(String, Vec<String>)> {
    (dictionary.entries.iter())
        .map(|entry| {
            let words = &dictionary.words[entry.words.range()];
            let words = words
                .iter()
                .map(|span| dictionary.text[span.range()].to_owned());
            (dictionary.reading(*entry).to_owned(), words.collect())
        })
        .collect()
}

fn entry(reading: &str, words: &[&str]) -> (String, Vec<String>) {
    let words = words.iter().map(|word| (*word).to_owned()).collect();
    (reading.to_owned(), words)
}

#[test]
fn reads_migemo_dict_lines() {
    let dictionary = Dictionary::new("かんじ\t漢字\t感じ\nあい\t愛\n".to_owned());
    assert_eq!(
        table(&dictionary),
        [entry("あい", &["愛"]), entry("かんじ", &["漢字", "感じ"])]
    );
    assert_eq!((dictionary.readings(), dictionary.words()), (2, 3));
    assert_eq!(dictionary.skipped(), 0);
}

#[test]
fn reads_skk_lines_with_okurigana_annotations_and_lisp() {
    let text = ";; okuri-ari entries.\n\
                かk /書/描;picture/\n\
                かんじ /漢字/幹事;secretary/(concat \"a\\057b\")/(株)/\n";
    let dictionary = Dictionary::new(text.to_owned());
    assert_eq!(
        table(&dictionary),
        [
            entry("か", &["書", "描"]),
            entry("かんじ", &["漢字", "幹事", "(株)"])
        ]
    );
    // The Lisp word.
    assert_eq!(dictionary.skipped(), 1);
}

#[test]
fn tells_the_formats_apart_line_by_line() {
    let text = "\u{feff}かな\t仮名\r\nかに /蟹/\r\n";
    let dictionary = Dictionary::new(text.to_owned());
    assert_eq!(
        table(&dictionary),
        [entry("かな", &["仮名"]), entry("かに", &["蟹"])]
    );
}

#[test]
fn leaves_out_lines_in_neither_shape_and_readings_not_hiragana() {
    let long = "あ".repeat(MAX_READING + 1);
    let text = format!(
        "no shape here\nabc\tエービーシー\nカナ\t仮名\nか゛\tが\n{long}\t長\n\
         いぬ /\nねこ\t\n;comment\n\nぱーてぃー\tパーティー\nゔぁ\tヴァ\n"
    );
    let dictionary = Dictionary::new(text);
    assert_eq!(
        table(&dictionary),
        [
            entry("ぱーてぃー", &["パーティー"]),
            entry("ゔぁ", &["ヴァ"])
        ]
    );
    // Seven lines: no shape, `abc`, `カナ`, `か゛`, too long, and two with no word.
    assert_eq!(dictionary.skipped(), 7);
}

#[test]
fn leaves_out_words_too_long_or_with_control_characters() {
    let long = "x".repeat(MAX_WORD + 1);
    let most = "y".repeat(MAX_WORD);
    let text = format!("あ\t{long}\t{most}\tbell\u{7}\t\u{202e}は\n");
    let dictionary = Dictionary::new(text);
    // U+202E is a format character, not a control one: a word is only text in a pattern.
    assert_eq!(table(&dictionary), [entry("あ", &[&most, "\u{202e}は"])]);
    assert_eq!(dictionary.skipped(), 2);
}

#[test]
fn a_reading_given_twice_is_one_entry_with_both_lines_words() {
    // Okurigana counts only in SKK's lines.
    let text = "かk /書/\nか\t蚊\nか /課/\nかk\t掻\n";
    let dictionary = Dictionary::new(text.to_owned());
    assert_eq!(table(&dictionary), [entry("か", &["書", "蚊", "課"])]);
}

#[test]
fn a_reading_keeps_at_most_its_limit_of_words_over_its_lines() {
    let words: Vec<String> = (0..MAX_WORDS_A_READING + 10)
        .map(|n| format!("w{n}"))
        .collect();
    let (first, second) = words.split_at(200);
    let text = format!("あ\t{}\nあ /{}/\n", first.join("\t"), second.join("/"));
    let dictionary = Dictionary::new(text);
    let table = table(&dictionary);
    assert_eq!(table[0].1, words[..MAX_WORDS_A_READING]);
    assert_eq!(dictionary.skipped(), 10);
}

#[test]
fn reads_at_most_its_limit_of_bytes_to_a_whole_line() {
    let line = "あい\t愛\n";
    let mut text = line.repeat(MAX_BYTES / line.len());
    text.push_str("ん\tここまで\n");
    assert!(text.len() > MAX_BYTES);
    let dictionary = Dictionary::new(text);
    assert_eq!(
        table(&dictionary).len(),
        1,
        "the last line is past the limit"
    );
}

#[test]
fn a_strings_kana_give_the_words_of_the_readings_they_start() {
    let text = "かん\t缶\nかんじ\t漢字\nかんじゃ\t患者\nかな\t仮名\nき\t木\n";
    let dictionary = Dictionary::new(text.to_owned());
    // `かんじゃ` starts with `かんじ`.
    assert_eq!(words(&dictionary, "kanji"), ["患者", "漢字"]);
    assert_eq!(words(&dictionary, "kanjo"), Vec::<String>::new());
    assert_eq!(words(&dictionary, "kanj"), ["患者", "漢字"]);
    assert_eq!(words(&dictionary, "kan"), ["仮名", "患者", "漢字", "缶"]);
    assert_eq!(
        words(&dictionary, "k"),
        ["仮名", "患者", "木", "漢字", "缶"]
    );
    assert!(words(&dictionary, "sa").is_empty());
}

#[test]
fn either_of_two_kana_finds_both_readings() {
    let text = "つづく\t続く\nつずく\t続く2\nはなぢ\t鼻血\n";
    let dictionary = Dictionary::new(text.to_owned());
    assert_eq!(words(&dictionary, "tsuzuku"), ["続く", "続く2"]);
    assert_eq!(words(&dictionary, "hanaji"), ["鼻血"]);
    assert_eq!(words(&dictionary, "hanaj"), ["鼻血"]);
}

/// A dictionary of `count` readings of `prefix` and three kana, each with `words` words.
fn many(prefix: &str, count: usize, words: usize) -> String {
    let kana: Vec<char> = ('ぁ'..='ゖ').collect();
    let mut text = String::new();
    for n in 0..count {
        let (a, b, c) = (n % 86, n / 86 % 86, n / 86 / 86);
        let reading: String = [kana[c], kana[b], kana[a]].iter().collect();
        let words: Vec<String> = (0..words).map(|word| format!("_{n}.{word}")).collect();
        text.push_str(&format!("{prefix}{reading}\t{}\n", words.join("\t")));
    }
    text
}

#[test]
fn past_its_limit_of_entries_only_the_exact_readings_give_words() {
    let mut text = many("かな", MAX_PREFIX_ENTRIES - 2, 1);
    text.push_str("かな\t仮名\nかん\t缶\n");
    let dictionary = Dictionary::new(text.clone());
    assert_eq!(words(&dictionary, "kan").len(), MAX_PREFIX_ENTRIES);
    text.push_str("か\t蚊\nかんじ\t漢字\n");
    let dictionary = Dictionary::new(text);
    assert_eq!(words(&dictionary, "ka"), ["蚊"]);
    // The kana and each unfinished end, exactly.
    assert_eq!(words(&dictionary, "kan"), ["仮名", "缶"]);
    // An unfinished end alone gives none.
    assert!(words(&dictionary, "k").is_empty());
    // Few enough again.
    assert_eq!(words(&dictionary, "kanji"), ["漢字"]);
}

#[test]
fn a_pattern_takes_at_most_its_limit_of_words_the_exact_and_shorter_first() {
    let mut text = many("あい", MAX_PREFIX_ENTRIES - 2, 2);
    let shorter: Vec<String> = (0..10).map(|n| format!("s{n}")).collect();
    text.push_str(&format!("あいう\t{}\nあい\t愛\n", shorter.join("\t")));
    let dictionary = Dictionary::new(text);
    let romaji = Romaji::new("ai").unwrap();
    let words = dictionary.pattern_words(&romaji);
    assert_eq!(words.len(), MAX_PATTERN_WORDS);
    assert_eq!(words[0], "愛");
    assert_eq!(words[1..11], shorter);
    assert!(words[11..].iter().all(|word| word.starts_with('_')));
}

#[test]
fn a_pattern_takes_at_most_its_limit_of_words_bytes() {
    // 1024 words of 60 bytes, each of a reading of its own, and the shorter words before.
    let mut text = many("あい", 600, 1);
    for n in 0..1024 {
        text.push_str(&format!(
            "あいう{}\t{}{n:04}\n",
            hiragana(n),
            "x".repeat(56)
        ));
    }
    let dictionary = Dictionary::new(text);
    let romaji = Romaji::new("ai").unwrap();
    let words = dictionary.pattern_words(&romaji);
    let cost: usize = words.iter().map(|word| word_cost(word)).sum();
    assert!(cost <= MAX_PATTERN_WORD_BYTES, "{cost}");
    assert!(cost > MAX_PATTERN_WORD_BYTES - 4 * MAX_WORD, "{cost}");
}

#[test]
fn a_letter_with_a_case_counts_four_times_its_bytes() {
    assert_eq!(word_cost("漢字"), 6);
    assert_eq!(word_cost("(株)1"), 6);
    assert_eq!(word_cost("ab"), 8);
    // Greek, Cyrillic, the Kelvin sign, long s, a titlecase letter and sharp s.
    for letter in ["σ", "Ж", "\u{212a}", "ſ", "ǅ", "ß"] {
        assert_eq!(word_cost(letter), 4 * letter.len(), "{letter}");
    }
    // Letters with a case take a quarter of the bytes of those without.
    let dictionary = |word: &str| {
        let mut text = String::new();
        for n in 0..2048 {
            text.push_str(&format!("か{}\t{word}{n:04}\n", hiragana(n)));
        }
        Dictionary::new(text)
    };
    let romaji = Romaji::new("ka").unwrap();
    let kanji = dictionary(&"漢".repeat(20)).pattern_words(&romaji).len();
    let latin = dictionary(&"x".repeat(60)).pattern_words(&romaji).len();
    assert!(latin * 3 < kanji, "{latin} {kanji}");
}

/// Three hiragana for `n`, a reading of its own.
fn hiragana(n: usize) -> String {
    let kana: Vec<char> = ('ぁ'..='ゖ').collect();
    [kana[n / 86 / 86 % 86], kana[n / 86 % 86], kana[n % 86]]
        .iter()
        .collect()
}

#[test]
fn an_unfinished_end_that_starts_another_counts_its_readings_once() {
    let dictionary = Dictionary::new("きゃく\t客\nき\t木\n".to_owned());
    assert_eq!(words(&dictionary, "k"), ["客", "木"]);
}

#[test]
fn many_splits_of_either_kana_give_none_past_the_limit() {
    // Readings of `ず` or `づ` 12 times, each a range of its own for `zu` 12 times.
    let readings = |count: usize| {
        let mut text = String::new();
        for n in 0..count {
            let reading: String = (0..12)
                .map(|bit| if n >> bit & 1 == 1 { 'ず' } else { 'づ' })
                .collect();
            text.push_str(&format!("{reading}\tw{n}\n"));
        }
        Dictionary::new(text)
    };
    let string = "zu".repeat(12);
    assert_eq!(
        words(&readings(MAX_PREFIX_ENTRIES), &string).len(),
        MAX_PREFIX_ENTRIES
    );
    assert!(words(&readings(MAX_PREFIX_ENTRIES + 1), &string).is_empty());
}

#[test]
fn a_lookup_past_its_steps_stops_short_and_gives_no_words() {
    // Readings of `ず` or `づ` 11 times, then `あ` `long` times and `か`: a lookup keeps
    // every range through each `あ`.
    let readings = |long: usize| {
        let mut text = String::new();
        for n in 0..MAX_PREFIX_ENTRIES {
            let reading: String = (0..11)
                .map(|bit| if n >> bit & 1 == 1 { 'ず' } else { 'づ' })
                .chain(std::iter::repeat_n('あ', long))
                .chain(['か'])
                .collect();
            text.push_str(&format!("{reading}\tw{n}\n"));
        }
        Dictionary::new(text)
    };
    let given = |long: usize| {
        let string = format!("{}{}ka", "zu".repeat(11), "a".repeat(long));
        let romaji = Romaji::new(&string).expect("romaji");
        let dictionary = readings(long);
        let (words, spent) = dictionary.pattern_words_with_budget(&romaji, Budget::PATTERN);
        (words.len(), spent)
    };
    let (found, spent) = given(4);
    assert_eq!(found, MAX_PREFIX_ENTRIES);
    assert!(spent.used.steps > MAX_LOOKUP_STEPS / 4);
    assert!(spent.used.steps <= MAX_LOOKUP_STEPS);
    let (found, spent) = given(40);
    assert_eq!(found, 0);
    assert!(spent.used.steps <= MAX_LOOKUP_STEPS);
    assert!(spent.next.is_some_and(|next| next.steps > MAX_LOOKUP_STEPS));
}

#[test]
fn ordering_the_entries_found_takes_a_step_each() {
    // `か` and 2048 readings after it: two narrowings find them all.
    let mut text = String::new();
    for n in 0..MAX_PREFIX_ENTRIES {
        text.push_str(&format!("か{}\tw{n}\n", hiragana(n)));
    }
    let dictionary = Dictionary::new(text);
    let romaji = Romaji::new("ka").expect("romaji");
    let (words, spent) = dictionary.pattern_words_with_budget(&romaji, Budget::PATTERN);
    assert_eq!(words.len(), MAX_PREFIX_ENTRIES);
    assert!(spent.used.steps > MAX_PREFIX_ENTRIES && spent.used.steps < 2 * MAX_PREFIX_ENTRIES);
    let budget = Budget {
        steps: spent.used.steps - 1,
        ..Budget::PATTERN
    };
    let (words, short) = dictionary.pattern_words_with_budget(&romaji, budget);
    assert!(words.is_empty());
    assert_eq!(short.next.map(|next| next.steps), Some(spent.used.steps));
}

#[test]
fn one_inputs_components_share_the_lookup_steps() {
    // As above: each lookup keeps every range through 10 `あ`, which takes more than half
    // of the steps, and the first finds no word.
    let mut text = String::new();
    for n in 0..MAX_PREFIX_ENTRIES {
        let reading: String = (0..11)
            .map(|bit| if n >> bit & 1 == 1 { 'ず' } else { 'づ' })
            .chain(std::iter::repeat_n('あ', 10))
            .chain(['か'])
            .collect();
        text.push_str(&format!("{reading}\tw{n}\n"));
    }
    let dictionary = Dictionary::new(text);
    let stem = format!("{}{}", "zu".repeat(11), "a".repeat(10));
    let compile = |string: &str, budget: &mut Budget| {
        crate::migemo::compile_with_budget(string, Some(&dictionary), budget, |_, _| Some(()))
    };
    let alone = compile(&format!("{stem}ka"), &mut { Budget::PATTERN });
    assert_eq!(alone.words.len(), MAX_PREFIX_ENTRIES);
    let mut budget = Budget::PATTERN;
    let first = compile(&format!("{stem}ni"), &mut budget);
    assert!(first.words.is_empty() && first.spent.next.is_none());
    assert_eq!(budget.steps, MAX_LOOKUP_STEPS - first.spent.used.steps);
    let second = compile(&format!("{stem}ka"), &mut budget);
    assert!(second.words.is_empty() && second.compiled.is_some());
    assert!(second.spent.next.is_some());
}

#[test]
fn an_skk_okuri_block_and_blank_words_are_left_out() {
    let text = "あいs /愛/[す/愛/]/[し/哀/]/\nか / /蚊/\nき\t \t木\n";
    let dictionary = Dictionary::new(text.to_owned());
    assert_eq!(
        table(&dictionary),
        [
            entry("あい", &["愛"]),
            entry("か", &["蚊"]),
            entry("き", &["木"])
        ]
    );
    assert_eq!(dictionary.skipped(), 8);
}

#[test]
fn a_table_keeps_each_word_once_after_readings_are_made_one() {
    let dictionary = Dictionary::new("あ\ta\nい\ti\nあ\tb\nあ\tc\n".to_owned());
    assert_eq!(
        table(&dictionary),
        [entry("あ", &["a", "b", "c"]), entry("い", &["i"])]
    );
    assert_eq!(dictionary.words.len(), 4);
}

#[test]
fn an_entry_takes_16_bytes_and_a_word_8() {
    assert_eq!(size_of::<Entry>(), 16);
    assert_eq!(size_of::<Span>(), 8);
}

#[test]
fn reads_at_most_its_limit_of_entries() {
    let text = "あ\tw\n".repeat(MAX_ENTRIES + 1);
    let dictionary = Dictionary::new(text);
    assert_eq!(
        (dictionary.readings(), dictionary.words()),
        (1, MAX_WORDS_A_READING)
    );
    // The line past the limit, and the words past one reading's.
    let past = 1 + MAX_ENTRIES - MAX_WORDS_A_READING;
    assert_eq!(dictionary.skipped(), past);
}

#[test]
fn reads_at_most_its_limit_of_words() {
    let lines = MAX_WORDS / MAX_WORDS_A_READING;
    let words = vec!["w"; MAX_WORDS_A_READING].join("\t");
    let mut text = String::with_capacity((lines + 1) * (words.len() + 11));
    for n in 0..=lines {
        text.push_str(&format!("{}\t{words}\n", hiragana(n)));
    }
    let dictionary = Dictionary::new(text);
    assert_eq!(
        (dictionary.readings(), dictionary.words()),
        (lines, MAX_WORDS)
    );
    // Every word of the last line, and the line.
    assert_eq!(dictionary.skipped(), MAX_WORDS_A_READING + 1);
}

#[test]
fn debug_prints_only_counts() {
    let dictionary = Dictionary::new("ひみつ\t秘密\n".to_owned());
    let printed = format!("{dictionary:?}");
    assert!(
        !printed.contains("秘密") && !printed.contains("ひみつ"),
        "{printed}"
    );
}

proptest! {
    #[test]
    fn any_text_gives_a_table_within_its_limits(
        text in "[ぁ-ゖa-z\t /;\\n()゛\u{feff}\\rーゔ漢カ\\\\.*|]{0,400}",
    ) {
        let dictionary = Dictionary::new(text);
        for (reading, words) in table(&dictionary) {
            prop_assert!(super::is_reading(&reading));
            prop_assert!(!words.is_empty() && words.len() <= MAX_WORDS_A_READING);
            prop_assert!(words.iter().all(|word| !word.is_empty() && word.len() <= MAX_WORD));
        }
        let readings: Vec<String> = table(&dictionary).into_iter().map(|(r, _)| r).collect();
        prop_assert!(readings.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// Whatever a dictionary holds, letters with a case among it, a pattern's words keep
    /// within their limits.
    #[test]
    fn any_patterns_words_keep_within_their_limits(
        words in prop::collection::vec("[a-zA-Z漢\u{212a}ſǅßσЖ]{1,16}", 1..64),
        readings in 1usize..600,
        string in "[a-z]{1,4}",
    ) {
        let mut text = String::new();
        for n in 0..readings {
            text.push_str(&format!("{}\t{}\n", hiragana(n), words.join("\t")));
        }
        let dictionary = Dictionary::new(text);
        let Some(romaji) = Romaji::new(&string) else {
            return Ok(());
        };
        let given = dictionary.pattern_words(&romaji);
        let cost: usize = given.iter().map(|word| word_cost(word)).sum();
        prop_assert!(given.len() <= MAX_PATTERN_WORDS);
        prop_assert!(cost <= MAX_PATTERN_WORD_BYTES, "{cost}");
    }
}

proptest! {
    /// What a budget left gives the words it does, and another budget gives the same words
    /// and spends the same exactly when what they spent says so (ADR 0116, Q7), whether its
    /// words, bytes or lookup steps ran out.
    #[test]
    fn a_budget_gives_the_same_words_exactly_when_what_they_spent_says_so(
        readings in 1usize..400,
        long in 1usize..40,
        string in "[a-z]{1,3}",
        budgets in prop::collection::vec((0usize..600, 0usize..20_000, 0usize..60), 2..6),
    ) {
        let mut text = String::new();
        for n in 0..readings {
            let word = "漢".repeat(1 + n % long);
            text.push_str(&format!("{}\t{word}{n}\n", hiragana(n)));
        }
        let dictionary = Dictionary::new(text);
        let Some(romaji) = Romaji::new(&string) else {
            return Ok(());
        };
        let given: Vec<_> = (budgets.iter())
            .map(|&(words, bytes, steps)| {
                let budget = Budget { words, bytes, steps };
                (budget, dictionary.pattern_words_with_budget(&romaji, budget))
            })
            .collect();
        for (budget, (words, spent)) in &given {
            let cost: usize = words.iter().map(|word| word_cost(word)).sum();
            prop_assert_eq!((spent.used.words, spent.used.bytes), (words.len(), cost));
            prop_assert!(budget.holds(spent.used));
            for (other, (other_words, other_spent)) in &given {
                let same = words == other_words && spent.used == other_spent.used;
                prop_assert_eq!(spent.same_within(*other), same, "{:?} {:?}", budget, other);
            }
        }
    }

    /// A lookup takes at most the steps it is given, and with all of a pattern's it finishes
    /// for a common string (ADR 0116, Q7).
    #[test]
    fn a_lookup_takes_at_most_its_steps(
        readings in 1usize..400,
        string in "[a-z]{1,8}",
        steps in 0usize..80,
    ) {
        let mut text = String::new();
        for n in 0..readings {
            text.push_str(&format!("{}\t漢{n}\n", hiragana(n)));
        }
        let dictionary = Dictionary::new(text);
        let Some(romaji) = Romaji::new(&string) else {
            return Ok(());
        };
        let budget = Budget { steps, ..Budget::PATTERN };
        let (_, spent) = dictionary.pattern_words_with_budget(&romaji, budget);
        prop_assert!(spent.used.steps <= steps);
        let (_, all) = dictionary.pattern_words_with_budget(&romaji, Budget::PATTERN);
        prop_assert!(all.used.steps < MAX_LOOKUP_STEPS / 16, "{:?}", all);
        prop_assert!(all.next.is_none_or(|next| next.steps == all.used.steps));
    }
}
