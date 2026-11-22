//! A pull request's commits on the editor (ADR 0167): the commits kept beside its review, the
//! link from a commit's review, the modeline, and the commit list, shown above the review's
//! area, laid out from the commits with its cursor on the review shown, and hidden, closed or
//! dropped as nothing shows it or a review of its pull request.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files.

use std::sync::Arc;

use super::strip_text;
use crate::editor::{CommitEntry, Commits, Editor, NoPair, ReviewId};
use crate::frame::{FrameSize, Way};
use crate::project::ProjectId;
use crate::window::WindowId;

fn entry(short: &str, subject: &str) -> CommitEntry {
    CommitEntry {
        short: short.to_owned(),
        subject: subject.to_owned(),
        author: "Ann".to_owned(),
        date: "2026-11-20".to_owned(),
        merge: false,
        empty: false,
        unpushed: false,
        viewed: 0,
        hunks: 0,
        unsent: 0,
    }
}

fn commits(shown: Option<usize>) -> Commits {
    let mut merge = entry("3333333", "Merge main");
    merge.merge = true;
    let mut last = entry("4444444", "Fix the pane");
    last.unpushed = true;
    last.viewed = 1;
    last.hunks = 3;
    last.unsent = 2;
    let mut first = entry("1111111", "Add the walk");
    first.viewed = 2;
    first.hunks = 2;
    Commits {
        number: 12,
        entries: Arc::from(vec![
            first,
            entry("2222222", "List the commits"),
            merge,
            last,
        ]),
        more: false,
        shown,
        open: Vec::new(),
    }
}

/// Links the review `commit` to `pull` as its commit `at`, as the host's commits name it.
fn link(editor: &mut Editor, pull: ReviewId, commit: ReviewId, at: usize) {
    let mut linked = editor.review_commits(pull).unwrap().clone();
    linked.open = vec![(at, commit)];
    editor.set_review_commits(pull, Some(linked));
}

/// An editor of `rows` showing a pull request's review, with its commits.
fn editor_with_pull(rows: usize) -> (Editor, ReviewId, WindowId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize { rows, columns: 120 });
    let (id, buffer) = editor.add_review(ProjectId(0), "*review: #12 p*".into());
    editor.show(buffer);
    editor.set_review_commits(id, Some(commits(None)));
    let window = editor.window().id();
    (editor, id, window)
}

/// The selected window's modeline.
fn modeline(editor: &Editor) -> String {
    let mut status = crate::editor::Status::default();
    editor.status_into(editor.window().id(), &mut status);
    status.modeline()
}

/// The text of the review `id`'s commit list.
fn shown_text(editor: &Editor, id: ReviewId) -> String {
    let buffer = editor.review_commits_pane(id).unwrap();
    editor.buffer(buffer).unwrap().text().to_string()
}

fn rows_of(editor: &Editor, window: WindowId) -> usize {
    editor.frame().regions(window).unwrap().outer().rows
}

fn line_of(editor: &Editor, window: WindowId) -> usize {
    let window = editor.window_by_id(window).unwrap();
    let text = editor.shown(window).text();
    text.offset_to_point(window.selections.primary().cursor).row
}

#[test]
fn the_text_is_the_whole_then_a_row_a_commit_with_its_marks() {
    let text = strip_text(&commits(Some(1)));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "  All changes \u{b7} #12 \u{b7} 4 commits");
    assert_eq!(
        lines[1],
        "  1/4 \u{2713}      1111111 Add the walk  \u{b7} 2026-11-20 \u{b7} Ann"
    );
    assert!(
        lines[2].starts_with("\u{25b6} 2/4"),
        "the one shown: {}",
        lines[2]
    );
    // What oso says of a commit comes before anyone's words: a subject cannot make a commit
    // read as a merge.
    assert!(
        lines[3].starts_with("  3/4 merge  3333333 Merge main  "),
        "{}",
        lines[3]
    );
    assert!(
        lines[4].starts_with("  4/4 \u{25d0}\u{25cf}2\u{2191}"),
        "some viewed, two unsent, not pushed: {}",
        lines[4]
    );
}

#[test]
fn a_long_subject_is_cut_and_more_commits_say_so() {
    let mut cut = commits(None);
    let mut entries = cut.entries.to_vec();
    entries[0].subject = "x".repeat(300);
    cut.entries = Arc::from(entries);
    cut.more = true;
    let text = strip_text(&cut);
    let first = text.lines().nth(1).unwrap();
    assert!(first.contains(&format!("{}\u{2026}", "x".repeat(120))));
    assert!(!first.contains(&"x".repeat(121)));
    assert!(text.ends_with("only the newest 4 commits are listed"));
}

