//! The comment commands of a review buffer on an editor with a review that holds comments:
//! what each key does on each kind of row, what it refuses, the motions by comment, keeping a
//! draft, and the questions that count an open draft.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use oso_core::buffer::Replacement;
use oso_core::command::{Args, Reach};
use oso_core::editor::{DraftFor, Editor, ReviewId, ReviewRequest};
use oso_core::review::{Anchor, Author, MAX_COMMENT_BYTES, Side};

use super::super::tests::{Comments, in_project, lines, lines_of, put_at, with_comments};
use super::super::{unsent, unsent_everywhere};
use crate::commands::tests::run;

/// An editor showing a review with comments, its cursor on `line`.
fn at(line: usize) -> (Editor, ReviewId, Comments) {
    let (mut editor, project) = in_project(true);
    let (review, comments) = with_comments(&mut editor, project);
    put_at(&mut editor, &[line]);
    (editor, review, comments)
}

/// What the open draft is for, closing it.
fn drafted(editor: &mut Editor) -> Option<DraftFor> {
    let draft = editor.draft().map(|(_, draft)| draft.clone());
    if draft.is_some() {
        editor.quit();
    }
    draft
}

fn type_in(editor: &mut Editor, text: &str) {
    let buffer = editor.prompt().unwrap().buffer();
    let end = editor.buffer(buffer).unwrap().text().len();
    let edit = Replacement {
        range: end..end,
        text,
    };
    editor.apply_edits(buffer, &[edit]).unwrap();
}

fn redraw_of(path: &[u8], review: ReviewId) -> ReviewRequest {
    ReviewRequest::Redraw {
        review,
        path: path.into(),
    }
}

#[test]
fn c_drafts_a_comment_on_a_line_or_a_hunk_and_a_reply_on_a_comment() {
    let (mut editor, _, comments) = at(lines::OLD);
    let a: Box<[u8]> = b"a".as_slice().into();
    let cases = [
        (
            lines::OLD,
            DraftFor::Comment {
                path: a.clone(),
                anchor: Anchor::Line {
                    side: Side::Old,
                    number: 1,
                },
            },
        ),
        (
            lines::NEW_2,
            DraftFor::Comment {
                path: a.clone(),
                anchor: Anchor::Line {
                    side: Side::New,
                    number: 2,
                },
            },
        ),
        (
            lines::HUNK,
            DraftFor::Comment {
                path: a,
                anchor: Anchor::Hunk { index: 0 },
            },
        ),
        (lines::MINE, DraftFor::Reply { to: comments.mine }),
        (
            lines::AGENTS_CARD,
            DraftFor::Reply {
                to: comments.agents,
            },
        ),
    ];
    for (line, wanted) in cases {
        put_at(&mut editor, &[line]);
        let (result, effects) = run(&mut editor, "review-comment", Args::NONE);
        assert_eq!(result, Ok(()), "{line}");
        assert!(effects.redraw);
        assert_eq!(drafted(&mut editor), Some(wanted), "{line}");
    }
    for line in [lines::TITLE, lines::FILE] {
        put_at(&mut editor, &[line]);
        assert_eq!(
            run(&mut editor, "review-comment", Args::NONE).0,
            Err("Comment on a line or a hunk's header".into()),
            "{line}"
        );
        assert!(editor.prompt().is_none());
    }
}

#[test]
fn comment_commands_act_on_the_primary_cursor_and_refuse_in_a_prompt_or_elsewhere() {
    let (mut editor, _, comments) = at(lines::MINE);
    put_at(&mut editor, &[lines::MINE, lines::AGENTS]);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.draft().map(|(_, draft)| draft.clone()),
        Some(DraftFor::Reply { to: comments.mine })
    );
    for name in [
        "review-comment",
        "review-edit-comment",
        "review-remove-comment",
        "review-resolve",
        "review-fold-agents",
    ] {
        assert_eq!(
            run(&mut editor, name, Args::NONE).0,
            Err("Not in the minibuffer".into()),
            "{name}"
        );
    }
    editor.quit();
    let (mut editor, _) = in_project(true);
    assert_eq!(
        run(&mut editor, "review-comment", Args::NONE).0,
        Err("Not in a review".into())
    );
}

#[test]
fn e_edits_only_your_own_comment_holding_its_text() {
    let (mut editor, _, comments) = at(lines::MINE_CARD);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Ok(())
    );
    assert_eq!(editor.current().1.text().to_string(), "fix this");
    assert_eq!(
        drafted(&mut editor),
        Some(DraftFor::Edit {
            comment: comments.mine
        })
    );
    put_at(&mut editor, &[lines::AGENTS]);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Err("Not yours: c replies".into())
    );
    put_at(&mut editor, &[lines::OLD]);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Err("Not on a comment".into())
    );
    assert!(editor.prompt().is_none());
}

