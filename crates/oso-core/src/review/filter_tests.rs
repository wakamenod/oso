//! The tests of a review's filter: every word must match, by path or comment, with smart case.
//!
//! Must not depend on: anything outside the review.

use proptest::prelude::*;

use super::*;

#[test]
fn every_word_must_match_the_path_or_a_comment() {
    let filter = Filter::new("  src  todo ").unwrap();
    assert_eq!(filter.text(), "src  todo");
    assert!(filter.matches(b"src/a.rs", &["a todo here"]));
    assert!(!filter.matches(b"src/a.rs", &["nothing"]));
    assert!(!filter.matches(b"lib/a.rs", &["todo"]));
    assert!(filter.matches(b"src/todo.rs", &[]));
    assert!(Filter::new("   ").is_none(), "no word shows every file");
    assert!(Filter::new("").is_none());
}

#[test]
fn a_word_with_an_upper_case_letter_matches_as_typed() {
    let lower = Filter::new("readme").unwrap();
    assert!(lower.matches(b"README.md", &[]));
    let upper = Filter::new("README").unwrap();
    assert!(upper.matches(b"README.md", &[]));
    assert!(!upper.matches(b"readme.md", &[]));
    assert!(!upper.matches(b"a.rs", &["see readme"]));
    // Beyond ASCII too.
    assert!(
        Filter::new("ärger")
            .unwrap()
            .matches(b"\xc3\x84rger.txt", &[])
    );
    assert!(
        !Filter::new("Ärger")
            .unwrap()
            .matches(b"\xc3\xa4rger.txt", &[])
    );
}

#[test]
fn a_path_that_is_not_utf_8_is_matched_by_what_it_says() {
    let filter = Filter::new("caf").unwrap();
    assert!(filter.matches(b"caf\xe9.txt", &[]));
    assert!(!Filter::new("café").unwrap().matches(b"caf\xe9.txt", &[]));
}

#[test]
fn a_long_text_is_cut_at_a_character() {
    let long = "é".repeat(MAX_FILTER_BYTES);
    let filter = Filter::new(&long).unwrap();
    assert!(filter.text().len() <= MAX_FILTER_BYTES);
    assert!(filter.text().chars().all(|c| c == 'é'));
}

proptest! {
    /// The filter agrees with a plain reading of its rule.
    #[test]
    fn the_filter_is_the_rule(
        words in proptest::collection::vec("[a-cA-C]{1,2}", 1..4),
        path in "[a-cA-C/]{0,8}",
        comments in proptest::collection::vec("[a-cA-C ]{0,8}", 0..3),
    ) {
        let filter = Filter::new(&words.join(" ")).unwrap();
        let comments: Vec<&str> = comments.iter().map(String::as_str).collect();
        let expected = words.iter().all(|word| {
            let found = |text: &str| if word.chars().any(char::is_uppercase) {
                text.contains(word.as_str())
            } else {
                text.to_lowercase().contains(&word.to_lowercase())
            };
            found(&path) || comments.iter().any(|comment| found(comment))
        });
        prop_assert_eq!(filter.matches(path.as_bytes(), &comments), expected);
    }
}

#[test]
fn a_word_is_a_literal_and_words_past_the_most_are_left_out() {
    let filter = Filter::new("a.b (x").unwrap();
    assert!(filter.matches(b"a.b/(x", &[]));
    assert!(!filter.matches(b"a.b", &[]), "both words must match");
    assert!(!filter.matches(b"axb/(x", &[]), "`.` stands for itself");
    let many: Vec<String> = (0..MAX_WORDS + 5).map(|n| format!("w{n}")).collect();
    let filter = Filter::new(&many.join(" ")).unwrap();
    let kept = many[..MAX_WORDS].join("/");
    assert!(
        filter.matches(kept.as_bytes(), &[]),
        "the words after are left out"
    );
    assert_eq!(filter.text(), many.join(" "), "the text stays as typed");
}