#[test]
fn the_list_goes_above_the_review_with_its_cursor_on_the_review_shown() {
    let (mut editor, id, window) = editor_with_pull(30);
    let pane = editor.show_review_commits(id, window).unwrap();
    assert_ne!(pane, window);
    assert_eq!(editor.window().id(), window, "the review stays selected");
    assert_eq!(rows_of(&editor, pane), 6, "five rows and the modeline");
    let order: Vec<WindowId> = editor.frame().windows().iter().map(|(id, _)| *id).collect();
    assert_eq!(order, [pane, window]);
    let buffer = editor.review_commits_pane(id).unwrap();
    let shown = editor.buffer(buffer).unwrap();
    assert_eq!(shown.name(), "*review: #12 p \u{b7} commits*");
    assert_eq!(shown.mode(), crate::buffer::Mode::ReviewCommits);
    assert!(shown.is_internal() && shown.read_only().is_some());
    assert_eq!(shown.text().to_string(), strip_text(&commits(None)));
    assert_eq!(line_of(&editor, pane), 0, "the whole pull request");
    editor.set_review_commits(id, Some(commits(Some(3))));
    assert_eq!(line_of(&editor, pane), 4, "its fourth commit");
    assert_eq!(
        shown_text(&editor, id),
        strip_text(&commits(Some(3))),
        "rows replaced"
    );
    // A mark counted again lays its row out again, and moves no cursor.
    editor.select_window(pane);
    let offset = editor.current().1.text().line_start(2);
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(offset);
    let mut marked = commits(Some(3));
    let mut entries = marked.entries.to_vec();
    entries[0].unsent = 5;
    marked.entries = Arc::from(entries);
    editor.set_review_commits(id, Some(marked.clone()));
    assert_eq!(line_of(&editor, pane), 2, "the person's row");
    assert_eq!(shown_text(&editor, id), strip_text(&marked));
    // Another review shown moves it.
    marked.shown = Some(0);
    editor.set_review_commits(id, Some(marked.clone()));
    assert_eq!(line_of(&editor, pane), 1);
    marked.shown = None;
    editor.set_review_commits(id, Some(marked.clone()));
    assert_eq!(line_of(&editor, pane), 0);
    assert_eq!(shown_text(&editor, id), strip_text(&marked));
    // Shown again, the same window.
    assert_eq!(editor.show_review_commits(id, window), Ok(pane));
}

#[test]
fn many_commits_take_nine_rows_and_a_short_frame_none() {
    let (mut editor, id, window) = editor_with_pull(40);
    let mut many = commits(None);
    many.entries = Arc::from(vec![entry("1234567", "x"); 30]);
    editor.set_review_commits(id, Some(many));
    let pane = editor.show_review_commits(id, window).unwrap();
    assert_eq!(rows_of(&editor, pane), 10);
    let (mut editor, id, window) = editor_with_pull(5);
    assert_eq!(
        editor.show_review_commits(id, window),
        Err(NoPair::TooSmall)
    );
}

#[test]
fn a_commits_review_is_linked_and_shows_the_list_and_its_place() {
    let (mut editor, pull, window) = editor_with_pull(30);
    let (commit, buffer) = editor.add_review(ProjectId(0), "*review: commit 2222222 p*".into());
    link(&mut editor, pull, commit, 1);
    assert_eq!(editor.review_pull(commit), Some(pull));
    assert_eq!(editor.review_pull(pull), None);
    editor.show(buffer);
    assert!(
        modeline(&editor).contains("#12 2/4"),
        "{}",
        modeline(&editor)
    );
    let pane = editor.show_review_commits(pull, window).unwrap();
    // The list stays while the commit's review shows; the pull request's own shows too.
    editor.follow_cursor();
    assert!(editor.window_by_id(pane).is_some());
    let review = editor.review_buffer(pull).unwrap();
    editor.show(review);
    editor.follow_cursor();
    assert!(editor.window_by_id(pane).is_some());
    assert!(
        modeline(&editor).contains("#12 all 4 commits"),
        "{}",
        modeline(&editor)
    );
    // A review of no pull request takes no list.
    let (other, other_buffer) = editor.add_review(ProjectId(0), "*review: uncommitted p*".into());
    editor.show(other_buffer);
    assert_eq!(
        editor.show_review_commits(other, window),
        Err(NoPair::NoBuffer)
    );
    editor.follow_cursor();
    assert!(
        editor.window_by_id(pane).is_none(),
        "no review of the pull request shows"
    );
    assert_eq!(rows_of(&editor, window), 29, "its rows given back");
}

#[test]
fn closing_the_pull_request_unlinks_its_commits_and_takes_the_list_along() {
    let (mut editor, pull, window) = editor_with_pull(30);
    let (commit, _) = editor.add_review(ProjectId(0), "*review: commit 2222222 p*".into());
    link(&mut editor, pull, commit, 1);
    // Commits that no longer name it unlink it.
    editor.set_review_commits(pull, Some(commits(None)));
    assert_eq!(editor.review_pull(commit), None);
    link(&mut editor, pull, commit, 1);
    let pane = editor.show_review_commits(pull, window).unwrap();
    editor.remove_review(pull);
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(editor.frame().windows().len(), 1);
    assert_eq!(editor.review_pull(commit), None);
}

#[test]
fn hiding_gives_the_rows_back_and_no_commits_hide_the_list() {
    let (mut editor, id, window) = editor_with_pull(30);
    let pane = editor.show_review_commits(id, window).unwrap();
    assert!(editor.hide_review_commits(id));
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(rows_of(&editor, window), 29);
    assert!(!editor.hide_review_commits(id), "nothing to hide");
    let pane = editor.show_review_commits(id, window).unwrap();
    editor.set_review_commits(id, None);
    assert!(editor.window_by_id(pane).is_none());
    assert_eq!(editor.review_commits_pane(id), None);
    assert_eq!(editor.review_commits(id), None);
}

#[test]
fn above_one_of_two_windows_side_by_side_it_takes_that_ones_columns() {
    let (mut editor, id, window) = editor_with_pull(30);
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 200,
    });
    let other = editor.split_window(Way::Across, false).unwrap();
    let pane = editor.show_review_commits(id, other).unwrap();
    let order: Vec<WindowId> = editor.frame().windows().iter().map(|(id, _)| *id).collect();
    assert_eq!(
        order.first(),
        Some(&window),
        "only the window it was asked above"
    );
    assert!(order.contains(&pane));
}
