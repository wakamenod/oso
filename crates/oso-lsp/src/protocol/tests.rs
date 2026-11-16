//! Tests of `protocol`: telling messages apart, the answers to a server's requests, the
//! settings it is given, and what progress and messages say.
//!
//! Must not depend on: `server`, threads or processes.

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::limits::MESSAGE_BYTES_MAX;

#[test]
fn messages_are_told_apart_by_their_fields() {
    assert_eq!(
        classify(json!({"jsonrpc": "2.0", "id": 3, "result": {"a": 1}})),
        Ok(Incoming::Answer {
            id: 3,
            outcome: Outcome::Result(json!({"a": 1})),
        })
    );
    assert_eq!(
        classify(json!({"id": 4, "result": null})),
        Ok(Incoming::Answer {
            id: 4,
            outcome: Outcome::Result(Value::Null),
        })
    );
    assert_eq!(
        classify(json!({"id": 5, "error": {"code": -32603, "message": "boom"}})),
        Ok(Incoming::Answer {
            id: 5,
            outcome: Outcome::Error {
                code: -32603,
                message: "boom".into(),
            },
        })
    );
    assert_eq!(
        classify(json!({"id": "x", "result": 1})),
        Ok(Incoming::Stray)
    );
    assert_eq!(
        classify(json!({"id": 1, "method": "a/b", "params": [1]})),
        Ok(Incoming::Request {
            id: RequestId::Number(1),
            method: "a/b".into(),
            params: json!([1]),
        })
    );
    assert_eq!(
        classify(json!({"id": "k", "method": "a/b"})),
        Ok(Incoming::Request {
            id: RequestId::Text("k".into()),
            method: "a/b".into(),
            params: Value::Null,
        })
    );
    assert_eq!(
        classify(json!({"method": "n"})),
        Ok(Incoming::Notification {
            method: "n".into(),
            params: Value::Null,
        })
    );
}

#[test]
fn what_is_not_json_rpc_is_told() {
    for message in [
        json!([1]),
        json!("text"),
        json!({}),
        json!({"id": 1}),
        json!({"method": 3}),
        json!({"method": "m", "id": [1]}),
        json!({"method": "m", "id": null}),
        json!({"method": "m".repeat(METHOD_BYTES_MAX + 1)}),
    ] {
        assert_eq!(classify(message.clone()), Err(NotRpc), "{message}");
    }
    assert!(classify(json!({"method": "m".repeat(METHOD_BYTES_MAX)})).is_ok());
}

#[test]
fn an_id_oso_cannot_give_back_gets_no_answer() {
    let long = "i".repeat(ID_BYTES_MAX + 1);
    assert_eq!(
        classify(json!({"id": long, "method": "m"})),
        Ok(Incoming::Unanswerable { method: "m".into() })
    );
    assert_eq!(
        classify(json!({"id": 1.5, "method": "m"})),
        Ok(Incoming::Unanswerable { method: "m".into() })
    );
    let at_bound = "i".repeat(ID_BYTES_MAX);
    assert!(matches!(
        classify(json!({"id": at_bound, "method": "m"})),
        Ok(Incoming::Request { .. })
    ));
}

#[test]
fn an_error_message_is_cut() {
    let long = "é".repeat(SHOWN_BYTES_MAX);
    let Ok(Incoming::Answer {
        outcome: Outcome::Error { message, .. },
        ..
    }) = classify(json!({"id": 1, "error": {"code": 1, "message": long}}))
    else {
        panic!("an error answer");
    };
    assert!(message.len() <= SHOWN_BYTES_MAX);
    assert!(message.chars().all(|c| c == 'é'));
}

#[test]
fn bodies_are_json_rpc() {
    let sent: Value = serde_json::from_slice(&request(7, "initialize", json!({}))).unwrap();
    assert_eq!(
        sent,
        json!({"jsonrpc": "2.0", "id": 7, "method": "initialize", "params": {}})
    );
    let sent: Value = serde_json::from_slice(&notification("exit", Value::Null)).unwrap();
    assert_eq!(
        sent,
        json!({"jsonrpc": "2.0", "method": "exit", "params": null})
    );
    let sent: Value =
        serde_json::from_slice(&answer(&RequestId::Text("a".into()), Ok(json!(1)))).unwrap();
    assert_eq!(sent, json!({"jsonrpc": "2.0", "id": "a", "result": 1}));
    let sent: Value =
        serde_json::from_slice(&answer(&RequestId::Number(2), Err((-1, "no")))).unwrap();
    assert_eq!(
        sent,
        json!({"jsonrpc": "2.0", "id": 2, "error": {"code": -1, "message": "no"}})
    );
}

