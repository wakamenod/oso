//! Tests of completion (ADR 0076): components in any order, each style and affix, smart
//! case, the order candidates show in, and narrowing.
//!
//! Must not depend on: `editor` or `minibuffer`; these run on strings alone.

use std::ops::Range;
use std::sync::Arc;

use proptest::prelude::*;

use super::query::MAX_COMPONENTS;
use super::{Candidate, Candidates, MAX_INPUT_BYTES, Query};

fn candidate(text: &str) -> Candidate {
    Candidate::new(text, "", "")
}

fn matches(input: &str, text: &str) -> bool {
    Query::parse(input).matches(&candidate(text))
}

fn highlights(input: &str, text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    Query::parse(input).highlights(&candidate(text), &mut out);
    out
}

fn list(texts: &[&str], history: &[&str]) -> Candidates {
    let items: Arc<[Candidate]> = texts.iter().map(|text| candidate(text)).collect();
    Candidates::new(items, history.iter().copied())
}

fn shown(candidates: &Candidates) -> Vec<&str> {
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text())
        .collect()
}

#[test]
fn components_match_in_any_order() {
    assert!(matches("buf save", "save-buffer"));
    assert!(matches("save buf", "save-buffer"));
    assert!(!matches("save kill", "save-buffer"));
    assert!(matches("", "anything"));
    assert!(matches("   ", "anything"));
}

#[test]
fn a_space_after_a_backslash_stays_in_its_component() {
    assert!(matches(r"a\ b", "a b"));
    assert!(!matches(r"a\ b", "b a"));
    assert!(matches("a b", "b a"));
}

#[test]
fn case_is_ignored_unless_the_input_has_an_upper_case_letter() {
    assert!(matches("save", "Save-Buffer"));
    assert!(matches("Save", "Save-Buffer"));
    assert!(!matches("Save", "save-buffer"));
    // One upper-case letter anywhere makes every component match case.
    assert!(!matches("save B", "save-buffer"));
    assert!(matches("ä", "ÄRGER"));
}

#[test]
fn each_affix_picks_its_style() {
    // `!`: must not match.
    assert!(!matches("!buf", "save-buffer"));
    assert!(matches("save !kill", "save-buffer"));
    assert!(matches("buf!", "goto-line"));
    // `,`: initialism.
    assert!(matches(",sbkt", "save-buffers-kill-terminal"));
    assert!(matches("sbk,", "save-buffers-kill-terminal"));
    assert!(!matches(",sbx", "save-buffers-kill-terminal"));
    // `=`: literal only.
    assert!(matches("=a.b", "a.b"));
    assert!(!matches("=a.b", "axb"));
    // `^`: at the start.
    assert!(matches("^save", "save-buffer"));
    assert!(!matches("^buffer", "save-buffer"));
    // `~`: flex.
    assert!(matches("~fwc", "forward-char"));
    assert!(!matches("~cwf", "forward-char"));
    // An affix alone matches everything.
    assert!(matches("!", "anything"));
}

#[test]
fn an_affix_and_matches_the_annotation() {
    let item = Candidate::new(
        "set-mark-command",
        "C-SPC",
        "Set the mark where the region starts.",
    );
    assert!(Query::parse("&region").matches(&item));
    assert!(!Query::parse("region").matches(&item));
    assert!(!Query::parse("&C-SPC").matches(&item));
    let mut out = Vec::new();
    Query::parse("&region mark").highlights(&item, &mut out);
    assert_eq!(out, vec![4..8]);
}

#[test]
fn a_component_with_an_expression_matches_as_one_or_as_a_literal() {
    assert!(matches("for.*char", "forward-char"));
    assert!(matches("^for", "forward-char"));
    assert!(matches("char$", "forward-char"));
    assert!(!matches("word$", "forward-char"));
    assert!(matches("a.b", "a.b"));
    assert!(matches("a.b", "axb"));
    // One that does not compile is a literal.
    assert!(matches("a(b", "xa(by"));
    assert!(!matches("a(b", "ab"));
    // Case as the input says.
    assert!(matches("for.*char", "FORWARD-CHAR"));
    assert!(!matches("For.*char", "forward-char"));
    // Negated.
    assert!(!matches("!for.*char", "forward-char"));
}

