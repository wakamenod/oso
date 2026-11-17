//! Tests of reading `publishDiagnostics`: its fields, what is left out, each bound at it and
//! past it, and a fuzz target over any JSON.
//!
//! Must not depend on: `server`, threads or processes.

use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::limits::DIAGNOSTICS_MAX;

fn item(severity: i64, message: &str) -> Value {
    json!({
        "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
        "severity": severity,
        "message": message,
    })
}

#[test]
fn a_report_reads_its_uri_version_and_diagnostics() {
    let params = json!({
        "uri": "file:///w/oso/src/main.rs",
        "version": 7,
        "diagnostics": [
            {
                "range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 9}},
                "severity": 2,
                "tags": [1, 2, 9],
                "source": "rustc",
                "code": "unused_variables",
                "message": "unused variable: `x`\n`#[warn(unused_variables)]` on by default",
            },
            {
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "code": 308,
                "message": "no severity",
            },
        ],
    });
    let report = published(&params).unwrap();
    assert_eq!(report.uri, "file:///w/oso/src/main.rs");
    assert_eq!(report.version, Some(7));
    let first = &report.diagnostics[0];
    assert_eq!(
        (first.start, first.end),
        (Position::new(3, 4), Position::new(3, 9))
    );
    assert_eq!(first.severity, Severity::Warning);
    assert_eq!(
        first.tags,
        Tags {
            unnecessary: true,
            deprecated: true
        }
    );
    assert_eq!(
        (first.source.as_str(), first.code.as_str()),
        ("rustc", "unused_variables")
    );
    assert!(first.message.starts_with("unused variable"));
    let second = &report.diagnostics[1];
    assert_eq!(second.severity, Severity::Error, "no severity is an error");
    assert_eq!(second.code, "308");
    assert!(
        !format!("{second:?}").contains("no severity"),
        "Debug shows no message"
    );
}

#[test]
fn a_report_with_no_uri_or_list_is_none_and_bad_entries_are_left_out() {
    assert!(published(&json!({"diagnostics": []})).is_none());
    assert!(published(&json!({"uri": "file:///a"})).is_none());
    assert!(published(&json!({"uri": "file:///a", "diagnostics": {}})).is_none());
    let long = format!("file:///{}", "a".repeat(URI_BYTES_MAX));
    assert!(published(&json!({"uri": long, "diagnostics": []})).is_none());
    let report = published(&json!({
        "uri": "file:///a",
        "version": "seven",
        "diagnostics": [
            {"message": "no range"},
            {"range": {"start": {"line": -1, "character": 0}, "end": {"line": 0, "character": 0}}},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}},
        ],
    }))
    .unwrap();
    assert_eq!(report.version, None);
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].message, "");
}

#[test]
fn strings_are_cut_at_their_bounds_on_a_character() {
    let message = format!("{}é", "m".repeat(DIAGNOSTIC_MESSAGE_BYTES_MAX - 1));
    let label = "s".repeat(DIAGNOSTIC_LABEL_BYTES_MAX + 1);
    let report = published(&json!({
        "uri": "file:///a",
        "diagnostics": [{
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "message": message,
            "source": label,
            "code": label,
        }],
    }))
    .unwrap();
    let reported = &report.diagnostics[0];
    assert_eq!(reported.message.len(), DIAGNOSTIC_MESSAGE_BYTES_MAX - 1);
    assert_eq!(reported.source.len(), DIAGNOSTIC_LABEL_BYTES_MAX);
    assert_eq!(reported.code.len(), DIAGNOSTIC_LABEL_BYTES_MAX);
    let exact = "x".repeat(DIAGNOSTIC_MESSAGE_BYTES_MAX);
    let report = published(&json!({"uri": "file:///a", "diagnostics": [item(1, &exact)]})).unwrap();
    assert_eq!(
        report.diagnostics[0].message.len(),
        DIAGNOSTIC_MESSAGE_BYTES_MAX
    );
}

