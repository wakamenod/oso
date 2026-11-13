//! Measures of migemo with a dictionary (ADR 0116, Q7), ignored by default, for the ADR's
//! notes: what building and compiling a needle costs on a key, over the dictionary a person
//! names, and what a key costs with its highlights. The dictionary is read from the file
//! `OSO_MIGEMO_DICTIONARY` names; only these measures read a file, and they print no word of
//! it. Run in a release build on an idle machine:
//! `OSO_MIGEMO_DICTIONARY=… cargo test -p oso-core --release --lib migemo_dictionary -- --ignored --nocapture --test-threads 1`.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

#![expect(clippy::disallowed_macros, reason = "the measures print their timings")]

use std::sync::Arc;
use std::time::Instant;

use super::Editor;
use crate::buffer::Buffer;
use crate::migemo::{Dictionary, Romaji};
use crate::search::Needle;
use crate::window::ViewSize;

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

/// The dictionary `OSO_MIGEMO_DICTIONARY` names, and what reading it into a table took.
fn dictionary() -> (Dictionary, f64) {
    let path = std::env::var_os("OSO_MIGEMO_DICTIONARY").expect("OSO_MIGEMO_DICTIONARY");
    let text = std::fs::read_to_string(path).expect("a UTF-8 dictionary");
    let started = Instant::now();
    let dictionary = Dictionary::new(text);
    (dictionary, ms(started))
}

/// Common romaji words, typed one character at a time.
const WORDS: &[&str] = &[
    "kanji",
    "henkan",
    "nihongo",
    "kensaku",
    "toukyou",
    "oosaka",
    "kyouto",
    "watashi",
    "anata",
    "kare",
    "kanojo",
    "gakkou",
    "sensei",
    "gakusei",
    "daigaku",
    "kaisha",
    "shigoto",
    "denwa",
    "denki",
    "densha",
    "jidousha",
    "kuruma",
    "hikouki",
    "eki",
    "michi",
    "mise",
    "kaimono",
    "tabemono",
    "nomimono",
    "gohan",
    "mizu",
    "ocha",
    "kouhii",
    "sakana",
    "niku",
    "yasai",
    "kudamono",
    "ringo",
    "mikan",
    "tamago",
    "asa",
    "hiru",
    "yoru",
    "kyou",
    "ashita",
    "kinou",
    "mainichi",
    "jikan",
    "fun",
    "byou",
    "tokei",
    "kaze",
    "ame",
    "yuki",
    "sora",
    "umi",
    "yama",
    "kawa",
    "mori",
    "hana",
    "ki",
    "tori",
    "inu",
    "neko",
    "uma",
    "ushi",
    "hito",
    "otoko",
    "onna",
    "kodomo",
    "kazoku",
    "chichi",
    "haha",
    "ani",
    "ane",
    "otouto",
    "imouto",
    "tomodachi",
    "namae",
    "kotoba",
    "bunshou",
    "shousetsu",
    "shinbun",
    "zasshi",
    "hon",
    "toshokan",
    "byouin",
    "isha",
    "kusuri",
    "kenkou",
    "byouki",
    "atama",
    "kokoro",
    "karada",
    "te",
    "ashi",
    "me",
    "mimi",
    "kuchi",
    "ongaku",
    "eiga",
    "shashin",
    "e",
    "iro",
    "aka",
    "ao",
    "shiro",
    "kuro",
    "atarashii",
    "furui",
    "ookii",
    "chiisai",
    "takai",
    "yasui",
    "hayai",
    "osoi",
    "tsuyoi",
    "yowai",
    "utsukushii",
    "kirei",
    "shizuka",
    "nigiyaka",
    "benri",
    "fuben",
    "kantan",
    "muzukashii",
    "omoshiroi",
    "tsumaranai",
    "tanoshii",
    "kanashii",
    "ureshii",
    "sabishii",
    "taisetsu",
    "hitsuyou",
    "jouhou",
    "gijutsu",
    "keisanki",
    "konpyuuta",
    "puroguramu",
    "sekkei",
    "kaihatsu",
    "kankyou",
    "settei",
    "hozon",
    "sakujo",
    "henshuu",
    "kensakuenjin",
    "seikihyougen",
    "mojiretsu",
    "hairetsu",
    "kansuu",
    "hensuu",
    "teisuu",
    "shiken",
    "kekka",
    "mondai",
    "kaiketsu",
    "riyuu",
    "houhou",
    "mokuteki",
    "keikaku",
    "yotei",
    "junbi",
    "kaigi",
    "soudan",
    "renraku",
    "henji",
    "shitsumon",
    "kotae",
    "imi",
    "rei",
    "tsuzuku",
    "hanaji",
    "chijimu",
];