#[test]
fn the_dependency_keeps_the_unicode_tables_perl_classes_need() {
    // Tests here pull in `regex-syntax`'s every table through `proptest`, the app does not:
    // this is what keeps `\w`, `\d`, `\b` and `\pL` compiling there.
    let manifest = include_str!("../../Cargo.toml");
    assert!(manifest.contains("\"unicode-perl\"") && manifest.contains("\"unicode-gencat\""));
}

#[test]
fn a_component_longer_than_a_candidate_skips_it_and_a_long_one_stays_cheap() {
    let long = "a".repeat(1000);
    assert!(!matches(&long, "save-buffer"));
    assert!(matches(&long, &format!("x{long}y")));
}

#[test]
fn an_expression_too_large_to_compile_is_a_literal() {
    let big = format!("(a{{1000}}){{1000}}{}", "b");
    assert!(!matches(&big, "ab"));
    assert!(matches(&big, &format!("x{big}y")));
    let deep = format!("{}a{}", "(".repeat(64), ")".repeat(64));
    assert!(!matches(&deep, "a"));
}

#[test]
fn highlights_are_where_each_component_matched_in_the_text() {
    assert_eq!(highlights("buf save", "save-buffer"), [0..4, 5..8]);
    assert_eq!(highlights(",sb", "save-buffer"), [0..1, 5..6]);
    assert_eq!(highlights("~svb", "save-buffer"), [0..1, 2..3, 5..6]);
    assert_eq!(highlights("ave av", "save"), vec![1..4]);
    assert_eq!(highlights("!x save", "save"), vec![0..4]);
    assert_eq!(highlights("r.*d", "forward"), vec![2..7]);
    // Offsets are the original text's, whatever folding did to its lengths.
    let text = "İx K bar";
    let range = highlights("bar", text)[0].clone();
    assert_eq!(&text[range], "bar");
    let range = highlights("k", text)[0].clone();
    assert_eq!(&text[range], "K");
}

#[test]
fn the_order_is_history_then_length_then_alphabetical() {
    let candidates = list(
        &["undo-redo", "undo", "forward-char", "goto-line", "abc"],
        &["goto-line", "undo-redo"],
    );
    assert_eq!(
        shown(&candidates),
        ["goto-line", "undo-redo", "abc", "undo", "forward-char"]
    );
}

#[test]
fn an_exact_match_shows_first_and_the_rest_keep_their_order() {
    let mut candidates = list(&["undo-redo", "undo", "redo"], &["undo-redo"]);
    candidates.set_input("undo");
    assert_eq!(shown(&candidates), ["undo", "undo-redo"]);
    assert_eq!(candidates.selected_candidate().unwrap().text(), "undo");
    candidates.set_input("und");
    assert_eq!(shown(&candidates), ["undo-redo", "undo"]);
}

#[test]
fn selection_stops_at_both_ends_and_resets_with_the_input() {
    let mut candidates = list(&["a1", "a2", "a3", "b"], &[]);
    assert_eq!(candidates.selected(), Some(0));
    candidates.move_by(-1);
    assert_eq!(candidates.selected(), Some(0));
    candidates.move_by(10);
    assert_eq!(candidates.selected(), Some(3));
    candidates.set_input("a");
    assert_eq!((candidates.selected(), candidates.len()), (Some(0), 3));
    candidates.set_input("z");
    assert_eq!(candidates.selected(), None);
    assert!(candidates.selected_candidate().is_none());
    assert_eq!(candidates.total(), 4);
}

