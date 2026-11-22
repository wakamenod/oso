//! The reviews a restart opens again (ADR 0122, step 6, Q-H): each review a window showed, by
//! its project's root and its scope, with the windows that showed it, by their place among the
//! state's windows, and the review layout's slot while the frame is its layout (ADR 0160), with
//! the window below it that `review-ask` showed a session in (ADR 0169). What a review held is
//! not in the state: the new instance reads it from the saved review the old one wrote before
//! it went.
//!
//! Must not depend on: `app`; the state's writer finds which windows show each review.
//!
//! An entry that is not one this writes is left out, never refusing the restart: the window
//! then shows what it showed behind the review, as before step 6. Past [`MAX_REVIEWS`], or
//! when they would not fit in the state, the reviews are left out too. A scope is written as a
//! saved review writes it (`review::saved`), so that a change of that format changes this
//! one: the reader takes both.

use std::path::PathBuf;

use oso_core::editor::{Editor, ReviewId};
use oso_core::window::{Window, WindowId};
use serde_json::{Map, Value, json};

use crate::review::Scope;
use crate::review::saved::{scope_from_value, scope_value};

/// The most reviews a state opens again.
pub const MAX_REVIEWS: usize = 64;

/// An open review the state may name: its project's root, its scope, and the review, whose
/// windows the state finds.
#[derive(Debug, Clone)]
pub struct Shown {
    pub root: PathBuf,
    pub scope: Scope,
    pub review: ReviewId,
    /// The review layout's slot, while the frame is the layout of this review.
    pub slot: Option<WindowId>,
    /// The window below the slot that shows `review-ask`'s session (ADR 0169).
    pub ask: Option<WindowId>,
}

/// The entries of `shown` that `tiled`, the state's windows in its order, show, at most
/// [`MAX_REVIEWS`].
pub(super) fn entries(editor: &Editor, tiled: &[&Window], shown: &[Shown]) -> Vec<ReviewEntry> {
    shown
        .iter()
        .filter_map(|review| {
            let shows = |at: &usize, pane: bool| {
                let buffer = tiled[*at].buffer();
                if pane {
                    editor.review_files(review.review) == Some(buffer)
                } else {
                    editor.review_of(buffer) == Some(review.review)
                }
            };
            // The review's own windows first, which it opens in; its files pane's after, which
            // close and come back as the review saved them (step 6b).
            let windows: Vec<usize> = (0..tiled.len())
                .filter(|at| shows(at, false))
                .chain((0..tiled.len()).filter(|at| shows(at, true)))
                .collect();
            if !windows.first().is_some_and(|at| shows(at, false)) {
                return None;
            }
            let at = |id: Option<WindowId>| {
                id.and_then(|id| tiled.iter().position(|window| window.id() == id))
                    .filter(|at| !windows.contains(at))
            };
            let slot = at(review.slot);
            // Only with the slot, and never the slot itself.
            let ask = slot.and(at(review.ask)).filter(|ask| Some(*ask) != slot);
            Some(ReviewEntry {
                root: review.root.clone(),
                scope: review.scope.clone(),
                windows,
                slot,
                ask,
            })
        })
        .take(MAX_REVIEWS)
        .collect()
}

/// A review to open again: its project's root, its scope, and the windows that showed it, the
/// first to show it again in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewEntry {
    pub root: PathBuf,
    pub scope: Scope,
    pub windows: Vec<usize>,
    /// The review layout's slot among the state's windows, a reader before ADR 0160 leaving it
    /// out.
    pub slot: Option<usize>,
    /// The window below the slot that showed `review-ask`'s session, among the state's
    /// windows, with a slot only; a reader before ADR 0169 leaves it out.
    pub ask: Option<usize>,
}

/// `entries` as the state keeps them.
pub fn state(entries: &[ReviewEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .filter_map(|entry| {
                let mut value = json!({
                    "root": entry.root.to_str()?,
                    "scope": scope_value(&entry.scope),
                    "windows": entry.windows,
                });
                if let (Some(slot), Some(map)) = (entry.slot, value.as_object_mut()) {
                    map.insert("slot".into(), json!(slot));
                    if let Some(ask) = entry.ask {
                        map.insert("ask".into(), json!(ask));
                    }
                }
                Some(value)
            })
            .collect(),
    )
}