#[test]
fn past_the_bound_the_most_severe_are_kept() {
    let at: Vec<Value> = (0..DIAGNOSTICS_MAX).map(|_| item(2, "w")).collect();
    let report = published(&json!({"uri": "file:///a", "diagnostics": at})).unwrap();
    assert_eq!(report.diagnostics.len(), DIAGNOSTICS_MAX);
    let mut past: Vec<Value> = (0..DIAGNOSTICS_MAX).map(|_| item(4, "hint")).collect();
    past.extend((0..10).map(|_| item(1, "error")));
    past.extend((0..DIAGNOSTICS_MAX).map(|_| item(2, "warning")));
    let report = published(&json!({"uri": "file:///a", "diagnostics": past})).unwrap();
    assert_eq!(report.diagnostics.len(), DIAGNOSTICS_MAX);
    let errors = report
        .diagnostics
        .iter()
        .filter(|reported| reported.severity == Severity::Error)
        .count();
    assert_eq!(errors, 10);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|reported| reported.severity != Severity::Hint)
    );
    assert!(report.bytes() >= DIAGNOSTICS_MAX * std::mem::size_of::<Reported>());
}

#[test]
fn a_long_list_keeps_no_room_past_what_is_kept() {
    let many: Vec<Value> = (0..100_000).map(|_| item(1, "")).collect();
    let report = published(&json!({"uri": "file:///a", "diagnostics": many})).unwrap();
    assert_eq!(report.diagnostics.len(), DIAGNOSTICS_MAX);
    assert!(report.diagnostics.capacity() <= DIAGNOSTICS_MAX);
    assert!(report.bytes() >= report.diagnostics.capacity() * std::mem::size_of::<Reported>());
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        ".{0,12}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 64, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::from),
            prop::collection::hash_map(
                prop_oneof![
                    Just("uri".to_owned()),
                    Just("version".to_owned()),
                    Just("diagnostics".to_owned()),
                    Just("range".to_owned()),
                    Just("start".to_owned()),
                    Just("end".to_owned()),
                    Just("line".to_owned()),
                    Just("character".to_owned()),
                    Just("severity".to_owned()),
                    Just("tags".to_owned()),
                    Just("code".to_owned()),
                    Just("message".to_owned()),
                    ".{0,6}",
                ],
                inner,
                0..6
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    /// Any JSON reads, or does not, within the bounds and without a panic.
    #[test]
    fn fuzz(params in any_json()) {
        if let Some(report) = published(&params) {
            prop_assert!(report.diagnostics.len() <= DIAGNOSTICS_MAX);
            for reported in &report.diagnostics {
                prop_assert!(reported.message.len() <= DIAGNOSTIC_MESSAGE_BYTES_MAX);
                prop_assert!(reported.source.len() <= DIAGNOSTIC_LABEL_BYTES_MAX);
                prop_assert!(reported.code.len() <= DIAGNOSTIC_LABEL_BYTES_MAX);
            }
        }
    }
}

#[test]
fn a_diagnostic_keeps_its_data_up_to_the_bound_and_its_code_s_type() {
    use crate::limits::DIAGNOSTIC_DATA_BYTES_MAX;
    // `"…"` is the string's JSON: its quotes take two bytes of the bound.
    let data = |len: usize| json!("d".repeat(len - 2));
    let span = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
    let report = |data: Value| {
        published(&json!({"uri": "file:///w/a.rs", "diagnostics": [
            {"range": span, "message": "m", "code": 7, "data": data},
            {"range": span, "message": "m", "code": "E7"},
        ]}))
        .unwrap()
    };
    let at = report(data(DIAGNOSTIC_DATA_BYTES_MAX));
    assert_eq!(
        at.diagnostics[0].data.as_ref().map(|data| data.len()),
        Some(DIAGNOSTIC_DATA_BYTES_MAX)
    );
    assert!(at.diagnostics[0].code_number && !at.diagnostics[1].code_number);
    assert!(at.diagnostics[1].data.is_none());
    let past = report(data(DIAGNOSTIC_DATA_BYTES_MAX + 1));
    assert!(
        past.diagnostics[0].data.is_none(),
        "past the bound, none is kept"
    );
    assert_eq!(past.diagnostics.len(), 2, "the diagnostic itself stays");
}
