//! Tests of rename's values (ADR 0138, step 14): what `initialize` says of it, what a
//! prepare and a rename send, a new name's bounds, and what a prepare's answer gives.
//!
//! Must not depend on: `server`, threads or processes.

use serde_json::json;

use super::*;
use crate::limits::NEW_NAME_BYTES_MAX;

#[test]
fn a_server_says_whether_it_renames_and_checks_a_place_first() {
    let of = |provider: serde_json::Value| {
        renaming(&json!({"capabilities": {"renameProvider": provider}}))
    };
    assert_eq!(renaming(&json!({"capabilities": {}})), Renaming::default());
    assert_eq!(
        of(json!(true)),
        Renaming {
            offers: true,
            prepares: false
        }
    );
    assert_eq!(of(json!(false)), Renaming::default());
    assert_eq!(
        of(json!({})),
        Renaming {
            offers: true,
            prepares: false
        }
    );
    assert_eq!(
        of(json!({"prepareProvider": true})),
        Renaming {
            offers: true,
            prepares: true
        }
    );
    assert_eq!(of(json!("yes")), Renaming::default());
}

#[test]
fn a_rename_sends_the_place_and_the_new_name() {
    let at = Position::new(3, 7);
    assert_eq!(
        rename_params("file:///p/a.rs", at, "renamed"),
        json!({
            "textDocument": {"uri": "file:///p/a.rs"},
            "position": {"line": 3, "character": 7},
            "newName": "renamed",
        })
    );
    assert_eq!(
        prepare_rename_params("file:///p/a.rs", at),
        json!({"textDocument": {"uri": "file:///p/a.rs"}, "position": {"line": 3, "character": 7}})
    );
}

#[test]
fn a_new_name_is_short_and_has_no_control() {
    assert!(new_name_fits("renamed"));
    assert!(new_name_fits("名前"));
    assert!(new_name_fits(&"a".repeat(NEW_NAME_BYTES_MAX)));
    assert!(!new_name_fits(&"a".repeat(NEW_NAME_BYTES_MAX + 1)));
    assert!(!new_name_fits(""));
    assert!(!new_name_fits("two\nlines"));
    assert!(!new_name_fits("a\u{7}b"));
}

#[test]
fn a_prepare_gives_a_range_its_placeholder_the_servers_way_or_nothing() {
    let range = json!({"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 9}});
    let (start, end) = (Position::new(1, 4), Position::new(1, 9));
    assert_eq!(prepared(&range), Some(Some((start, end, None))));
    assert_eq!(
        prepared(&json!({"range": range, "placeholder": "value"})),
        Some(Some((start, end, Some("value".to_owned()))))
    );
    assert_eq!(prepared(&json!({"defaultBehavior": true})), Some(None));
    assert_eq!(prepared(&json!(null)), None);
    assert_eq!(prepared(&json!({"range": "nowhere"})), None);
    assert_eq!(prepared(&json!([1, 2])), None);
}

#[test]
fn a_placeholder_is_shown_on_one_line_and_cut() {
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
    let placeholder = |text: &str| {
        let answer = prepared(&json!({"range": range, "placeholder": text}));
        answer.flatten().and_then(|(_, _, placeholder)| placeholder)
    };
    assert_eq!(placeholder("two\nlines").as_deref(), Some("two lines"));
    assert_eq!(placeholder("a\u{202e}b").as_deref(), Some("a\\u{202e}b"));
    let long = "名".repeat(NEW_NAME_BYTES_MAX);
    let shown = placeholder(&long).unwrap();
    assert!(shown.len() <= NEW_NAME_BYTES_MAX, "{}", shown.len());
    assert!(shown.chars().all(|c| c == '名'));
}
