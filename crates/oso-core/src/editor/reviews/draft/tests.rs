//! The tests of a review comment's draft and of the person's removal of a comment: what a
//! draft is for and says, how its answer grows, what `C-g` keeps, and what a removed thread
//! leaves in the kill ring.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files.

use std::sync::Arc;

use super::super::ReviewId;
use super::DraftFor;
use crate::buffer::Replacement;
use crate::editor::{Editor, Quit};
use crate::frame::FrameSize;
use crate::minibuffer::{Kind, PromptError};
use crate::project::ProjectId;
use crate::review::{Anchor, Author, Base, CommentError, Hunk, Shape, Side};

const FILE: &[u8] = b"src/a.rs";

/// An editor with a review of [`FILE`], one hunk adding three lines at line 1.
fn editor_with_file() -> (Editor, ReviewId) {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 100,
    });
    let (id, buffer) = editor.add_review(ProjectId(0), "*review*".into());
    editor.show(buffer);
    let hunk = Hunk {
        old_start: 1,
        new_start: 1,
        removed: Vec::new(),
        added: ["a", "b", "c"]
            .iter()
            .map(|l| l.as_bytes().into())
            .collect(),
    };
    let shape = Arc::new(Shape::new(Base::Unknown, vec![hunk]).unwrap());
    let review = editor.review_mut(id).unwrap();
    let carried = review
        .carry(FILE, shape)
        .run(|_, _| None, |_, _| Vec::new());
    review.apply(carried);
    (editor, id)
}

fn line(number: u32) -> Anchor {
    Anchor::Line {
        side: Side::New,
        number,
    }
}

fn type_in(editor: &mut Editor, text: &str) {
    let buffer = editor.current().0.buffer();
    let end = editor.buffer(buffer).unwrap().text().len();
    editor
        .apply_edits(
            buffer,
            &[Replacement {
                range: end..end,
                text,
            }],
        )
        .unwrap();
}

fn answer_text(editor: &Editor) -> String {
    editor.current().1.text().to_string()
}

#[test]
fn a_draft_says_what_it_is_for_and_remembers_it_until_it_closes() {
    let (mut editor, id) = editor_with_file();
    let comment = editor
        .review_mut(id)
        .unwrap()
        .add_comment(Author::Person, FILE, line(2), "mine".into())
        .unwrap();
    let cases = [
        (
            DraftFor::Comment {
                path: FILE.into(),
                anchor: line(2),
            },
            "Comment on src/a.rs L2 (new)",
        ),
        (
            DraftFor::Comment {
                path: FILE.into(),
                anchor: Anchor::Hunk { index: 0 },
            },
            "Comment on src/a.rs hunk 1",
        ),
        (DraftFor::Reply { to: comment }, "Reply to #0"),
        (DraftFor::Edit { comment }, "Edit #0"),
    ];
    for (draft, label) in cases {
        editor.open_draft(id, draft.clone(), "").unwrap();
        assert_eq!(editor.prompt().unwrap().label(), label);
        assert_eq!(editor.prompt().unwrap().kind(), Kind::Draft);
        assert_eq!(editor.draft(), Some((id, &draft)));
        assert!(editor.current().1.wraps(), "a draft wraps");
        assert_eq!(editor.quit(), Quit::Prompt);
        assert_eq!(editor.draft(), None);
    }
}

#[test]
fn a_draft_opens_in_no_prompt_and_only_for_an_open_review() {
    let (mut editor, id) = editor_with_file();
    let to = editor
        .review_mut(id)
        .unwrap()
        .add_comment(Author::Agent, FILE, line(1), "note".into())
        .unwrap();
    let draft = DraftFor::Reply { to };
    editor
        .open_prompt("Name", Kind::Text { history: None })
        .unwrap();
    assert_eq!(
        editor.open_draft(id, draft.clone(), ""),
        Err(PromptError::TooDeep)
    );
    assert_eq!(editor.prompts().len(), 1);
    editor.quit();
    editor.remove_review(id);
    assert_eq!(editor.open_draft(id, draft, ""), Err(PromptError::Invalid));
    assert!(editor.prompts().is_empty());
}

