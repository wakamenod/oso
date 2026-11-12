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

#[test]
fn a_run_written_inline_is_refused_so_none_nests_past_the_limit() {
    let state = with(|s| {
        s["layout"] = json!({ "across": [
            { "weight": 1, "window": 0 },
            { "weight": 1, "down": [
                { "weight": 1, "window": 1 },
                { "weight": 1, "window": 2 },
            ] },
        ] });
    });
    let message = parse(&state).unwrap_err();
    assert!(message.contains("neither a window nor a run"), "{message}");
}

#[test]
fn a_layout_too_heavy_to_take_is_refused_by_the_editor_and_nothing_panics() {
    let state = with(|s| {
        let window = s["windows"][0].clone();
        s["windows"] = json!([window, window, window]);
        s["layout"] = json!({ "across": [
            { "weight": 1, "window": 0 },
            { "weight": 1, "run": { "across": [
                { "weight": u64::MAX, "window": 1 },
                { "weight": 1, "window": 2 },
            ] } },
        ] });
    });
    let parsed = parse(&state).unwrap();
    let mut editor = oso_core::editor::Editor::default();
    assert_eq!(
        editor.set_tiling(&parsed.windows.layout),
        Err(oso_core::frame::BadTiling::TooHeavy)
    );
    assert_eq!(editor.frame().windows().len(), 1, "one window, as before");
}

#[test]
fn a_layout_of_mixed_shapes_is_refused() {
    let window_and_run = with(|s| {
        s["layout"] = json!({ "window": 0, "across": [] });
    });
    let message = parse(&window_and_run).unwrap_err();
    assert!(message.contains("says more than its number"), "{message}");
    let both_ways = with(|s| {
        let window = s["windows"][0].clone();
        s["windows"] = json!([window, window]);
        s["layout"] = json!({
            "across": [{ "weight": 1, "window": 0 }, { "weight": 1, "window": 1 }],
            "down": [],
        });
    });
    let message = parse(&both_ways).unwrap_err();
    assert!(message.contains("neither across nor down"), "{message}");
    let child_and_run = with(|s| {
        let window = s["windows"][0].clone();
        s["windows"] = json!([window, window]);
        s["layout"] = json!({ "across": [
            { "weight": 1, "window": 0, "run": { "down": [] } },
            { "weight": 1, "window": 1 },
        ] });
    });
    let message = parse(&child_and_run).unwrap_err();
    assert!(message.contains("neither a window nor a run"), "{message}");
}
