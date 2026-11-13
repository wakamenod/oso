//! The review glue's choices that need no window: which scope a request names, what a review
//! is called, and which open review a request finds.
//!
//! Must not depend on: winit, the renderer or the review's threads.

use oso_core::editor::ScopeKind;

use super::{same_scope, scope_label, scope_of};
use crate::commands::review::{MAX_NAME, one_line};
use crate::review::{Id, Scope};

fn id(digit: u8) -> Id {
    Id::parse(&[digit; 40]).unwrap()
}

#[test]
fn a_request_names_its_scope_or_says_why_it_cannot() {
    assert_eq!(
        scope_of(ScopeKind::Uncommitted, None, None),
        Ok(Scope::Uncommitted)
    );
    assert_eq!(
        scope_of(ScopeKind::Session, None, None),
        Err("No baseline: B takes one")
    );
    assert_eq!(
        scope_of(ScopeKind::Session, None, Some(id(b'1'))),
        Ok(Scope::Session { baseline: id(b'1') })
    );
    assert_eq!(
        scope_of(ScopeKind::Branch, Some("main".into()), None),
        Ok(Scope::Branch {
            base: "main".into()
        })
    );
    assert_eq!(
        scope_of(ScopeKind::Range, Some("a..b".into()), None),
        Ok(Scope::Range {
            from: "a".into(),
            to: "b".into()
        })
    );
    assert_eq!(
        scope_of(ScopeKind::Range, Some("a".into()), None),
        Err("A range is FROM..TO")
    );
    assert_eq!(
        scope_of(ScopeKind::Commit, None, None),
        Err("No revision was named")
    );
    assert!(scope_of(ScopeKind::PullRequest, None, None).is_err());
}

#[test]
fn a_session_review_is_the_same_whatever_its_baseline_and_others_by_their_revision() {
    let first = Scope::Session { baseline: id(b'1') };
    let second = Scope::Session { baseline: id(b'2') };
    assert!(same_scope(&first, &second));
    assert!(same_scope(&Scope::Staged, &Scope::Staged));
    assert!(!same_scope(&Scope::Staged, &Scope::Unstaged));
    let main = Scope::Branch {
        base: "main".into(),
    };
    let dev = Scope::Branch { base: "dev".into() };
    assert!(!same_scope(&main, &dev));
}

#[test]
fn names_are_one_line_with_no_control_and_cut() {
    assert_eq!(one_line("a\nb\x1b[0m\r", 10), "a b [0m ");
    let long = "x".repeat(MAX_NAME + 5);
    let cut = scope_label(&Scope::Commit {
        revision: long.clone(),
    });
    assert_eq!(cut.chars().count(), MAX_NAME + 1);
    assert!(cut.ends_with('\u{2026}'));
    assert_eq!(
        scope_label(&Scope::Range {
            from: "a".into(),
            to: "b".into()
        }),
        "range a..b"
    );
}