/// The reviews `state` opens again, those of `windows` windows: entries this does not write,
/// or that name no window, are left out.
pub(super) fn parse(state: &Map<String, Value>, windows: usize) -> Vec<ReviewEntry> {
    let Some(entries) = state.get("reviews").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .take(MAX_REVIEWS)
        .filter_map(|entry| {
            let root = PathBuf::from(entry.get("root")?.as_str()?);
            let shown: Vec<usize> = (entry.get("windows")?.as_array()?.iter())
                .map(|at| {
                    usize::try_from(at.as_u64()?)
                        .ok()
                        .filter(|at| *at < windows)
                })
                .collect::<Option<_>>()?;
            // A slot that names no window, or one of the review's, is left out.
            let at = |key: &str| {
                (entry.get(key).and_then(Value::as_u64))
                    .and_then(|at| usize::try_from(at).ok())
                    .filter(|at| *at < windows && !shown.contains(at))
            };
            let slot = at("slot");
            // The window below the slot, only with a slot, and never the slot itself.
            let ask = slot.and(at("ask")).filter(|ask| Some(*ask) != slot);
            (root.is_absolute() && !shown.is_empty()).then_some(ReviewEntry {
                root,
                scope: scope_from_value(entry.get("scope")?)?,
                windows: shown,
                slot,
                ask,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// An absolute root on this system: a Unix path is not one on Windows.
    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work\p")
        } else {
            PathBuf::from("/work/p")
        }
    }

    #[test]
    fn reviews_go_both_ways_and_what_this_does_not_write_is_left_out() {
        let entry = ReviewEntry {
            root: root(),
            scope: Scope::Branch {
                base: "main".into(),
            },
            windows: vec![0, 2],
            slot: Some(1),
            ask: Some(3),
        };
        let state = json!({ "reviews": state(std::slice::from_ref(&entry)) });
        assert_eq!(
            parse(state.as_object().unwrap(), 4),
            std::slice::from_ref(&entry)
        );
        assert!(
            parse(state.as_object().unwrap(), 2).is_empty(),
            "no window 2"
        );
        assert_eq!(
            parse(state.as_object().unwrap(), 3)[0].ask,
            None,
            "no window 3, the rest kept"
        );
        let odd = json!({ "reviews": [
            { "root": "relative", "scope": { "kind": "staged" }, "windows": [0] },
            { "root": root(), "scope": { "kind": "what" }, "windows": [0] },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [] },
            "no",
        ] });
        assert!(parse(odd.as_object().unwrap(), 3).is_empty());
        // A slot that names no window, or one of the review's, is left out, the rest kept; a
        // state with none, as before the layout, has none.
        let slots = json!({ "reviews": [
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 7 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 0 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": "1" },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0] },
        ] });
        let parsed = parse(slots.as_object().unwrap(), 3);
        assert_eq!(parsed.len(), 4);
        assert!(parsed.iter().all(|entry| entry.slot.is_none()));
        // The window below the slot: none without a slot, as the slot, or one of the review's;
        // a state without it, as before ADR 0169, has none.
        let asks = json!({ "reviews": [
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "ask": 2 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 1, "ask": 1 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 1, "ask": 0 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 1 },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [0], "slot": 1, "ask": 2 },
        ] });
        let asks: Vec<_> = (parse(asks.as_object().unwrap(), 3).iter())
            .map(|entry| entry.ask)
            .collect();
        assert_eq!(asks, [None, None, None, None, Some(2)]);
        assert!(parse(json!({}).as_object().unwrap(), 3).is_empty());
    }

    #[test]
    fn the_layout_s_slot_and_the_window_below_it_are_named_by_their_place() {
        use oso_core::frame::Way;
        let mut editor = Editor::with_buffer(oso_core::buffer::Buffer::from_text("a.rs", "a"));
        editor.set_frame_size(oso_core::frame::FrameSize {
            rows: 40,
            columns: 120,
        });
        let project = editor.add_project(oso_core::project::Found {
            root: oso_core::project::RootIdentity {
                path: root(),
                identity: None,
            },
            worktree_of: None,
            checked_main: None,
        });
        let (review, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
        let left = editor.window().id();
        let slot = editor.split_window(Way::Across, true).unwrap();
        let below = editor.split_window(Way::Down, false).unwrap();
        editor.show_in(left, buffer);
        let tiled = super::super::tiled_windows(&editor);
        let shown = |slot, ask| Shown {
            root: root(),
            scope: Scope::Staged,
            review,
            slot,
            ask,
        };
        let entry = |slot, ask| entries(&editor, &tiled, &[shown(slot, ask)])[0].clone();
        let at = |id: WindowId| tiled.iter().position(|window| window.id() == id);
        let named = entry(Some(slot), Some(below));
        assert_eq!((named.slot, named.ask), (at(slot), at(below)));
        // No window below without a slot, and never the slot or the review's own window.
        assert_eq!(entry(None, Some(below)).ask, None);
        assert_eq!(entry(Some(slot), Some(slot)).ask, None);
        assert_eq!(entry(Some(slot), Some(left)).ask, None);
    }
}
