//! Stepping through a pull request's commits on an editor (ADR 0167): `M-n` and `M-p` ask the
//! host for a step from a pull request's review, one of its commits' and its commit list, and
//! are refused in a review of no pull request; `l` shows the list and selects it, and closes it
//! from it; `RET` and `SPC` there ask for the row's review; `?` lists the list's keys.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use std::sync::Arc;

use oso_core::command::Args;
use oso_core::editor::{CommitEntry, Commits, Editor, ReviewId, ReviewRequest, Step};
use oso_core::selection::SelectionSet;

use super::super::tests::{in_project, with_review};
use crate::commands::tests::run;

fn entry(short: &str) -> CommitEntry {
    CommitEntry {
        short: short.into(),
        subject: "s".into(),
        author: "Ann".into(),
        date: "2026-11-20".into(),
        merge: false,
        empty: false,
        unpushed: false,
        viewed: 0,
        hunks: 0,
        unsent: 0,
    }
}

/// An editor showing a pull request's review, with two commits.
fn with_pull() -> (Editor, ReviewId) {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    let commits = Commits {
        number: 12,
        entries: Arc::from(vec![entry("aaaaaaa"), entry("bbbbbbb")]),
        more: false,
        shown: None,
        open: Vec::new(),
    };
    editor.set_review_commits(review, Some(commits));
    (editor, review)
}

fn steps(editor: &mut Editor) -> Vec<(ReviewId, Step)> {
    (editor.take_review_requests().into_iter())
        .filter_map(|request| match request {
            ReviewRequest::Step { review, to } => Some((review, to)),
            _ => None,
        })
        .collect()
}

#[test]
fn m_n_and_m_p_ask_for_a_step_from_the_review() {
    let (mut editor, review) = with_pull();
    assert_eq!(run(&mut editor, "review-next-commit", Args::NONE).0, Ok(()));
    assert_eq!(
        run(&mut editor, "review-previous-commit", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        run(&mut editor, "review-pull-request-whole", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        steps(&mut editor),
        [
            (review, Step::Next),
            (review, Step::Previous),
            (review, Step::Whole)
        ]
    );
    // In no review, nothing is asked.
    let other = editor.add_buffer(oso_core::buffer::Buffer::from_text("notes", "x"));
    editor.show(other);
    assert!(
        run(&mut editor, "review-next-commit", Args::NONE)
            .0
            .is_err()
    );
    assert!(steps(&mut editor).is_empty());
}

#[test]
fn l_shows_the_list_above_and_selects_it_and_closes_it_from_it() {
    let (mut editor, review) = with_pull();
    let window = editor.window().id();
    assert_eq!(run(&mut editor, "review-commits", Args::NONE).0, Ok(()));
    let list = editor.review_commits_pane(review).unwrap();
    assert_eq!(editor.window().buffer(), list);
    assert_ne!(editor.window().id(), window);
    // From the list, a step asks from it, which the host sees selected.
    assert_eq!(run(&mut editor, "review-next-commit", Args::NONE).0, Ok(()));
    assert_eq!(steps(&mut editor), [(review, Step::Next)]);
    assert_eq!(editor.window().buffer(), list);
    assert_eq!(run(&mut editor, "review-commits", Args::NONE).0, Ok(()));
    assert_eq!(editor.review_commits_pane(review), None);
    assert!(editor.window_by_id(window).is_some());
}

#[test]
fn a_review_of_no_pull_request_has_no_list() {
    let (mut editor, project) = in_project(true);
    let _ = with_review(&mut editor, project);
    let refused = run(&mut editor, "review-commits", Args::NONE).0;
    assert!(refused.unwrap_err().contains("no pull request"));
}

#[test]
fn ret_and_spc_in_the_list_ask_for_its_rows_review() {
    let (mut editor, review) = with_pull();
    assert_eq!(run(&mut editor, "review-commits", Args::NONE).0, Ok(()));
    let go = |editor: &mut Editor, line: usize| {
        let offset = editor.current().1.text().line_start(line);
        editor.current_mut().0.selections = SelectionSet::new(offset);
    };
    let list = editor.window().id();
    go(&mut editor, 0);
    assert_eq!(
        run(&mut editor, "review-commits-display", Args::NONE).0,
        Ok(())
    );
    assert_eq!(editor.window().id(), list, "SPC stays");
    go(&mut editor, 2);
    assert_eq!(
        run(&mut editor, "review-commits-visit", Args::NONE).0,
        Ok(())
    );
    assert_ne!(
        editor.window().id(),
        list,
        "RET selects the review below first"
    );
    assert_eq!(
        steps(&mut editor),
        [(review, Step::Whole), (review, Step::Commit(1))]
    );
    editor.select_window(list);
    // `g` there reads the pull request again.
    assert_eq!(run(&mut editor, "review-refresh", Args::NONE).0, Ok(()));
    assert!(
        editor
            .take_review_requests()
            .contains(&ReviewRequest::Refresh(review))
    );
}

#[test]
fn question_mark_lists_the_lists_keys() {
    let (mut editor, _) = with_pull();
    assert_eq!(run(&mut editor, "review-commits", Args::NONE).0, Ok(()));
    assert_eq!(
        run(&mut editor, "review-commits-keys", Args::NONE).0,
        Ok(())
    );
    let text = editor.current().1.text().to_string();
    assert!(text.starts_with("Keys in a pull request's commit list"));
    assert!(text.contains("M-n     review-next-commit"), "{text}");
}
