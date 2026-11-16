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
