//! The JSON reads' bounds: the nesting limit at and past it, strings and arrays cut at their
//! bound, numbers refused out of range; and a fuzz target of arbitrary bytes.
//!
//! Must not depend on: `framing`; messages here are bytes in memory.
//!
//! The fuzz target runs in `just ci` with proptest's default cases. For a long run:
//! `PROPTEST_CASES=200000 cargo test -p oso-lsp --release json::tests::fuzz`.

use proptest::prelude::*;
use serde_json::{Value, json};

use super::{JsonError, array, integer, integer_in, parse, string, uinteger, values_within};
use crate::limits::{JSON_DEPTH_LIMIT, JSON_VALUES_MAX};

fn nested(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

#[test]
fn nesting_is_bounded() {
    assert!(parse(nested(JSON_DEPTH_LIMIT - 1).as_bytes()).is_ok());
    assert!(parse(nested(JSON_DEPTH_LIMIT).as_bytes()).is_err());
    let objects = |depth| format!("{}0{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
    assert!(parse(objects(JSON_DEPTH_LIMIT - 1).as_bytes()).is_ok());
    assert!(parse(objects(JSON_DEPTH_LIMIT).as_bytes()).is_err());
    // Far past it, parsing stops at the limit, with no deep recursion.
    assert!(parse(nested(1_000_000).as_bytes()).is_err());
}

#[test]
fn errors_say_where_and_not_what() {
    let error = parse(br#"{"secret": nope}"#).unwrap_err();
    assert_eq!(
        error,
        JsonError::NotJson {
            line: 1,
            column: 13
        }
    );
    assert_eq!(error.to_string(), "not JSON at line 1, column 13");
    assert!(parse(b"").is_err());
    assert!(parse(b"{} {}").is_err());
    assert!(parse(b"\xff").is_err());
}

#[test]
fn values_are_counted_before_parsing() {
    let zeros = |count: usize| format!("[{}]", vec!["0"; count].join(","));
    // The array and its items.
    assert!(parse(zeros(JSON_VALUES_MAX - 1).as_bytes()).is_ok());
    assert_eq!(
        parse(zeros(JSON_VALUES_MAX).as_bytes()),
        Err(JsonError::TooManyValues)
    );
    assert_eq!(
        JsonError::TooManyValues.to_string(),
        "more than 1000000 JSON values"
    );
    // Inside strings, escaped quotes included, nothing counts.
    assert!(values_within(br#"["a,b[{", "\",[", 1]"#, 4));
    assert!(!values_within(br#"["a,b[{", "\",[", 1, 2]"#, 4));
    assert!(values_within(b"", 1));
    assert!(!values_within(b",", 1));
}

#[test]
fn strings_are_cut_at_a_character_boundary() {
    let value = json!("ab日c");
    assert_eq!(string(&value, 0), Some(""));
    assert_eq!(string(&value, 2), Some("ab"));
    assert_eq!(string(&value, 3), Some("ab"));
    assert_eq!(string(&value, 4), Some("ab"));
    assert_eq!(string(&value, 5), Some("ab日"));
    assert_eq!(string(&value, 6), Some("ab日c"));
    assert_eq!(string(&value, usize::MAX), Some("ab日c"));
    assert_eq!(string(&json!(1), 10), None);
}

#[test]
fn arrays_are_cut_to_a_count() {
    let value = json!([1, 2, 3]);
    assert_eq!(array(&value, 2), Some(&[json!(1), json!(2)][..]));
    assert_eq!(array(&value, 3).map(<[Value]>::len), Some(3));
    assert_eq!(array(&value, 4).map(<[Value]>::len), Some(3));
    assert_eq!(array(&json!({}), 4), None);
}

#[test]
fn numbers_are_refused_out_of_range() {
    assert_eq!(uinteger(&json!(0)), Some(0));
    assert_eq!(uinteger(&json!(2_147_483_647)), Some(2_147_483_647));
    assert_eq!(uinteger(&json!(2_147_483_648_u64)), None);
    assert_eq!(uinteger(&json!(-1)), None);
    assert_eq!(uinteger(&json!(1.5)), None);
    assert_eq!(uinteger(&json!("1")), None);
    assert_eq!(integer(&json!(-2_147_483_648_i64)), Some(i32::MIN));
    assert_eq!(integer(&json!(2_147_483_648_i64)), None);
    assert_eq!(integer(&json!(u64::MAX)), None);
    assert_eq!(integer_in(&json!(5), 0..=5), Some(5));
    assert_eq!(integer_in(&json!(6), 0..=5), None);
    assert_eq!(
        integer_in(&parse(b"1e400").unwrap_or(Value::Null), 0..=5),
        None
    );
}

proptest! {
    /// Arbitrary bytes and JSON-shaped text: parsing never panics, and the reads keep to
    /// their bounds on whatever it gives.
    #[test]
    fn fuzz(
        bytes in prop_oneof![
            prop::collection::vec(any::<u8>(), 0..200),
            "[\\[\\]{}\":,0-9a-z.eE+\\-\\\\ ]{0,300}".prop_map(String::into_bytes),
            (0usize..400).prop_map(|depth| nested(depth).into_bytes()),
        ],
        max in 0usize..40,
    ) {
        let parsed = parse(&bytes);
        // What parses never holds more values than the pre-count said.
        if let Ok(value) = &parsed {
            let mut count = 0;
            let mut stack = vec![value];
            while let Some(value) = stack.pop() {
                count += 1;
                match value {
                    Value::Array(items) => stack.extend(items),
                    Value::Object(map) => stack.extend(map.values()),
                    _ => {}
                }
            }
            // The count is at least the values parsed, exactly so but for empty arrays and objects.
            prop_assert!(!values_within(&bytes, count - 1));
        }
        if let Ok(value) = parsed {
            let mut stack = vec![&value];
            while let Some(value) = stack.pop() {
                if let Some(text) = string(value, max) {
                    prop_assert!(text.len() <= max);
                }
                if let Some(items) = array(value, max) {
                    prop_assert!(items.len() <= max);
                }
                if let Some(number) = integer_in(value, 0..=max as i64) {
                    prop_assert!((0..=max as i64).contains(&number));
                }
                match value {
                    Value::Array(items) => stack.extend(items),
                    Value::Object(map) => stack.extend(map.values()),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn a_cut_string_is_the_longest_prefix_that_fits(text in "\\PC{0,40}", max in 0usize..60) {
        let value = Value::String(text.clone());
        let cut = string(&value, max).unwrap_or_default();
        prop_assert!(text.starts_with(cut));
        prop_assert!(cut.len() <= max);
        let next = text[cut.len()..].chars().next().map_or(0, char::len_utf8);
        prop_assert!(cut.len() == text.len() || cut.len() + next > max);
    }
}