#[test]
fn the_rows_shown_follow_the_selection() {
    let texts: Vec<String> = (0..30).map(|n| format!("c{n:02}")).collect();
    let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
    let mut candidates = list(&texts, &[]);
    assert_eq!(candidates.keep_in_view(18), 0);
    candidates.move_by(17);
    assert_eq!(candidates.keep_in_view(18), 0);
    candidates.move_by(1);
    assert_eq!(candidates.keep_in_view(18), 1);
    candidates.select(29);
    assert_eq!(candidates.keep_in_view(18), 12);
    candidates.select(5);
    assert_eq!(candidates.keep_in_view(18), 5);
    // Fewer rows than before keep the selection in view, and more show what is above.
    assert_eq!(candidates.keep_in_view(1), 5);
    candidates.select(29);
    candidates.keep_in_view(1);
    assert_eq!(candidates.keep_in_view(18), 12);
}

#[test]
fn narrowing_needs_every_component_kept_or_longer_at_its_end() {
    let narrows = |new: &str, old: &str| Query::parse(new).narrows(&Query::parse(old));
    assert!(narrows("save", "sav"));
    assert!(narrows("sav buf", "sav"));
    assert!(narrows("sav", ""));
    assert!(narrows("saV", "sa"));
    assert!(narrows(",sbk", ",sb"));
    assert!(!narrows("sa", "Sa"));
    assert!(!narrows("sav", "save"));
    assert!(!narrows("buf sav", "sav"));
    assert!(!narrows("!bu", "!b"));
    assert!(narrows("!b x", "!b"));
    assert!(!narrows("sav!", "sav"));
    assert!(!narrows("sav~", "sav"));
    assert!(!narrows("a|b", "a"));
    assert!(!narrows("a.", "a"));
    // A literal read as an expression too finds more, and the other way fewer.
    assert!(!narrows("a.c", "=a.c"));
    assert!(!narrows("=a.c", "a.c"));
}

#[test]
fn an_input_too_long_or_of_too_many_components_matches_nothing_until_it_is_short_again() {
    let mut candidates = list(&["save-buffer", "undo"], &[]);
    candidates.set_input(&"s".repeat(MAX_INPUT_BYTES + 1));
    assert_eq!((candidates.len(), candidates.selected()), (0, None));
    candidates.set_input("sa");
    assert_eq!(shown(&candidates), ["save-buffer"]);
    candidates.set_too_long();
    assert!(candidates.is_empty());
    candidates.set_input("");
    assert_eq!(candidates.len(), 2, "the empty input matches all again");
    let many = vec!["a"; MAX_COMPONENTS + 1].join(" ");
    candidates.set_input(&many);
    assert!(candidates.is_empty());
    assert!(!Query::parse(&many).matches(&candidate("a")));
}

/// A literal as long as an answer may be.
const LONG: &str = concat!(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
);

/// Matching at 5000 candidates stays within ADR 0076's budget of 1 ms a key. Run it by hand
/// in release: `cargo test --release -p oso-core matching_5000 -- --ignored`.
#[test]
#[ignore = "a timing to read, run by hand in release"]
fn matching_5000_candidates_takes_under_a_millisecond_a_key() {
    use std::time::Instant;
    let words = [
        "save", "buffer", "kill", "line", "forward", "char", "word", "mark", "region",
    ];
    let items: Arc<[Candidate]> = (0..5000)
        .map(|n| {
            let text = format!(
                "{}-{}-{}-{n}",
                words[n % 9],
                words[n / 9 % 9],
                words[n / 81 % 9]
            );
            let note = format!(
                "Do the {} thing to the {} at every cursor.",
                words[n % 7],
                words[n % 5]
            );
            Candidate::new(text, "C-x C-s", note)
        })
        .collect();
    let long_more = format!("{LONG} z");
    let steps: &[&[&str]] = &[
        &["e", "ea", "eab"],
        &["e", "", "e"],
        &["buf", "buf sav"],
        &["~svbf"],
        &[",sbk"],
        &["a.b", "a.bc"],
        &["!x", "!x e"],
        // A long literal, narrowing as typed, and one more component after it.
        &[&LONG[..999], LONG, &long_more],
    ];
    let mut highlights = Vec::new();
    for inputs in steps {
        let mut candidates = Candidates::new(Arc::clone(&items), []);
        for input in *inputs {
            let start = Instant::now();
            candidates.set_input(input);
            let matched = start.elapsed();
            let start = Instant::now();
            for place in 0..crate::complete::ROWS.min(candidates.len()) {
                let candidate = candidates.get(place).unwrap();
                candidates.query().highlights(candidate, &mut highlights);
            }
            // A debug build is several times slower; the budget is a release build's.
            assert!(
                cfg!(debug_assertions) || matched.as_micros() < 1000,
                "{input}: {} matches in {matched:?}, 18 rows highlighted in {:?}",
                candidates.len(),
                start.elapsed()
            );
        }
    }
}