#[test]
fn a_draft_holds_the_text_it_was_given_with_the_cursor_after_it() {
    let (mut editor, id) = editor_with_file();
    let comment = editor
        .review_mut(id)
        .unwrap()
        .add_comment(Author::Person, FILE, line(1), "old\ntext".into())
        .unwrap();
    editor
        .open_draft(id, DraftFor::Edit { comment }, "old\ntext")
        .unwrap();
    assert_eq!(answer_text(&editor), "old\ntext");
    assert_eq!(editor.current().0.selections.primary().cursor, 8);
}

#[test]
fn a_draft_with_text_goes_to_the_kill_ring_on_quit_and_an_empty_one_does_not() {
    let (mut editor, id) = editor_with_file();
    let draft = DraftFor::Comment {
        path: FILE.into(),
        anchor: line(1),
    };
    editor.open_draft(id, draft.clone(), "").unwrap();
    editor.quit();
    assert!(editor.kill_ring().is_empty());
    editor.open_draft(id, draft, "").unwrap();
    type_in(&mut editor, "two\nlines");
    assert_eq!(editor.quit(), Quit::Prompt);
    assert!(editor.prompts().is_empty());
    assert_eq!(
        editor
            .kill_ring()
            .get(0)
            .map(|entry| entry.joined())
            .as_deref(),
        Some("two\nlines")
    );
}

#[test]
fn the_answer_grows_with_the_text_to_ten_rows_and_its_window_with_it() {
    let (mut editor, id) = editor_with_file();
    let draft = DraftFor::Comment {
        path: FILE.into(),
        anchor: line(1),
    };
    editor.open_draft(id, draft, "").unwrap();
    let rows = |editor: &Editor| editor.prompt_layout().unwrap().input.rows;
    assert_eq!(rows(&editor), 1);
    assert_eq!(editor.current().0.size().rows, 1);
    type_in(&mut editor, "a\nb\nc");
    assert_eq!(rows(&editor), 3);
    assert_eq!(editor.current().0.size().rows, 3);
    let columns = editor.prompt_layout().unwrap().input.columns;
    type_in(&mut editor, &"x".repeat(columns));
    assert_eq!(rows(&editor), 4, "a long line wraps");
    type_in(&mut editor, &"\n".repeat(30));
    assert_eq!(rows(&editor), 10);
    assert_eq!(editor.current().0.size().rows, 10);
    assert!(editor.current().0.wrap_map().is_some(), "its window wraps");
    editor.quit();
}

#[test]
fn removing_a_thread_puts_it_in_the_kill_ring_and_refuses_an_agents() {
    let (mut editor, id) = editor_with_file();
    let review = editor.review_mut(id).unwrap();
    let mine = review
        .add_comment(Author::Person, FILE, line(1), "why?".into())
        .unwrap();
    let reply = review.reply(Author::Agent, mine, "because".into()).unwrap();
    review.reply(Author::Person, reply, "ok".into()).unwrap();
    let theirs = review
        .add_comment(Author::Agent, FILE, line(2), "note".into())
        .unwrap();
    assert_eq!(
        editor.kill_review_comment(id, theirs),
        Err(CommentError::NotYours)
    );
    assert_eq!(
        editor.kill_review_comment(id, reply),
        Err(CommentError::NotYours)
    );
    assert_eq!(editor.kill_review_comment(id, mine), Ok(3));
    assert_eq!(
        editor
            .kill_ring()
            .get(0)
            .map(|entry| entry.joined())
            .as_deref(),
        Some("You #0\nwhy?\nAgent #1\nbecause\nYou #2\nok")
    );
    let left: Vec<_> = editor
        .review(id)
        .unwrap()
        .comments()
        .map(|c| c.id())
        .collect();
    assert_eq!(left, [theirs]);
    assert_eq!(
        editor.kill_review_comment(id, mine),
        Err(CommentError::NoSuchComment)
    );
}
