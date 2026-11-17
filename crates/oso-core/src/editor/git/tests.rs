//! Tests of git status buffers in the editor (ADR 0127).
//!
//! Must not depend on: the host's threads, files or processes.

use super::{GitHeading, GitLines, GitRequest, GitShown};
use crate::buffer::Mode;
use crate::editor::Editor;
use crate::load::decode_all;
use crate::project::ProjectId;
use crate::secret::Verdict;
use crate::syntax::{Highlight, Highlights};

const TEXT: &str = "Head:     main\n\nUnstaged changes (1)\nM  src/a.rs  +1 -0\n";

fn shown(line: usize, lines: Vec<(crate::window::WindowId, usize)>) -> GitShown {
    let mut highlights = Highlights::new(TEXT.len());
    highlights.replace(10..14, [(4, Some(Highlight::GitBranchLocal))]);
    GitShown {
        highlights,
        outline: vec![
            GitHeading { line: 2, depth: 0 },
            GitHeading { line: 3, depth: 1 },
        ],
        line,
        lines,
    }
}

fn text() -> crate::load::LoadSnapshot {
    decode_all(TEXT.as_bytes(), Default::default())
}

#[test]
fn a_status_goes_in_with_point_on_its_line() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    editor.show(id);
    let given = editor.set_git_text(id, text(), shown(3, Vec::new()));
    assert!(given.is_none());
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.text().to_string(), TEXT);
    assert_eq!(buffer.mode(), Mode::GitStatus);
    assert_eq!(buffer.name(), "*git: oso*");
    assert_eq!(buffer.verdict(), Verdict::Ordinary);
    assert!(buffer.read_only().is_some());
    assert!(!buffer.wraps());
    let highlights = buffer.highlights().expect("highlights");
    assert_eq!(highlights.at(11), Some(Highlight::GitBranchLocal));
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.find("M  ").unwrap());
    assert_eq!(editor.git_points(id), vec![(editor.window().id(), 3)]);
    assert_eq!(
        editor.git_outline(id),
        [
            GitHeading { line: 2, depth: 0 },
            GitHeading { line: 3, depth: 1 }
        ]
    );
    assert_eq!(editor.git_status_of(id), Some(ProjectId(0)));
    assert_eq!(editor.git_status_buffer(ProjectId(0)), Some(id));
    assert_eq!(editor.git_status_buffer(ProjectId(1)), None);
}

#[test]
fn a_window_keeps_the_line_the_host_found() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    editor.show(id);
    let window = editor.window().id();
    editor.set_git_text(id, text(), shown(3, vec![(window, 2)]));
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.find("Unstaged").unwrap());
    // A line past the end goes to the last line.
    editor.set_git_text(id, text(), shown(99, Vec::new()));
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.len());
}

#[test]
fn highlights_that_do_not_cover_the_text_come_back() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    let mut wrong = shown(0, Vec::new());
    wrong.highlights = Highlights::new(3);
    let given = editor.set_git_text(id, text(), wrong);
    assert_eq!(given.map(|given| given.len()), Some(3));
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), TEXT);
}

#[test]
fn another_buffer_takes_no_status_text() {
    let mut editor = Editor::default();
    let other = editor.window().buffer();
    let given = editor.set_git_text(other, text(), shown(0, Vec::new()));
    assert!(given.is_some());
    assert!(editor.git_outline(other).is_empty());
    assert_eq!(editor.git_status_of(other), None);
}

#[test]
fn a_person_goes_to_a_status_and_kills_it() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    editor.set_git_text(id, text(), shown(0, Vec::new()));
    assert!(editor.buffers_by_recency().contains(&id));
    assert_eq!(editor.buffer_named("*git: oso*"), Some(id));
    editor.show(id);
    editor.kill_by_person(id, false).unwrap();
    assert!(editor.buffer(id).is_none());
    assert_eq!(editor.git_status_of(id), None);
    assert!(editor.git_statuses().next().is_none());
    assert!(editor.git_outline(id).is_empty());
}

#[test]
fn a_person_lists_and_kills_a_pull_request_s_buffer_and_a_list() {
    let mut editor = Editor::default();
    let pr = editor.add_git_pr(ProjectId(0), "*pr: #12*");
    let other = editor.add_git_pr(ProjectId(0), "*pr: #12*");
    let list = editor.add_git_pr_list(ProjectId(1), "*pull requests: oso*");
    assert_eq!(editor.buffer_named("*pr: #12*<2>"), Some(other));
    assert_eq!(editor.buffer(pr).map(|b| b.mode()), Some(Mode::GitPr));
    assert_eq!(editor.buffer(list).map(|b| b.mode()), Some(Mode::GitPrList));
    assert_eq!(editor.git_pr_of(list), Some(ProjectId(1)));
    assert_eq!(editor.git_status_of(pr), None, "not a status");
    // Its text goes in as a status's, and the person can come back to it and kill it.
    editor.set_git_text(pr, text(), shown(0, Vec::new()));
    assert!(editor.buffers_by_recency().contains(&pr));
    editor.show(pr);
    editor.kill_by_person(pr, false).unwrap();
    assert!(editor.buffer(pr).is_none());
    assert_eq!(editor.git_pr_of(pr), None);
}

