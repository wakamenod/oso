//! The tests of how a state's layout is read (ADR 0112): a tree of runs and windows in the
//! tree's order, each child with a weight, refused when it does not hold the state's windows.
//!
//! Must not depend on: a window or the file system; a state is a plain value.

use serde_json::{Value, json};

use super::super::parse;
use super::{Mutation, with};

#[test]
fn bad_layouts_are_refused() {
    let bad: [(&str, Mutation); 7] = [
        ("layout is not", Box::new(|s| s["layout"] = Value::Null)),
        (
            "tree's order",
            Box::new(|s| s["layout"] = json!({ "window": 1 })),
        ),
        (
            "does not hold",
            Box::new(|s| {
                s["layout"] = json!({ "down": [
                    { "weight": 1, "window": 0 },
                    { "weight": 1, "window": 1 },
                ] });
            }),
        ),
        (
            "fewer than two",
            Box::new(|s| s["layout"] = json!({ "across": [{ "weight": 1, "window": 0 }] })),
        ),
        (
            "no weight",
            Box::new(|s| {
                s["layout"] = json!({ "across": [
                    { "weight": 0, "window": 0 },
                    { "weight": 1, "window": 1 },
                ] });
            }),
        ),
        (
            "neither across nor down",
            Box::new(|s| s["layout"] = json!({ "sideways": [] })),
        ),
        (
            "deeper",
            Box::new(|s| {
                let mut layout = json!({ "window": 0 });
                for level in 0..17 {
                    let way = if level % 2 == 0 { "down" } else { "across" };
                    layout = json!({ way: [
                        { "weight": 1, "run": layout },
                        { "weight": 1, "window": 0 },
                    ] });
                }
                s["layout"] = layout;
            }),
        ),
    ];
    for (expected, mutate) in bad {
        let state = with(|state| mutate(state));
        let message = parse(&state).unwrap_err();
        assert!(message.contains(expected), "{expected}: {message}");
    }
}