#[test]
fn debug_says_nothing_of_the_input() {
    let mut candidates = list(&["secret-word"], &[]);
    candidates.set_input("secret");
    assert!(!format!("{candidates:?}").contains("secret"));
    assert!(!format!("{:?}", Query::parse("secret")).contains("secret"));
}

fn words() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-cA-C\\-]{1,6}", 1..40)
}

proptest! {
    #[test]
    fn components_in_any_order_match_the_same(
        texts in words(),
        parts in prop::collection::vec("[a-c!,~^]{1,3}", 1..4),
    ) {
        let forward = Query::parse(&parts.join(" "));
        let mut reversed = parts.clone();
        reversed.reverse();
        let backward = Query::parse(&reversed.join(" "));
        for text in &texts {
            let item = candidate(text);
            prop_assert_eq!(forward.matches(&item), backward.matches(&item));
        }
    }

    #[test]
    fn narrowing_gives_what_a_full_pass_gives(
        texts in words(),
        inputs in prop::collection::vec("[a-cA-C !,~^=&.*\\\\]{0,3}", 1..6),
    ) {
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let mut typed = list(&texts, &[]);
        let mut input = String::new();
        for part in &inputs {
            input.push_str(part);
            typed.set_input(&input);
            let mut fresh = list(&texts, &[]);
            fresh.set_input(&input);
            prop_assert_eq!(shown(&typed), shown(&fresh));
        }
    }

    #[test]
    fn typing_never_moves_a_candidate(texts in words(), input in "[a-c]{0,4}") {
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let mut candidates = list(&texts, &texts[..texts.len() / 2]);
        let all: Vec<String> = shown(&candidates).iter().map(|s| (*s).to_owned()).collect();
        candidates.set_input(&input);
        // What shows, but an exact match moved first, keeps the order of the whole list.
        let mut at = 0;
        for text in shown(&candidates).into_iter().filter(|text| *text != input) {
            let place = all[at..].iter().position(|other| other == text);
            prop_assert!(place.is_some());
            at += place.unwrap_or(0) + 1;
        }
    }

    /// Batches merged as a walk sends them show what the whole list made at once shows, in
    /// its order and with its matches (ADR 0114).
    #[test]
    fn merged_batches_give_what_the_whole_list_gives(
        texts in words(),
        cuts in prop::collection::vec(0usize..40, 0..5),
        input in "[a-cA-C !~^]{0,3}",
    ) {
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let history: Vec<&str> = texts.iter().copied().step_by(3).collect();
        let mut cuts: Vec<usize> = cuts.into_iter().map(|cut| cut.min(texts.len())).collect();
        cuts.push(texts.len());
        cuts.sort_unstable();
        let mut merged = list(&[], &history);
        merged.set_input(&input);
        let mut from = 0;
        for cut in cuts {
            merged.merge(texts[from..cut].iter().map(|text| candidate(text)), history.iter().copied());
            from = cut;
        }
        let mut whole = list(&texts, &history);
        whole.set_input(&input);
        prop_assert_eq!(shown(&merged), shown(&whole));
        let order = |candidates: &Candidates| candidates.order().to_vec();
        prop_assert_eq!(order(&merged), order(&whole));
    }

    #[test]
    fn highlights_are_inside_the_text_on_char_boundaries(
        text in "[a-zA-ZİßK\\- ]{0,12}",
        input in "[a-zİk~,^ .*?]{0,6}",
    ) {
        let item = candidate(&text);
        let query = Query::parse(&input);
        let mut out = Vec::new();
        query.highlights(&item, &mut out);
        let mut end = 0;
        for range in &out {
            prop_assert!(range.start >= end && range.start < range.end && range.end <= text.len());
            prop_assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            end = range.end;
        }
    }
}