#[test]
fn an_issue_s_buffer_is_listed_killed_and_never_taken_for_a_pull_request_s() {
    let mut editor = Editor::default();
    let issue = editor.add_git_issue(ProjectId(0), "*issue: #45*");
    let list = editor.add_git_issue_list(ProjectId(0), "*issues: oso*");
    let pr = editor.add_git_pr(ProjectId(0), "*pr: #12*");
    assert_eq!(editor.buffer(issue).map(|b| b.mode()), Some(Mode::GitIssue));
    assert_eq!(
        editor.buffer(list).map(|b| b.mode()),
        Some(Mode::GitIssueList)
    );
    assert_eq!(editor.git_issue_of(issue), Some(ProjectId(0)));
    assert_eq!(editor.git_pr_of(issue), None);
    assert_eq!(editor.git_issue_of(pr), None);
    editor.set_git_text(issue, text(), shown(0, Vec::new()));
    assert!(editor.buffers_by_recency().contains(&issue));
    editor.show(issue);
    editor.kill_by_person(issue, false).unwrap();
    assert_eq!(editor.git_issue_of(issue), None);
}

#[test]
fn requests_go_to_the_host_in_order() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    let lines = vec![GitLines {
        first: 3,
        last: 3,
        region: false,
    }];
    editor.request_git(GitRequest::Refresh { buffer: id });
    editor.request_git(GitRequest::Stage {
        buffer: id,
        lines: lines.clone(),
    });
    assert_eq!(
        editor.take_git_requests(),
        [
            GitRequest::Refresh { buffer: id },
            GitRequest::Stage { buffer: id, lines }
        ]
    );
    assert!(editor.take_git_requests().is_empty());
}

#[test]
fn the_highlights_a_new_text_replaces_go_back_to_the_host() {
    let mut editor = Editor::default();
    let id = editor.add_git_status(ProjectId(0), "*git: oso*");
    assert!(
        editor
            .set_git_text(id, text(), shown(0, Vec::new()))
            .is_none()
    );
    // The ones before come back, for the host to free off the UI thread.
    let given = editor.set_git_text(id, text(), shown(0, Vec::new()));
    assert_eq!(given.map(|given| given.len()), Some(TEXT.len()));
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(
        buffer.highlights().and_then(|h| h.at(11)),
        Some(Highlight::GitBranchLocal)
    );
}

#[test]
fn two_statuses_of_one_name_are_told_apart() {
    let mut editor = Editor::default();
    let first = editor.add_git_status(ProjectId(0), "*git: oso*");
    let second = editor.add_git_status(ProjectId(1), "*git: oso*");
    assert_eq!(editor.buffer(first).unwrap().name(), "*git: oso*");
    assert_eq!(editor.buffer(second).unwrap().name(), "*git: oso*<2>");
    assert_eq!(editor.buffer_named("*git: oso*<2>"), Some(second));
}

#[test]
fn a_log_grows_at_its_end_and_keeps_its_last_whole_lines() {
    let mut editor = Editor::default();
    let log = editor.add_internal_buffer("*git log: oso*", Mode::GitLog);
    editor.show(log);
    editor.append_git_log(log, "$ git one\n");
    editor.append_git_log(log, "out\n");
    let text = |editor: &Editor| editor.buffer(log).unwrap().text().to_string();
    assert_eq!(text(&editor), "$ git one\nout\n");
    // Point at the end stays at the end, as a process buffer's.
    let end = editor.buffer(log).unwrap().text().len();
    editor.current_mut().0.selections = crate::selection::SelectionSet::new(end);
    editor.append_git_log(log, "more\n");
    assert_eq!(
        editor.window().selections.primary().cursor,
        text(&editor).len()
    );
    // Past the limit the oldest lines go, whole.
    let line = format!("{}\n", "x".repeat(1023));
    let many = line.repeat(super::MAX_GIT_LOG / 1024);
    editor.append_git_log(log, &many);
    let kept = text(&editor);
    assert!(kept.len() <= super::MAX_GIT_LOG);
    assert!(kept.starts_with('x') && kept.ends_with('\n'));
    // A piece larger than the whole keeps its own last lines.
    editor.append_git_log(log, &format!("{many}last\n"));
    let kept = text(&editor);
    assert!(kept.len() <= super::MAX_GIT_LOG);
    assert!(kept.ends_with("last\n"));
    // Another buffer takes nothing.
    let other = editor.add_internal_buffer("*other*", Mode::Fundamental);
    editor.append_git_log(other, "x\n");
    assert_eq!(editor.buffer(other).unwrap().text().len(), 0);
}

#[test]
fn the_logs_git_runs_in_are_listed_while_they_are_there() {
    let mut editor = Editor::default();
    let first = editor.add_internal_buffer("*git log: a*", Mode::GitLog);
    let second = editor.add_internal_buffer("*git log: b*", Mode::GitLog);
    editor.set_git_running(first, true);
    editor.set_git_running(second, true);
    editor.set_git_running(first, true);
    assert_eq!(editor.git_runs().collect::<Vec<_>>(), [second, first]);
    editor.set_git_running(second, false);
    assert_eq!(editor.git_runs().collect::<Vec<_>>(), [first]);
    // A log is killed as a status is, and one gone is not listed.
    editor.kill_buffer(first);
    assert!(editor.buffer(first).is_none());
    assert_eq!(editor.git_runs().count(), 0);
}

#[test]
fn a_progress_line_never_covers_news() {
    let mut editor = Editor::default();
    editor.echo_unless_news("", "git commit… 1 s");
    editor.echo_unless_news("git commit… 1 s", "git commit… 2 s");
    assert_eq!(editor.echo_text(), "git commit… 2 s");
    editor.echo("Saved");
    editor.echo_unless_news("git commit… 2 s", "git commit… 3 s");
    assert_eq!(editor.echo_text(), "Saved");
}
