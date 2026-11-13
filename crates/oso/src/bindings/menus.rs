//! The built-in menus (ADR 0117): defaults that `config.toml` adds to, replaces whole by name, or
//! removes. They come first in every set, so their ids, which default keys name, never change.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data.
//!
//! - `review` (ADR 0122), on `C-x v r`: a review of each scope, and the session's baseline, with
//!   the counts its describe function says.

use oso_core::command::Registry;
use oso_core::key::KeySequence;
use oso_core::menu::{Definition, Head, HeadAction, Menus, Stay};

/// A head of a built-in menu: its keys, label, command and whether the menu stays after it.
type HeadRow = (&'static str, &'static str, &'static str, bool);

/// The review menu's groups: a title, and its heads.
const REVIEW: &[(&str, &[HeadRow])] = &[
    (
        "Review",
        &[
            ("D", "session changes", "review-session", false),
            ("w", "uncommitted", "review-uncommitted", false),
            ("u", "unstaged", "review-unstaged", false),
            ("s", "staged", "review-staged", false),
            ("b", "branch against base", "review-branch", false),
        ],
    ),
    (
        "Revisions",
        &[
            ("c", "a commit\u{2026}", "review-commit", false),
            ("r", "a range\u{2026}", "review-range", false),
        ],
    ),
    (
        "Session",
        &[("B", "take a baseline", "review-mark-baseline", true)],
    ),
];

/// The review menu over the commands in `registry`, or none without them, as in tests that
/// bring their own commands.
fn review(registry: &Registry) -> Option<Definition> {
    let mut menu = Definition::new("review", "Review").ok()?;
    menu.set_describe(crate::commands::review::review_menu);
    for (title, heads) in REVIEW {
        menu.add_group(Some(title), 1).ok()?;
        for &(keys, label, command, stays) in *heads {
            let keys: KeySequence = keys.parse().ok()?;
            menu.add_head(Head {
                keys,
                label: label.to_owned(),
                action: HeadAction::Command(registry.lookup(command)?),
                stay: stays.then_some(Stay::Stay),
                count: None,
            })
            .ok()?;
        }
    }
    Some(menu)
}

/// The built-in menus over the commands in `registry`.
pub fn menus(registry: &Registry) -> Menus {
    let mut menus = Menus::new();
    if let Some(review) = review(registry)
        && let Ok(id) = menus.id("review")
    {
        // A menu built from this module's own table always fits.
        let _ = menus.define(id, review);
    }
    menus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_review_menu_is_built_from_the_default_commands() {
        let registry = crate::commands::registry();
        let menus = menus(&registry);
        let id = menus.lookup("review").expect("the review menu");
        let menu = menus.get(id).expect("defined");
        assert_eq!(menu.heads().len(), 8);
        assert!(menu.describe().is_some());
    }
}