#[test]
fn initialize_claims_only_what_the_steps_use() {
    let options = json!({"cargo": {"targetDir": true}});
    let params = initialize_params(&Initialize {
        root_uri: "file:///w/oso",
        root_name: "oso",
        options: Some(&options),
    });
    assert_eq!(params["rootUri"], "file:///w/oso");
    assert_eq!(
        params["workspaceFolders"],
        json!([{"uri": "file:///w/oso", "name": "oso"}])
    );
    assert_eq!(params["initializationOptions"], options);
    assert_eq!(
        params["capabilities"],
        json!({
            "general": {"positionEncodings": ["utf-8", "utf-16"]},
            "window": {"workDoneProgress": true},
            "workspace": {"configuration": true},
            "textDocument": {
                "synchronization": {"didSave": true},
                "definition": {"linkSupport": true},
                "references": {},
                "completion": {
                    "completionItem": {"snippetSupport": false, "insertReplaceSupport": true},
                    "contextSupport": true,
                },
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "publishDiagnostics": {
                    "versionSupport": true,
                    "tagSupport": {"valueSet": [1, 2]},
                },
            },
        })
    );
    let params = initialize_params(&Initialize {
        root_uri: "file:///w",
        root_name: "w",
        options: None,
    });
    assert!(params.get("initializationOptions").is_none());
}

#[test]
fn the_chosen_encoding_is_read() {
    let chosen = |result: Value| chosen_encoding(&result);
    assert_eq!(chosen(json!({"capabilities": {}})), Some(Encoding::Utf16));
    assert_eq!(
        chosen(json!({"capabilities": {"positionEncoding": "utf-8"}})),
        Some(Encoding::Utf8)
    );
    assert_eq!(
        chosen(json!({"capabilities": {"positionEncoding": "utf-32"}})),
        None
    );
    assert_eq!(chosen(json!({})), None, "no capabilities at all");
    assert_eq!(
        server_name(&json!({"serverInfo": {"name": "rust-analyzer"}})),
        Some("rust-analyzer")
    );
}

#[test]
fn a_servers_requests_get_oso_s_answers() {
    let options = json!({"cargo": {"targetDir": true}, "check": {"command": "clippy"}});
    let section = Some("rust-analyzer");
    let params = json!({"items": [
        {"section": "rust-analyzer"},
        {"section": "rust-analyzer.cargo"},
        {"section": "rust-analyzer.check.command"},
        {"section": "rust-analyzer.nothing"},
        {"section": "rust-analyzerx"},
        {"section": "gopls"},
        {},
    ]});
    assert_eq!(
        answer_for("workspace/configuration", &params, section, &options),
        Ok(json!([
            options,
            {"targetDir": true},
            "clippy",
            null,
            null,
            null,
            null,
        ]))
    );
    let many = json!({"items": vec![json!({}); CONFIGURATION_ITEMS_MAX + 1]});
    assert!(answer_for("workspace/configuration", &many, section, &options).is_err());
    let at_bound = json!({"items": vec![json!({}); CONFIGURATION_ITEMS_MAX]});
    assert!(answer_for("workspace/configuration", &at_bound, section, &options).is_ok());
    assert!(answer_for("workspace/configuration", &json!({}), section, &options).is_err());
    // Large settings asked for many times: refused before any answer is built.
    let large = json!({"text": "x".repeat(MESSAGE_BYTES_MAX / 100)});
    let hundred = json!({"items": vec![json!({"section": "rust-analyzer"}); 200]});
    assert_eq!(
        answer_for("workspace/configuration", &hundred, section, &large).map_err(|e| e.0),
        Err(INVALID_PARAMS)
    );
    let none = Value::Null;
    for (method, expected) in [
        ("window/workDoneProgress/create", Value::Null),
        ("client/registerCapability", Value::Null),
        ("workspace/applyEdit", json!({"applied": false})),
        ("window/showDocument", json!({"success": false})),
        ("window/showMessageRequest", Value::Null),
    ] {
        assert_eq!(answer_for(method, &none, section, &options), Ok(expected));
    }
    assert_eq!(
        answer_for("workspace/somethingElse", &none, section, &options).map_err(|e| e.0),
        Err(METHOD_NOT_FOUND)
    );
}

