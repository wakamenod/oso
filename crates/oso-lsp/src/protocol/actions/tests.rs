//! Tests of code actions as plain values (ADR 0138, step 13): what `initialize` says of them,
//! the ask's parameters with the diagnostics given back, an answer's actions within the bounds,
//! and workspace edits read whole or not at all.
//!
//! Must not depend on: `server`, threads or processes.

use oso_core::diagnostic::{Severity, Tags};
use serde_json::{Value, json};

use super::*;
use crate::limits::{CODE_ACTIONS_MAX, EDIT_EDITS_MAX, EDIT_FILES_MAX};

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

#[test]
fn initialize_says_whether_a_server_offers_and_resolves_code_actions() {
    let acting =
        |provider: Value| acting(&json!({"capabilities": {"codeActionProvider": provider}}));
    assert_eq!(
        acting(json!(true)),
        Acting {
            offers: true,
            resolves: false
        }
    );
    assert_eq!(acting(json!(false)), Acting::default());
    assert_eq!(
        acting(json!({"resolveProvider": true})),
        Acting {
            offers: true,
            resolves: true
        }
    );
    assert_eq!(
        super::acting(&json!({"capabilities": {}})),
        Acting::default()
    );
}

#[test]
fn an_ask_gives_the_range_and_the_diagnostics_back_as_the_server_gave_them() {
    let reported = Reported {
        start: Position::new(1, 2),
        end: Position::new(1, 5),
        severity: Severity::Warning,
        tags: Tags {
            unnecessary: true,
            deprecated: false,
        },
        source: "ts".into(),
        code: "2304".into(),
        code_number: true,
        message: "Cannot find name".into(),
        data: Some(r#"{"fix":3}"#.into()),
    };
    let params = code_action_params(
        "file:///w/a.ts",
        (Position::new(1, 3), Position::new(1, 3)),
        &[&reported],
    );
    assert_eq!(
        params,
        json!({
            "textDocument": {"uri": "file:///w/a.ts"},
            "range": range(1, 3, 3),
            "context": {
                "diagnostics": [{
                    "range": range(1, 2, 5),
                    "severity": 2,
                    "code": 2304,
                    "source": "ts",
                    "message": "Cannot find name",
                    "tags": [1],
                    "data": {"fix": 3},
                }],
                "triggerKind": 1,
            },
        })
    );
}

#[test]
fn an_answer_gives_actions_and_commands_cut_to_their_bounds() {
    let long = "t\u{7}".repeat(300);
    let answer = json!([
        {"title": "Import `x`", "kind": "quickfix", "isPreferred": true, "edit": {
            "changes": {"file:///w/a.rs": [{"range": range(0, 0, 0), "newText": "use x;\n"}]},
        }},
        {"title": "Run test", "command": "rust-analyzer.runSingle", "arguments": []},
        {"title": "Extract", "kind": "refactor.extract", "data": {"id": 1}},
        {"title": "Not here", "disabled": {"reason": "no selection"}},
        {"title": long},
        {"no": "title"},
    ]);
    let list = code_actions(&answer);
    assert_eq!(
        list.at,
        [0, 1, 2, 3, 4],
        "the one with no title is left out"
    );
    let [import, run, extract, disabled, cut] = &list.actions[..] else {
        panic!("{list:?}");
    };
    assert!(import.preferred && import.kind.as_deref() == Some("quickfix"));
    let edit = import.edit.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(edit.files[0].edits[0].2, "use x;\n");
    assert_eq!(run.command.as_deref(), Some("rust-analyzer.runSingle"));
    assert!(
        extract.edit.is_none() && extract.command.is_none(),
        "a resolve may give it"
    );
    assert_eq!(disabled.disabled.as_deref(), Some("no selection"));
    assert!(cut.title.len() <= ACTION_TITLE_BYTES_MAX && !cut.title.contains('\u{7}'));
    // Taken as given, for their resolve.
    let taken = take_actions(answer, &list.at[2..3]);
    assert_eq!(
        taken,
        [json!({"title": "Extract", "kind": "refactor.extract", "data": {"id": 1}})]
    );
    // Past the bound, counted.
    let many: Vec<Value> = (0..CODE_ACTIONS_MAX + 3)
        .map(|n| json!({"title": format!("{n}")}))
        .collect();
    let list = code_actions(&Value::Array(many));
    assert_eq!((list.actions.len(), list.left), (CODE_ACTIONS_MAX, 3));
    assert_eq!(code_actions(&Value::Null), ActionList::default());
}

#[test]
fn a_workspace_edit_reads_by_version_or_by_uri_whole_or_not_at_all() {
    let versioned = json!({"documentChanges": [{
        "textDocument": {"uri": "file:///w/a.rs", "version": 7},
        "edits": [{"range": range(2, 0, 3), "newText": "let"}],
    }]});
    let edit = workspace_edit(&versioned).unwrap();
    assert_eq!(edit.files.len(), 1);
    assert_eq!(
        (edit.files[0].uri.as_str(), edit.files[0].version),
        ("file:///w/a.rs", Some(7))
    );
    assert_eq!(
        edit.files[0].edits,
        [(Position::new(2, 0), Position::new(2, 3), "let".to_owned())]
    );
    // Creating, renaming or deleting a file refuses it all (Q6).
    let create = json!({"documentChanges": [{"kind": "create", "uri": "file:///w/b.rs"}]});
    assert!(workspace_edit(&create).is_err());
    // A control character other than a tab or a line feed refuses it all.
    let control = json!({"changes": {"file:///w/a.rs": [{"range": range(0, 0, 0), "newText": "a\u{1b}[2J"}]}});
    assert!(workspace_edit(&control).is_err());
    // Past the files' and the edits' bounds.
    let files: serde_json::Map<String, Value> = (0..=EDIT_FILES_MAX)
        .map(|n| (format!("file:///w/{n}.rs"), json!([])))
        .collect();
    assert!(workspace_edit(&json!({"changes": files})).is_err());
    let edits: Vec<Value> = (0..=EDIT_EDITS_MAX)
        .map(|_| json!({"range": range(0, 0, 0), "newText": ""}))
        .collect();
    assert!(workspace_edit(&json!({"changes": {"file:///w/a.rs": edits}})).is_err());
    let unread = json!({"changes": {"file:///w/a.rs": [{"newText": "x"}]}});
    assert!(workspace_edit(&unread).is_err(), "an edit with no range");
    // A resolve's edit.
    assert!(
        resolved_action(&json!({"title": "x", "edit": versioned})).is_some_and(|edit| edit.is_ok())
    );
    assert!(resolved_action(&json!({"title": "x"})).is_none());
}

/// Any JSON of a few shapes a server might send where an action or an edit goes.
fn wire() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        "[a-z:/.\u{7}\u{1b} \n]{0,12}".prop_map(Value::from),
        Just(range(0, 1, 2)),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        let keys = prop_oneof![
            Just("title"),
            Just("kind"),
            Just("edit"),
            Just("command"),
            Just("disabled"),
            Just("changes"),
            Just("documentChanges"),
            Just("textDocument"),
            Just("uri"),
            Just("version"),
            Just("edits"),
            Just("range"),
            Just("newText"),
            Just("file:///w/a.rs"),
        ];
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::from),
            proptest::collection::btree_map(keys, inner, 0..6).prop_map(|map| {
                Value::Object(
                    map.into_iter()
                        .map(|(key, value)| (key.to_owned(), value))
                        .collect(),
                )
            }),
        ]
    })
}

proptest::proptest! {
    /// Any answer reads within the bounds and without a panic: at most so many actions, each
    /// title and kind cut and free of controls, and any edit read whole within its own.
    #[test]
    fn fuzz(answer in wire(), edit in wire()) {
        let list = code_actions(&answer);
        proptest::prop_assert!(list.actions.len() <= CODE_ACTIONS_MAX);
        proptest::prop_assert_eq!(list.actions.len(), list.at.len());
        for action in &list.actions {
            proptest::prop_assert!(action.title.len() <= ACTION_TITLE_BYTES_MAX);
            proptest::prop_assert!(!action.title.chars().any(char::is_control));
        }
        if let Ok(edit) = workspace_edit(&edit) {
            proptest::prop_assert!(edit.files.len() <= EDIT_FILES_MAX);
            let edits: usize = edit.files.iter().map(|file| file.edits.len()).sum();
            proptest::prop_assert!(edits <= EDIT_EDITS_MAX);
        }
        let taken = take_actions(answer, &list.at);
        proptest::prop_assert!(taken.len() <= list.at.len());
    }
}