/// Every string measured: each prefix of [`WORDS`], and every string of one to three letters
/// that gives kana.
fn strings() -> Vec<String> {
    let mut strings: Vec<String> = WORDS
        .iter()
        .flat_map(|word| (1..=word.len()).map(|end| word[..end].to_owned()))
        .collect();
    let letters = || (b'a'..=b'z').map(char::from);
    for a in letters() {
        strings.push(a.to_string());
        for b in letters() {
            strings.push(format!("{a}{b}"));
            for c in letters() {
                strings.push(format!("{a}{b}{c}"));
            }
        }
    }
    strings.retain(|string| Romaji::new(string).is_some());
    strings.sort();
    strings.dedup();
    strings
}

/// What a needle with the dictionary's words costs to build and compile on a key: looking up
/// the words, writing the pattern, and the whole needle, which also compiles it.
#[test]
#[ignore = "a measure, in a release build, of the dictionary OSO_MIGEMO_DICTIONARY names"]
fn migemo_dictionary_needle_costs() {
    let (dictionary, read) = dictionary();
    println!(
        "table: {} readings, {} words, {} skipped, built in {read:.1} ms",
        dictionary.readings(),
        dictionary.words(),
        dictionary.skipped()
    );
    let strings = strings();
    let (mut lookups, mut patterns, mut needles) = (Vec::new(), Vec::new(), Vec::new());
    let mut slowest: Vec<(f64, usize, usize, bool)> = Vec::new();
    let mut fell_back = 0;
    for string in &strings {
        let romaji = Romaji::new(string).expect("romaji");
        for _ in 0..3 {
            let started = Instant::now();
            let words = dictionary.pattern_words(&romaji);
            lookups.push(ms(started));
            let started = Instant::now();
            let pattern = romaji.pattern(string, &words);
            patterns.push(ms(started));
            let started = Instant::now();
            let needle = Needle::migemo(string, false, Some(&dictionary));
            let took = ms(started);
            needles.push(took);
            let kept = match &needle {
                Some(Needle::Regexp(regexp)) => regexp.pattern_len() == pattern.len(),
                _ => false,
            };
            if !kept && !words.is_empty() {
                fell_back += 1;
            }
            slowest.push((took, words.len(), pattern.len(), kept));
        }
    }
    for (what, times) in [
        ("lookup", &mut lookups),
        ("pattern", &mut patterns),
        ("needle", &mut needles),
    ] {
        let (p50, p99, max) = spread(times);
        let over = times.iter().filter(|&&time| time > 2.0).count();
        println!(
            "{what:8} n={} p50={p50:.3} p99={p99:.3} max={max:.3} ms, over 2 ms: {over}",
            times.len()
        );
    }
    println!(
        "{} strings; {fell_back} needles dropped their words",
        strings.len()
    );
    slowest.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (took, words, bytes, kept) in slowest.iter().take(15) {
        println!("slow: {took:.3} ms, {words} words, pattern {bytes} B, words kept: {kept}");
    }
    let mut sizes: Vec<(usize, usize)> = slowest.iter().map(|s| (s.2, s.1)).collect();
    sizes.sort_unstable();
    if let Some((bytes, words)) = sizes.last() {
        println!("largest pattern: {bytes} B with {words} words");
    }
}

