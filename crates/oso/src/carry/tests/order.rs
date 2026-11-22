//! The tests of the buffers' order a state hands over (ADR 0171): the ids of its buffers, the
//! most recent first, read back checked, and none in a state from before it.
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use std::path::PathBuf;

use oso_core::buffer::Buffer;
use serde_json::{Value, json};

use super::super::{MAX_BUFFERS, export_value as export, parse};
use super::{absolute, visiting};

#[test]
fn the_buffers_order_is_handed_over_and_a_state_without_it_keeps_theirs() {
    let mut editor = visiting(&absolute("/p/a.txt"), "abc\n");
    let b = editor.add_buffer(Buffer::visiting(PathBuf::from(absolute("/p/b.txt"))));
    editor.show(b);
    // b, a.txt, then `*scratch*`; `*Messages*` is not handed over.
    let exported = export(&editor, &[], None, None).unwrap();
    let ids = |state: &Value| -> Vec<u64> {
        state["buffers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_u64().unwrap())
            .collect()
    };
    let carried = ids(&exported);
    let order: Vec<u64> = exported["order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect();
    assert_eq!(order, [b.get(), carried[1], carried[0]]);
    assert_eq!(parse(&exported).unwrap().order, [2, 1, 0]);

    let mut odd = exported.clone();
    odd["order"] = json!([carried[1], 999, carried[1]]);
    assert_eq!(
        parse(&odd).unwrap().order,
        [1],
        "unknown ids and repeats are passed over"
    );
    let mut old = exported.clone();
    old.as_object_mut().unwrap().remove("order");
    assert!(parse(&old).unwrap().order.is_empty());
    for bad in [
        json!("b"),
        json!(["x"]),
        json!(vec![carried[0]; carried.len() + 1]),
        json!(vec![0; MAX_BUFFERS + 1]),
    ] {
        let mut state = exported.clone();
        state["order"] = bad;
        let message = parse(&state).unwrap_err();
        assert!(message.contains("order"), "{message}");
    }
}