#[test]
fn k_removes_your_own_thread_into_the_kill_ring_and_refuses_the_agents() {
    let (mut editor, review, comments) = at(lines::AGENTS_CARD);
    assert_eq!(
        run(&mut editor, "review-remove-comment", Args::NONE).0,
        Err("Not yours: c replies".into())
    );
    assert!(
        editor
            .review(review)
            .unwrap()
            .comment(comments.agents)
            .is_some()
    );
    put_at(&mut editor, &[lines::MY_REPLY]);
    assert_eq!(
        run(&mut editor, "review-remove-comment", Args::NONE).0,
        Ok(())
    );
    assert!(editor.prompt().is_none(), "it asks nothing");
    assert_eq!(
        crate::commands::kept_messages(&editor)
            .last()
            .map(String::as_str),
        Some(format!("Removed #{}: C-y yanks it", comments.my_reply.get()).as_str())
    );
    assert_eq!(editor.take_review_requests(), [redraw_of(b"a", review)]);
    put_at(&mut editor, &[lines::MINE]);
    assert_eq!(
        run(&mut editor, "review-remove-comment", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        crate::commands::kept_messages(&editor)
            .last()
            .map(String::as_str),
        Some(
            format!(
                "Removed #{} with 1 reply: C-y yanks it",
                comments.mine.get()
            )
            .as_str()
        )
    );
    let model = editor.review(review).unwrap();
    assert!(model.comment(comments.mine).is_none());
    assert!(model.comment(comments.agents_reply).is_none());
    assert!(
        editor
            .kill_ring()
            .get(0)
            .unwrap()
            .joined()
            .contains("fix this")
    );
}

#[test]
fn r_resolves_and_reopens_the_threads_root() {
    let (mut editor, review, comments) = at(lines::MY_REPLY);
    assert_eq!(run(&mut editor, "review-resolve", Args::NONE).0, Ok(()));
    let model = editor.review(review).unwrap();
    assert!(model.comment(comments.mine).unwrap().is_resolved());
    assert!(!model.comment(comments.my_reply).unwrap().is_resolved());
    assert_eq!(editor.take_review_requests(), [redraw_of(b"a", review)]);
    assert_eq!(run(&mut editor, "review-resolve", Args::NONE).0, Ok(()));
    let model = editor.review(review).unwrap();
    assert!(!model.comment(comments.mine).unwrap().is_resolved());
    // The person may resolve the agent's too.
    put_at(&mut editor, &[lines::AGENTS]);
    assert_eq!(run(&mut editor, "review-resolve", Args::NONE).0, Ok(()));
    let model = editor.review(review).unwrap();
    assert!(model.comment(comments.agents).unwrap().is_resolved());
    put_at(&mut editor, &[lines::HUNK]);
    assert_eq!(
        run(&mut editor, "review-resolve", Args::NONE).0,
        Err("Not on a comment".into())
    );
}

#[test]
fn a_folds_the_agents_comments_and_shows_them_again() {
    let (mut editor, review, _) = at(lines::TITLE);
    assert!(!editor.agents_folded(review));
    let (result, effects) = run(&mut editor, "review-fold-agents", Args::NONE);
    assert_eq!(result, Ok(()));
    assert!(effects.redraw);
    assert!(editor.agents_folded(review));
    assert_eq!(run(&mut editor, "review-fold-agents", Args::NONE).0, Ok(()));
    assert!(!editor.agents_folded(review));
}

#[test]
fn braces_move_every_cursor_by_comment_with_a_count() {
    let (mut editor, _, _) = at(lines::TITLE);
    put_at(&mut editor, &[lines::TITLE, lines::MINE_CARD]);
    assert_eq!(
        run(&mut editor, "review-next-comment", Args::NONE).0,
        Ok(())
    );
    assert_eq!(lines_of(&editor), [lines::MINE, lines::MY_REPLY]);
    put_at(&mut editor, &[lines::TITLE]);
    assert_eq!(
        run(&mut editor, "review-next-comment", Args::numeric(2)).0,
        Ok(())
    );
    assert_eq!(lines_of(&editor), [lines::MY_REPLY]);
    let (result, _) = run(&mut editor, "review-next-comment", Args::numeric(5));
    assert_eq!(result, Err("No next comment".into()));
    assert_eq!(lines_of(&editor), [lines::AGENTS], "as far as it goes");
    put_at(&mut editor, &[lines::B_LINE]);
    assert_eq!(
        run(&mut editor, "review-previous-comment", Args::numeric(2)).0,
        Ok(())
    );
    assert_eq!(lines_of(&editor), [lines::MY_REPLY]);
    assert_eq!(
        run(&mut editor, "review-next-comment", Args::numeric(-1)).0,
        Ok(())
    );
    assert_eq!(lines_of(&editor), [lines::MINE]);
    let (result, effects) = run(&mut editor, "review-previous-comment", Args::NONE);
    assert_eq!(result, Err("No comment before".into()));
    assert!(!effects.redraw);
    let registry = crate::commands::registry();
    for name in ["review-next-comment", "review-previous-comment"] {
        let id = registry.lookup(name).unwrap();
        assert_eq!(registry.get(id).unwrap().reach, Reach::Look, "{name}");
    }
}

#[test]
fn keeping_a_draft_adds_replies_and_edits_as_yours() {
    let (mut editor, review, comments) = at(lines::OLD);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    type_in(&mut editor, "why x?\nit was fine");
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    assert!(editor.prompt().is_none());
    assert!(editor.take_prompt_outcome().is_none(), "taken");
    let model = editor.review(review).unwrap();
    let added = model.comments().last().unwrap();
    assert_eq!(added.author(), Author::Person);
    assert_eq!(added.text(), "why x?\nit was fine");
    assert_eq!(
        added.anchor(),
        Anchor::Line {
            side: Side::Old,
            number: 1
        }
    );
    assert_eq!(editor.take_review_requests(), [redraw_of(b"a", review)]);

    put_at(&mut editor, &[lines::AGENTS]);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    type_in(&mut editor, "because");
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    let model = editor.review(review).unwrap();
    let reply = model.comments().last().unwrap();
    assert_eq!(reply.reply_to(), Some(comments.agents));
    assert_eq!(reply.author(), Author::Person);

    put_at(&mut editor, &[lines::MINE]);
    editor
        .review_mut(review)
        .unwrap()
        .mark_sent(&[comments.mine]);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Ok(())
    );
    type_in(&mut editor, " now");
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    let mine = editor
        .review(review)
        .unwrap()
        .comment(comments.mine)
        .unwrap();
    assert_eq!(mine.text(), "fix this now");
    assert!(!mine.is_sent(), "unsent again");
}