#[test]
fn a_match_on_the_worker_gives_what_matching_here_gives() {
    use std::sync::atomic::AtomicU64;

    use super::{CompletionJob, CompletionReport, Report};
    let items: Arc<[Candidate]> = (0..6000)
        .map(|index| Candidate::new(format!("name {index}"), "", ""))
        .collect();
    let mut here = Candidates::new(Arc::clone(&items), []);
    let mut there = Candidates::new(items, []);
    // Narrowing, a new component, one taken back, and a regular expression.
    for input in ["1", "12", "12 3", "2", "na.*9", ""] {
        here.set_input(input);
        let live = Arc::new(AtomicU64::new(1));
        let job = CompletionJob::matching(there.matching(input).unwrap(), 1, live);
        let Some(CompletionReport(Report::Matched { matched, .. })) = job.run() else {
            panic!("matched");
        };
        there.apply(matched, false);
        assert_eq!(here.matches, there.matches, "{input:?}");
        assert_eq!(here.exact, there.exact, "{input:?}");
    }
}

#[test]
fn a_row_shown_only_while_nothing_is_typed_is_hidden_here_and_on_the_worker() {
    use std::sync::atomic::AtomicU64;

    use super::{CompletionJob, CompletionReport, Report};
    // A row like M-y's secret kill, whose text every input below would match but for the rule
    // (ADR 0091).
    let items: Arc<[Candidate]> = [
        Candidate::new("alpha kill", "", "").hidden_by_input(),
        candidate("alpha"),
        candidate("beta"),
    ]
    .into();
    let mut here = Candidates::new(Arc::clone(&items), []);
    assert_eq!(here.len(), 3, "it shows while nothing is typed");
    let mut there = Candidates::new(items, []);
    // A first input, then one that narrows it, so both of `match_over`'s passes are taken.
    for input in ["al", "alp"] {
        here.set_input(input);
        let live = Arc::new(AtomicU64::new(1));
        let job = CompletionJob::matching(there.matching(input).unwrap(), 1, live);
        let Some(CompletionReport(Report::Matched { matched, .. })) = job.run() else {
            panic!("matched");
        };
        there.apply(matched, false);
        for candidates in [&here, &there] {
            let texts: Vec<&str> = (0..candidates.len())
                .filter_map(|place| candidates.get(place).map(Candidate::text))
                .collect();
            assert_eq!(texts, ["alpha"], "{input:?}");
        }
    }
}

#[test]
fn a_merge_on_the_worker_gives_what_merging_here_gives() {
    use std::sync::atomic::AtomicU64;

    use super::{CompletionJob, Merge, Report};
    let first: Vec<String> = (0..6000).map(|index| format!("src/{index}.rs")).collect();
    let more: Vec<Box<str>> = (0..50)
        .map(|index| format!("a/{index}.md").into())
        .collect();
    let history = ["src/42.rs", "a/7.md"];
    let mut here = list(
        &first.iter().map(String::as_str).collect::<Vec<_>>(),
        &history,
    );
    here.set_input("4");
    for input in ["4", "7"] {
        let job = CompletionJob::merge(
            Merge {
                items: Arc::clone(here.items()),
                order: Arc::clone(here.order()),
                matched: here.matched(),
                batches: vec![more.clone().into()],
                history: history.iter().map(|text| (*text).to_owned()).collect(),
                input: Some(input.to_owned()),
                dictionary: None,
            },
            1,
            Arc::new(AtomicU64::new(1)),
        );
        let Some(super::CompletionReport(Report::Merged { merged, .. })) = job.run() else {
            panic!("a merge reports what it merged");
        };
        let mut there = list(&[], &[]);
        there.grown(merged.items, merged.order, merged.matched);
        let mut whole = list(
            &first
                .iter()
                .map(String::as_str)
                .chain(more.iter().map(|path| &**path))
                .collect::<Vec<_>>(),
            &history,
        );
        whole.set_input(input);
        assert_eq!(shown(&there), shown(&whole), "for {input}");
    }
}
