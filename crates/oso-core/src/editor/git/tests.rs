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
