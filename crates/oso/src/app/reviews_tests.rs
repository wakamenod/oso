//! The review glue's choices that need no window: which scope a request names, what a review
//! is called, which open review a request finds, what reopening one does, which reports are
//! dropped, and what a project no longer trusted stops.
//!
//! Must not depend on: winit, the renderer or the review's threads.

use std::collections::VecDeque;
use std::path::PathBuf;

use oso_core::editor::{Editor, ReviewId, ScopeKind};
use oso_core::project::ProjectId;

use super::state::{Baseliner, Reopen, Reviews, Running};
use super::{same_scope, scope_label, scope_of};
use crate::commands::review::{MAX_NAME, one_line};
use crate::review::view::{self, View};
use crate::review::{Id, Scope};
use crate::review::{Reviewer, TrustedRoot};

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

/// A root no one reviews in: the threads below never run git, as they are asked nothing.
fn root() -> TrustedRoot {
    TrustedRoot::for_tests(PathBuf::from("/nowhere/oso-review-glue"))
}

fn reviewer(root: &TrustedRoot) -> Reviewer {
    Reviewer::start(root.clone(), None, |_| true).unwrap()
}

/// An open review of `project`, its thread's `serial`th start.
fn running(id: ReviewId, project: ProjectId, scope: Scope, serial: u64) -> Running {
    let root = root();
    Running {
        id,
        project,
        scope,
        reviewer: Some(reviewer(&root)),
        root,
        serial,
        view: View::start("t".into(), |_| true).unwrap(),
        texts: VecDeque::new(),
        revoked: false,
        carrying: VecDeque::new(),
        folded: false,
        shown_folded: false,
    }
}

/// An editor with a review open in a project, and the two.
fn editor_with_review() -> (Editor, ProjectId, ReviewId) {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (id, _) = editor.add_review(project, "*review*".into());
    (editor, project, id)
}

#[test]
fn a_report_of_a_thread_started_over_or_of_a_closed_review_is_dropped() {
    let (_editor, project, id) = editor_with_review();
    let mut reviews = Reviews::default();
    reviews.open.push(running(id, project, Scope::Staged, 3));
    assert_eq!(reviews.current(id, 3), Some(0));
    assert_eq!(reviews.current(id, 2), None);
    // Closing it hands back the text it left waiting, for the app to free.
    reviews.open[0]
        .texts
        .push_back(Box::new(view::built("t", Vec::new())));
    let closed = reviews.close(id).unwrap();
    assert_eq!(closed.texts.len(), 1);
    assert_eq!(reviews.current(id, 3), None);
    assert!(reviews.close(id).is_none());
}

#[test]
fn reopening_refreshes_opens_a_new_baseline_or_starts_a_revoked_review_over() {
    let (_editor, project, review) = editor_with_review();
    let one = Scope::Session { baseline: id(b'1') };
    let two = Scope::Session { baseline: id(b'2') };
    let mut open = running(review, project, one.clone(), 1);
    assert_eq!(open.reopen(&one), Reopen::Refresh);
    assert_eq!(open.reopen(&two), Reopen::Open);
    open.revoked = true;
    assert_eq!(open.reopen(&one), Reopen::Restart);
    assert_eq!(open.reopen(&two), Reopen::Restart);
}

#[test]
fn only_the_latest_baseline_thread_of_a_project_reports() {
    let (_editor, project, _) = editor_with_review();
    let mut reviews = Reviews::default();
    for serial in [1, 2] {
        let root = root();
        reviews.add_baseliner(Baseliner {
            project,
            serial,
            _reviewer: reviewer(&root),
            root,
        });
    }
    assert_eq!(reviews.baseliners.len(), 1);
    assert!(
        !reviews.take_baseliner(project, 1),
        "replaced by the second B"
    );
    assert!(reviews.take_baseliner(project, 2));
    assert!(!reviews.take_baseliner(project, 2), "done once");
}

#[test]
fn a_project_no_longer_trusted_stops_its_reviews_baselines_and_counts() {
    let (_editor, project, id) = editor_with_review();
    let mut reviews = Reviews::default();
    reviews
        .open
        .push(running(id, project, Scope::Uncommitted, 1));
    let baseline_root = root();
    reviews.add_baseliner(Baseliner {
        project,
        serial: 2,
        _reviewer: reviewer(&baseline_root),
        root: baseline_root.clone(),
    });
    let counts_root = root();
    reviews.counted = Some((project, counts_root.clone()));
    reviews.base_wanted = Some(project);

    reviews.revoke_untrusted(|_| true);
    assert!(!reviews.open[0].revoked);
    assert_eq!(reviews.baseliners.len(), 1);
    assert!(reviews.counted.is_some());

    reviews.revoke_untrusted(|_| false);
    assert!(reviews.open[0].revoked);
    assert!(reviews.open[0].root.revoked());
    assert!(reviews.baseliners.is_empty());
    assert!(baseline_root.revoked());
    // Its report, should one come, is not used.
    assert!(!reviews.take_baseliner(project, 2));
    assert!(reviews.counted.is_none());
    assert!(counts_root.revoked());
    assert_eq!(reviews.base_wanted, None);
}