/// What an isearch key costs with the dictionary, its highlights with it, over 20 MB of
/// short Japanese lines, for strings typed one character at a time.
#[test]
#[ignore = "a measure, in a release build, of the dictionary OSO_MIGEMO_DICTIONARY names"]
fn migemo_dictionary_key_costs() {
    let (dictionary, _) = dictionary();
    let dictionary = Arc::new(dictionary);
    let line = "日本語のテキストと漢字かなカナの行です。\n";
    let text = line.repeat(20 * 1024 * 1024 / line.len());
    let (mut times, mut adds, mut cells_times) = (Vec::new(), Vec::new(), Vec::new());
    let mut keys: Vec<(f64, f64, String)> = Vec::new();
    for word in [
        "kanji",
        "nihongo",
        "kensaku",
        "toukyou",
        "shigoto",
        "atarashii",
    ] {
        for _ in 0..5 {
            let mut editor = Editor::with_buffer(Buffer::from_text("notes.txt", &text));
            editor.set_view_size(ViewSize::new(40, 120));
            editor.set_migemo_dictionary(Some(Arc::clone(&dictionary)));
            editor.isearch_begin(false, false);
            for (at, c) in word.char_indices() {
                let started = Instant::now();
                editor.isearch_add(c.encode_utf8(&mut [0; 4]));
                let added = ms(started);
                let drawn = Instant::now();
                let mut cells = super::MatchCells::default();
                editor.isearch_cells(editor.tiled().0.id(), &editor.screen_now(), &mut cells);
                let cells_took = ms(drawn);
                times.push(ms(started));
                adds.push(added);
                cells_times.push(cells_took);
                keys.push((added, cells_took, word[..=at].to_owned()));
                while let Some(job) = editor.take_isearch_job() {
                    let mut reports = Vec::new();
                    job.run(|report| reports.push(report));
                    for report in reports {
                        editor.isearch_reported(report);
                    }
                }
            }
        }
    }
    for (what, times) in [
        ("key", &mut times),
        ("add", &mut adds),
        ("cells", &mut cells_times),
    ] {
        let (p50, p99, max) = spread(times);
        println!(
            "{what:5} n={} p50={p50:.3} p99={p99:.3} max={max:.3} ms",
            times.len()
        );
    }
    keys.sort_by(|a, b| (b.0 + b.1).total_cmp(&(a.0 + a.1)));
    for (added, cells, string) in keys.iter().take(12) {
        println!("slow key {string:10} add={added:.3} cells={cells:.3} ms");
    }
}

/// What a needle costs over a synthetic dictionary at the limits of a pattern's words:
/// readings that `ka` starts, each with two words of the most bytes, no prefix shared.
#[test]
#[ignore = "a measure, in a release build"]
fn migemo_dictionary_crafted_costs() {
    let kana: Vec<char> = ('ぁ'..='ゖ').collect();
    for (entries, words, chars) in [(2048, 2, 21), (2048, 1, 21), (1024, 2, 21), (2048, 2, 4)] {
        let mut text = String::new();
        let mut n: u32 = 0;
        for entry in 0..entries {
            text.push('か');
            text.push(kana[entry / 86 % 86]);
            text.push(kana[entry % 86]);
            for _ in 0..words {
                text.push('\t');
                for _ in 0..chars {
                    n += 1;
                    text.push(char::from_u32(0x4E00 + n.wrapping_mul(7919) % 20_000).unwrap());
                }
            }
            text.push('\n');
        }
        let dictionary = Dictionary::new(text);
        let romaji = Romaji::new("ka").expect("romaji");
        let words_given = dictionary.pattern_words(&romaji);
        let pattern = romaji.pattern("ka", &words_given);
        let mut times = Vec::new();
        let mut kept = false;
        for _ in 0..20 {
            let started = Instant::now();
            let needle = Needle::migemo("ka", false, Some(&dictionary));
            times.push(ms(started));
            kept = matches!(&needle, Some(Needle::Regexp(r)) if r.pattern_len() == pattern.len());
        }
        let (p50, p99, max) = spread(&mut times);
        println!(
            "crafted {entries}x{words}x{chars}: {} words, pattern {} B, words kept: {kept}, \
             needle p50={p50:.3} p99={p99:.3} max={max:.3} ms",
            words_given.len(),
            pattern.len()
        );
    }
}
