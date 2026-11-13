//! The review's commands on an editor with a review in it: moving by its rows, opening a scope
//! only in a trusted project, refreshing, burying, listing the keys and killing.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::{Baseline, Buffer};
use oso_core::command::Args;
use oso_core::editor::{Editor, ReviewId, ReviewRequest, ScopeKind};
use oso_core::load::decode_all;
use oso_core::project::{Found, ProjectId, RootIdentity, TrustList};
use oso_core::review::{Anchor, Author, Base, Hunk, RowKind, Rows, Section, Shape, Side};
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;
use oso_core::window::ViewSize;

use super::super::tests::{cursor, run};

/// An editor showing a file of the project `/home/me/src/oso`, trusted if `trusted`.
pub(crate) fn in_project(trusted: bool) -> (Editor, ProjectId) {
    let mut buffer = Buffer::visiting(PathBuf::from("/home/me/src/oso/a.rs"));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/home/me/src/oso/a.rs"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(24, 120));
    editor.set_home(Some(PathBuf::from("/home/me")));
    let id = editor.tiled().0.buffer();
    editor.request_project(id);
    let request = editor.take_project_requests().pop().unwrap();
    let found = Found {
        root: RootIdentity {
            path: PathBuf::from("/home/me/src/oso"),
            identity: cfg!(unix).then_some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    let record = if cfg!(unix) {
        "/home/me/src/oso\tdev=1 ino=7\n"
    } else {
        "/home/me/src/oso\n"
    };
    editor.set_trust_list(Some(TrustList::parse(if trusted { record } else { "" }).0));
    let project = editor.current().1.project().unwrap();
    (editor, project)
}

/// The text of two files' sections, as the view thread lays them out, and their rows.
const TEXT: &str = "Review\nmodified  a  +2 \u{2212}1\n@@ -1 +1,2 @@\n-x\n+y\n+z\nadded  b  +1 \u{2212}0\n@@ -0,0 +1 @@\n+w\n";

fn rows() -> Rows {
    let mut title = Section::new(Box::default());
    title.push(RowKind::Note, 1);
    let mut a = Section::new(b"a".as_slice().into());
    a.push(RowKind::File, 1);
    a.push(RowKind::Hunk { hunk: 0 }, 1);
    let lines = |side, first| RowKind::Lines {
        hunk: 0,
        side,
        first,
    };
    a.push(lines(Side::Old, 1), 1);
    a.push(lines(Side::New, 1), 2);
    let mut b = Section::new(b"b".as_slice().into());
    b.push(RowKind::File, 1);
    b.push(RowKind::Hunk { hunk: 0 }, 1);
    b.push(lines(Side::New, 1), 1);
    Rows::new(vec![Arc::new(title), Arc::new(a), Arc::new(b)])
}

/// A review of `project` shown in the selected window, holding [`TEXT`].
fn with_review(editor: &mut Editor, project: ProjectId) -> ReviewId {
    let (review, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    let window = editor.window().id();
    editor.show_in(window, buffer);
    let text = decode_all(TEXT.as_bytes(), Default::default()).compared_with(&Rope::new());
    editor.set_review_text(review, text, rows(), Highlights::new(TEXT.len()));
    review
}

fn line(editor: &Editor) -> usize {
    editor
        .current()
        .1
        .text()
        .offset_to_point(cursor(editor))
        .row
}

#[test]
fn n_p_and_shifted_move_by_hunk_and_file_by_the_rows() {
    let (mut editor, project) = in_project(true);
    with_review(&mut editor, project);
    assert_eq!(run(&mut editor, "review-next-hunk", Args::NONE).0, Ok(()));
    assert_eq!(line(&editor), 2);
    assert_eq!(run(&mut editor, "review-next-hunk", Args::NONE).0, Ok(()));
    assert_eq!(line(&editor), 7);
    let (result, effects) = run(&mut editor, "review-next-hunk", Args::NONE);
    assert_eq!(result, Err("No next hunk".into()));
    assert!(!effects.redraw);
    assert_eq!(
        run(&mut editor, "review-previous-file", Args::NONE).0,
        Ok(())
    );
    assert_eq!(line(&editor), 6);
    assert_eq!(
        run(&mut editor, "review-previous-file", Args::NONE).0,
        Ok(())
    );
    assert_eq!(line(&editor), 1);
    assert_eq!(
        run(&mut editor, "review-next-file", Args::numeric(5)).0,
        Err("No next file".into())
    );
    assert_eq!(line(&editor), 6, "as far as it goes");
    assert_eq!(
        run(&mut editor, "review-previous-hunk", Args::numeric(-1)).0,
        Ok(())
    );
    assert_eq!(line(&editor), 7);
}

#[test]
fn motions_refuse_outside_a_review() {
    let (mut editor, _) = in_project(true);
    assert_eq!(
        run(&mut editor, "review-next-hunk", Args::NONE).0,
        Err("Not in a review".into())
    );
}

#[test]
fn opening_asks_the_host_only_in_a_trusted_project() {
    let (mut editor, project) = in_project(false);
    let (result, _) = run(&mut editor, "review-uncommitted", Args::NONE);
    assert_eq!(
        result,
        Err("This project is not trusted to run git: M-x trust-project".into())
    );
    assert!(editor.take_review_requests().is_empty());
    let (mut editor, project_trusted) = in_project(true);
    assert_eq!(run(&mut editor, "review-staged", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Open {
            project: project_trusted,
            scope: ScopeKind::Staged,
            revision: None
        }]
    );
    let _ = project;
    assert_eq!(
        run(&mut editor, "review-session", Args::NONE).0,
        Err("No baseline: B takes one".into())
    );
    assert_eq!(
        run(&mut editor, "review-branch", Args::NONE).0,
        Err("The branch's base is not known yet: C-u b names it".into())
    );
    assert_eq!(
        run(&mut editor, "review-mark-baseline", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Baseline {
            project: project_trusted
        }]
    );
}

#[test]
fn g_refreshes_q_buries_and_question_mark_lists_the_keys() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    assert_eq!(run(&mut editor, "review-refresh", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Refresh(review)]
    );
    assert_eq!(run(&mut editor, "review-keys", Args::NONE).0, Ok(()));
    let (_, keys) = editor.current();
    assert_eq!(keys.name(), "*Review keys*");
    let text = keys.text().to_string();
    for (key, name) in super::KEYS {
        assert!(text.contains(&format!("{key:<6}{name}")), "{key} {name}");
    }
    assert!(text.contains("Move every cursor to the header of the Nth next hunk"));
    // A second `?` reuses the buffer.
    let before = editor.buffers().count();
    assert_eq!(run(&mut editor, "review-keys", Args::NONE).0, Ok(()));
    assert_eq!(editor.buffers().count(), before);
    let buffer = editor.review_buffer(review).unwrap();
    let window = editor.window().id();
    editor.show_in(window, buffer);
    assert_eq!(run(&mut editor, "review-quit", Args::NONE).0, Ok(()));
    assert_ne!(editor.window().buffer(), buffer);
    assert!(editor.review(review).is_some(), "only buried");
}

#[test]
fn killing_a_review_asks_only_while_it_holds_unsent_comments_of_yours() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    assert_eq!(run(&mut editor, "review-kill", Args::NONE).0, Ok(()));
    assert!(editor.review(review).is_none());
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Close(review)]
    );

    let review = with_review(&mut editor, project);
    let shape = Arc::new(
        Shape::new(
            Base::Unknown,
            vec![Hunk {
                old_start: 1,
                new_start: 1,
                removed: vec![b"x\n".as_slice().into()],
                added: vec![b"y\n".as_slice().into()],
            }],
        )
        .unwrap(),
    );
    let model = editor.review_mut(review).unwrap();
    let carried = model.carry(b"a", shape).run(|_, _| None, |_, _| Vec::new());
    model.apply(carried);
    let anchor = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    model
        .add_comment(Author::Person, b"a", anchor, "fix this".into())
        .unwrap();
    assert_eq!(run(&mut editor, "review-kill", Args::NONE).0, Ok(()));
    let question = editor.prompt().map(|prompt| prompt.label().to_owned());
    assert!(question.is_some_and(|q| q.starts_with("1 comment of yours will be lost; kill?")));
    assert!(editor.review(review).is_some());
}