#[test]
fn progress_is_read() {
    let read = |params: Value| progress(&params);
    assert_eq!(
        read(json!({"token": 1, "value": {"kind": "begin", "title": "Indexing"}})),
        Some((Token::Number(1), Progress::Working(None)))
    );
    assert_eq!(
        read(json!({"token": "t", "value": {"kind": "report", "percentage": 34}})),
        Some((Token::Text("t".into()), Progress::Working(Some(34))))
    );
    assert_eq!(
        read(json!({"token": "t", "value": {"kind": "report", "percentage": 700}})),
        Some((Token::Text("t".into()), Progress::Working(Some(100))))
    );
    assert_eq!(
        read(json!({"token": "t", "value": {"kind": "end"}})),
        Some((Token::Text("t".into()), Progress::Ended))
    );
    assert_eq!(
        read(json!({"token": "t", "value": {"kind": "other"}})),
        None
    );
    assert_eq!(read(json!({"token": [1], "value": {"kind": "end"}})), None);
    assert_eq!(read(json!({"value": {"kind": "end"}})), None);
}

#[test]
fn messages_are_read_with_their_severity() {
    assert_eq!(
        message(&json!({"type": 1, "message": "bad"})),
        Some((Severity::Error, "bad"))
    );
    assert_eq!(
        message(&json!({"type": 2, "message": "w"})),
        Some((Severity::Warning, "w"))
    );
    assert_eq!(
        message(&json!({"type": 9, "message": "x"})),
        Some((Severity::Log, "x"))
    );
    assert_eq!(message(&json!({"type": 1})), None);
}

#[test]
fn settings_merge_key_by_key() {
    let mut options = json!({"cargo": {"targetDir": true, "features": "all"}, "a": 1});
    merge(
        &mut options,
        &json!({"cargo": {"targetDir": false}, "check": {"command": "clippy"}, "a": {"b": 2}}),
    );
    assert_eq!(
        options,
        json!({
            "cargo": {"targetDir": false, "features": "all"},
            "check": {"command": "clippy"},
            "a": {"b": 2},
        })
    );
    let mut scalar = json!(1);
    merge(&mut scalar, &json!({"x": 1}));
    assert_eq!(scalar, json!({"x": 1}));
}

/// Any JSON value: telling it apart never panics, and an answer is read only from an
/// object with an integer id. Run longer with `PROPTEST_CASES`.
fn any_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>().prop_map(|n| json!(n)),
        "[a-z/$]{0,8}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![
                    Just("id".to_owned()),
                    Just("method".to_owned()),
                    Just("result".to_owned()),
                    Just("error".to_owned()),
                    Just("params".to_owned()),
                    "[a-z]{1,4}",
                ],
                inner,
                0..6,
            )
            .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn fuzz(value in any_value()) {
        let had_method = value.get("method").is_some();
        let had_integer_id = value.get("id").and_then(Value::as_i64).is_some();
        match classify(value) {
            Ok(Incoming::Answer { .. }) => prop_assert!(had_integer_id && !had_method),
            Ok(Incoming::Request { .. } | Incoming::Notification { .. }
                | Incoming::Unanswerable { .. }) => prop_assert!(had_method),
            Ok(Incoming::Stray) => prop_assert!(!had_method && !had_integer_id),
            Err(NotRpc) => {}
        }
    }

    #[test]
    fn configuration_never_panics(value in any_value(), options in any_value()) {
        let _ = answer_for("workspace/configuration", &value, Some("s"), &options);
        let _ = progress(&value);
        let _ = message(&value);
        let _ = chosen_encoding(&value);
    }
}

#[test]
fn the_sync_a_server_asks_for_is_read() {
    let sync_of = |given: Value| sync(&json!({"capabilities": {"textDocumentSync": given}}));
    assert_eq!(
        sync_of(json!({"openClose": true, "change": 2, "save": {"includeText": false}})),
        Sync {
            open_close: true,
            change: SyncKind::Incremental,
            save: true
        }
    );
    assert_eq!(
        sync_of(json!({"openClose": true, "change": 1})),
        Sync {
            open_close: true,
            change: SyncKind::Full,
            save: false
        }
    );
    assert_eq!(
        sync_of(json!(2)),
        Sync {
            open_close: true,
            change: SyncKind::Incremental,
            save: false
        }
    );
    assert!(!sync_of(json!({"openClose": true, "change": 2, "save": {"includeText": true}})).save);
    assert_eq!(
        sync_of(json!(0)),
        Sync {
            open_close: false,
            change: SyncKind::None,
            save: false
        }
    );
    assert_eq!(
        sync(&json!({"capabilities": {}})),
        Sync {
            open_close: false,
            change: SyncKind::None,
            save: false
        }
    );
    assert_eq!(sync_of(json!({"change": 9})).change, SyncKind::None);
}