#[test]
fn a_draft_too_long_or_empty_stays_open_and_says_so() {
    let (mut editor, review, _) = at(lines::OLD);
    let before = editor.review(review).unwrap().comments().count();
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.prompt().and_then(|prompt| prompt.message()),
        Some("Type a comment, or C-g")
    );
    type_in(&mut editor, &"x".repeat(MAX_COMMENT_BYTES + 1));
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    assert!(editor.draft().is_some(), "still open");
    assert_eq!(
        editor.prompt().and_then(|prompt| prompt.message()),
        Some("Too long: a comment holds 64 KiB at most")
    );
    assert_eq!(editor.review(review).unwrap().comments().count(), before);
    assert!(editor.take_review_requests().is_empty());
}

#[test]
fn a_draft_whose_target_is_gone_goes_to_the_kill_ring() {
    // `b` has no reading, as a file read again and gone has none.
    let (mut editor, review, comments) = at(lines::B_LINE);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    type_in(&mut editor, "lost?");
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    assert!(editor.prompt().is_none(), "closed");
    assert_eq!(editor.kill_ring().get(0).unwrap().joined(), "lost?");
    assert_eq!(
        crate::commands::kept_messages(&editor)
            .last()
            .map(String::as_str),
        Some("Comment target gone: text in the kill ring")
    );
    // A reply to a comment removed meanwhile.
    put_at(&mut editor, &[lines::AGENTS]);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    type_in(&mut editor, "and this");
    let model = editor.review_mut(review).unwrap();
    model
        .remove_comment(Author::Agent, comments.agents)
        .unwrap();
    assert_eq!(run(&mut editor, "review-keep-draft", Args::NONE).0, Ok(()));
    assert!(editor.prompt().is_none());
    assert_eq!(editor.kill_ring().get(0).unwrap().joined(), "and this");
    assert!(editor.take_review_requests().is_empty());
    assert_eq!(
        run(&mut editor, "review-keep-draft", Args::NONE).0,
        Err("Not in a comment's draft".into())
    );
}

#[test]
fn an_open_draft_counts_as_an_unsent_comment_when_closing() {
    let (mut editor, review, comments) = at(lines::OLD);
    // Yours and your reply.
    assert_eq!(unsent(&editor, review), 2);
    assert_eq!(run(&mut editor, "review-comment", Args::NONE).0, Ok(()));
    assert_eq!(unsent(&editor, review), 3);
    assert_eq!(unsent_everywhere(&editor), 3);
    assert_eq!(
        run(&mut editor, "save-buffers-kill-terminal", Args::NONE).0,
        Ok(())
    );
    let question = editor.prompt().map(|prompt| prompt.label().to_owned());
    assert!(
        question.is_some_and(|q| q.contains("3 comments of yours will be lost")),
        "{:?}",
        editor.prompt().map(|prompt| prompt.label())
    );
    editor.quit();
    editor.quit();
    assert!(editor.prompt().is_none());
    // Editing a comment counted already counts it once.
    put_at(&mut editor, &[lines::MINE]);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Ok(())
    );
    assert_eq!(unsent(&editor, review), 2);
    editor.quit();
    editor
        .review_mut(review)
        .unwrap()
        .mark_sent(&[comments.mine]);
    assert_eq!(
        run(&mut editor, "review-edit-comment", Args::NONE).0,
        Ok(())
    );
    assert_eq!(unsent(&editor, review), 2, "your reply and the edit");
}
