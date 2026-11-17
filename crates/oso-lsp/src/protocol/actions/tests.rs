//! Tests of code actions as plain values (ADR 0138, step 13): what `initialize` says of them,
//! the ask's parameters with the diagnostics given back, an answer's actions within the bounds,
//! a title that would read as another, and a resolve.
//!
//! Must not depend on: `server`, threads or processes.

use oso_core::diagnostic::{Severity, Tags};
use serde_json::{Value, json};

use super::*;
use crate::limits::{CODE_ACTIONS_MAX, COMMANDS_MAX};
use crate::limits::{EDIT_EDITS_MAX, EDIT_FILES_MAX};
use crate::protocol::workspace_edit::workspace_edit;

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
            resolves: false,
            commands: Vec::new(),
        }
    );
    assert_eq!(acting(json!(false)), Acting::default());
    assert_eq!(
        acting(json!({"resolveProvider": true})),
        Acting {
            offers: true,
            resolves: true,
            commands: Vec::new(),
        }
    );
    assert_eq!(
        super::acting(&json!({"capabilities": {}})),
        Acting::default()
    );
    // The commands it declares, each within its bound, at most so many (13b).
    let long = "c".repeat(ACTION_TITLE_BYTES_MAX + 1);
    let mut names: Vec<Value> = vec![json!("fake.fix"), json!(long), json!(7)];
    names.extend((0..COMMANDS_MAX).map(|n| json!(format!("fake.{n}"))));
    let declared = super::acting(&json!({"capabilities": {
        "executeCommandProvider": {"commands": names},
    }}));
    assert_eq!(declared.commands[0], "fake.fix");
    assert!(
        declared.commands.len() < COMMANDS_MAX,
        "the long and the number left out, the rest cut"
    );
}

#[test]
fn a_command_s_parameters_are_its_name_and_arguments_as_given() {
    let bare = json!({"title": "Run", "command": "fake.run", "arguments": [1, {"a": 2}]});
    assert_eq!(
        execute_command_params(&bare),
        Some((
            "fake.run".to_owned(),
            json!({"command": "fake.run", "arguments": [1, {"a": 2}]})
        ))
    );
    let action = json!({"title": "Fix", "command": {"title": "Fix", "command": "fake.fix"}});
    assert_eq!(
        execute_command_params(&action),
        Some(("fake.fix".to_owned(), json!({"command": "fake.fix"})))
    );
    assert_eq!(execute_command_params(&json!({"title": "Edit only"})), None);
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
fn a_title_s_bidi_and_invisible_characters_are_written_out_and_a_resolve_reads_whole() {
    let list = code_actions(&json!([{"title": "Fix\u{202e}txt.exe\u{200b} it\n"}]));
    assert_eq!(list.actions[0].title, "Fix\\u{202e}txt.exe\\u{200b} it ");
    let resolved = resolved_action(&json!({
        "title": "x",
        "edit": {"changes": {"file:///w/a.rs": [{"range": range(0, 0, 0), "newText": "y"}]}},
        "command": {"title": "then", "command": "fake.then", "arguments": [1]},
    }))
    .unwrap();
    assert!(resolved.edit.is_some_and(|edit| edit.is_ok()));
    assert_eq!(resolved.command.as_deref(), Some("fake.then"));
    assert!(resolved_action(&json!({"no": "title"})).is_none());
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
