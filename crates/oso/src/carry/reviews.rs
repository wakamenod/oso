//! The reviews a restart opens again (ADR 0122, step 6, Q-H): each review a window showed, by
//! its project's root and its scope, with the windows that showed it, by their place among the
//! state's windows. What a review held is not in the state: the new instance reads it from
//! the saved review the old one wrote before it went.
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
use oso_core::window::Window;
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
            Some(ReviewEntry {
                root: review.root.clone(),
                scope: review.scope.clone(),
                windows,
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
}

/// `entries` as the state keeps them.
pub fn state(entries: &[ReviewEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .filter_map(|entry| {
                Some(json!({
                    "root": entry.root.to_str()?,
                    "scope": scope_value(&entry.scope),
                    "windows": entry.windows,
                }))
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
            (root.is_absolute() && !shown.is_empty()).then_some(ReviewEntry {
                root,
                scope: scope_from_value(entry.get("scope")?)?,
                windows: shown,
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
        };
        let state = json!({ "reviews": state(std::slice::from_ref(&entry)) });
        assert_eq!(
            parse(state.as_object().unwrap(), 3),
            std::slice::from_ref(&entry)
        );
        assert!(
            parse(state.as_object().unwrap(), 2).is_empty(),
            "no window 2"
        );
        let odd = json!({ "reviews": [
            { "root": "relative", "scope": { "kind": "staged" }, "windows": [0] },
            { "root": root(), "scope": { "kind": "what" }, "windows": [0] },
            { "root": root(), "scope": { "kind": "staged" }, "windows": [] },
            "no",
        ] });
        assert!(parse(odd.as_object().unwrap(), 3).is_empty());
        assert!(parse(json!({}).as_object().unwrap(), 3).is_empty());
    }
}
