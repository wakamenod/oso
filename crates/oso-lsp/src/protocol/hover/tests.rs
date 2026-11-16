//! Tests of what a hover answers: each shape of its contents, the fences, the bound, and what
//! describes nothing.
//!
//! Must not depend on: `server`, threads or processes.

use serde_json::json;

use super::*;

fn text(result: &Value) -> Option<(String, bool)> {
    hover(result).map(|hover| (hover.text, hover.markdown))
}

#[test]
fn markup_content_is_its_value_as_its_kind_says() {
    let markdown =
        json!({"contents": {"kind": "markdown", "value": "```rust\nfn f()\n```\r\nDocs"}});
    assert_eq!(
        text(&markdown),
        Some(("```rust\nfn f()\n```\nDocs".to_owned(), true))
    );
    let plain = json!({"contents": {"kind": "plaintext", "value": "fn f()"}});
    assert_eq!(text(&plain), Some(("fn f()".to_owned(), false)));
}

#[test]
fn marked_strings_join_as_markdown_with_fences_longer_than_their_backticks() {
    let marked = json!({"contents": [
        {"language": "rust", "value": "fn f() -> &'static str { \"```\" }\n"},
        "Some *docs*.",
        {"language": "ru st;rm", "value": "x"},
    ]});
    let (text, markdown) = text(&marked).unwrap();
    assert!(markdown);
    assert_eq!(
        text,
        "````rust\nfn f() -> &'static str { \"```\" }\n````\n\n---\n\nSome *docs*.\n\n---\n\n```rustrm\nx\n```"
    );
    let one = json!({"contents": "just text"});
    assert_eq!(super::hover(&one).unwrap().text, "just text");
}

#[test]
fn past_the_bound_it_is_cut_at_a_character_and_debug_gives_the_size() {
    let long = "é".repeat(HOVER_BYTES_MAX);
    let result = json!({"contents": {"kind": "markdown", "value": long}});
    let hover = hover(&result).unwrap();
    assert!(hover.text.len() <= HOVER_BYTES_MAX + '…'.len_utf8());
    assert!(hover.text.ends_with("é…"));
    let shown = format!("{hover:?}");
    assert!(!shown.contains('é') && shown.contains("bytes"));
}

#[test]
fn null_blank_or_unreadable_describes_nothing() {
    for result in [
        json!(null),
        json!({"contents": ""}),
        json!({"contents": {"kind": "markdown", "value": " \n "}}),
        json!({"contents": []}),
        json!({"contents": 7}),
        json!({"range": {}}),
    ] {
        assert!(hover(&result).is_none(), "{result}");
    }
}

#[test]
fn pieces_and_a_language_name_are_read_to_their_bounds_and_no_further() {
    use crate::limits::{HOVER_LANGUAGE_BYTES_MAX, HOVER_PIECES_MAX};
    let pieces = |count: usize| {
        let all: Vec<Value> = (0..count).map(|n| json!(format!("p{n}"))).collect();
        hover(&json!({"contents": all})).unwrap().text
    };
    let last = format!("p{}", HOVER_PIECES_MAX - 1);
    assert!(pieces(HOVER_PIECES_MAX).ends_with(&last));
    assert!(
        pieces(HOVER_PIECES_MAX + 1).ends_with(&last),
        "the one past is not read"
    );
    let language = |bytes: usize| {
        let piece = json!({"contents": {"language": "r".repeat(bytes), "value": "x"}});
        let text = hover(&piece).unwrap().text;
        text.lines().next().unwrap().trim_start_matches('`').len()
    };
    assert_eq!(language(HOVER_LANGUAGE_BYTES_MAX), HOVER_LANGUAGE_BYTES_MAX);
    assert_eq!(
        language(HOVER_LANGUAGE_BYTES_MAX + 1),
        HOVER_LANGUAGE_BYTES_MAX
    );
}

#[test]
fn a_huge_piece_is_fenced_by_what_is_kept_of_it() {
    // A value of backticks far past the bound: the fence is longer than any run kept, which
    // is at most the bound, and nothing past it is looked at.
    let value = "`".repeat(4 * HOVER_BYTES_MAX);
    let hover = hover(&json!({"contents": {"language": "rust", "value": value}})).unwrap();
    assert!(hover.text.len() <= HOVER_BYTES_MAX + '…'.len_utf8());
    assert!(hover.text.ends_with('…'));
}

/// Pieces of text and backticks, as a hover's answer may hold them.
fn piece() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let text = "[a-z` \n]{0,40}";
    let kind = prop_oneof![Just("markdown"), Just("plaintext"), Just("other")];
    prop_oneof![
        text.prop_map(Value::from),
        (text, "[a-z#+ ]{0,8}")
            .prop_map(|(value, language)| json!({"language": language, "value": value})),
        (text, kind).prop_map(|(value, kind)| json!({"kind": kind, "value": value})),
        Just(Value::Null),
        any::<i64>().prop_map(Value::from),
    ]
}

proptest::proptest! {
    /// Any contents read, or do not, within the bound and without a panic; and a fenced
    /// block's value holds no run of backticks as long as its fence, so none ends it early.
    #[test]
    fn fuzz(
        pieces in proptest::collection::vec(piece(), 0..8),
        one in piece(),
        value in "[a-z` \n]{0,60}",
    ) {
        for contents in [Value::from(pieces), one] {
            if let Some(hover) = hover(&json!({"contents": contents})) {
                proptest::prop_assert!(hover.text.len() <= HOVER_BYTES_MAX + '…'.len_utf8());
            }
        }
        let Some(fenced) = hover(&json!({"contents": {"language": "rust", "value": value}}))
        else {
            return Ok(());
        };
        let first = fenced.text.lines().next().unwrap_or_default();
        let fence = first.len() - first.trim_start_matches('`').len();
        let body = &fenced.text[first.len()..fenced.text.len() - fence];
        proptest::prop_assert!(fence >= 3);
        proptest::prop_assert!(!body.contains(&"`".repeat(fence)), "{:?}", fenced.text);
    }
}
