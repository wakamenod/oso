//! The files pane's commands on an editor with a review in it: `s` shows the pane and selects
//! it, `s` or `q` from it closes it, `n`/`p` walk its files, `RET`/`SPC` put a file's header at
//! the review's top, `/` keeps a filter beside the review and opens holding it, and `?` lists
//! the pane's keys.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use std::sync::Arc;

use oso_core::command::{Args, Ctx, ExecuteError, Invocation};
use oso_core::editor::{Editor, ReviewId};
use oso_core::load::decode_all;
use oso_core::review::{RowKind, Rows, Section};
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::super::tests::{in_project, with_review};
use crate::commands::registry;
use crate::commands::tests::run;

/// The pane's text, as the view thread lays it out for the review's files `a` and `b`.
const PANE: &str = "2 files \u{b7} / to filter\nM +2 \u{2212}1  a\nA +1 \u{2212}0  b\n";

fn pane_rows() -> Rows {
    let mut title = Section::new(Box::default());
    title.push(RowKind::Note, 1);
    let file = |path: &[u8]| {
        let mut section = Section::new(path.into());
        section.push(RowKind::File, 1);
        Arc::new(section)
    };
    Rows::new(vec![Arc::new(title), file(b"a"), file(b"b")])
}

/// An editor showing a review with its files pane shown and selected, holding [`PANE`].
fn with_pane() -> (Editor, ReviewId) {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    assert_eq!(run(&mut editor, "review-files", Args::NONE).0, Ok(()));
    let text = decode_all(PANE.as_bytes(), Default::default()).compared_with(&Rope::new());
    let _ = editor.set_review_files(review, text, pane_rows(), Highlights::new(PANE.len()));
    (editor, review)
}

fn line(editor: &Editor) -> usize {
    let (window, buffer) = editor.current();
    (buffer.text())
        .offset_to_point(window.selections.primary().cursor)
        .row
}

/// Runs `name` with `answer` as its prompt's.
fn run_answered(editor: &mut Editor, name: &str, answer: &str) -> Result<(), String> {
    let registry = registry();
    let id = registry.lookup(name).unwrap();
    let invocation = Invocation {
        answer: Some(answer),
        ..Invocation::default()
    };
    let mut ctx = Ctx::invoked(editor, invocation);
    (registry.execute(id, &mut ctx, Args::NONE)).map_err(|e| match e {
        ExecuteError::Failed { error, .. } => error.message().to_owned(),
        other => panic!("{other}"),
    })
}

#[test]
fn s_shows_the_pane_and_selects_it_and_s_or_q_from_it_closes_it() {
    let (mut editor, review) = with_pane();
    let pane = editor.review_files(review).expect("a pane");
    assert_eq!(editor.window().buffer(), pane, "selected");
    assert_eq!(editor.windows().count(), 2);
    for key in ["review-files", "review-files"] {
        let (result, effects) = run(&mut editor, key, Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        if editor.review_files(review).is_some() {
            continue;
        }
        assert_eq!(editor.windows().count(), 1, "its columns given back");
        assert_eq!(
            Some(editor.window().buffer()),
            editor.review_buffer(review),
            "the review selected again"
        );
        // `s` in the review again.
        assert_eq!(run(&mut editor, "review-files", Args::NONE).0, Ok(()));
        assert!(editor.review_files(review).is_some());
    }
}

#[test]
fn the_panes_commands_refuse_outside_it() {
    let (mut editor, project) = in_project(true);
    assert_eq!(
        run(&mut editor, "review-files", Args::NONE).0,
        Err("Not in a review".into())
    );
    with_review(&mut editor, project);
    for name in [
        "review-files-next",
        "review-files-visit",
        "review-files-display",
    ] {
        assert_eq!(
            run(&mut editor, name, Args::NONE).0,
            Err("Not in a review's files pane".into()),
            "{name}"
        );
    }
}

#[test]
fn n_and_p_walk_the_files_and_stop_at_either_end() {
    let (mut editor, _) = with_pane();
    // The review's cursor is on its title, which is no file's.
    assert_eq!(line(&editor), 0);
    assert_eq!(
        run(&mut editor, "review-files-next", Args::numeric(2)).0,
        Ok(())
    );
    assert_eq!(line(&editor), 2);
    let (result, effects) = run(&mut editor, "review-files-next", Args::NONE);
    assert_eq!(result, Err("No next file".into()));
    assert!(!effects.redraw);
    assert_eq!(
        run(&mut editor, "review-files-previous", Args::numeric(5)).0,
        Err("No file before".into())
    );
    assert_eq!(line(&editor), 1, "as far as it goes, never the title");
    assert_eq!(
        run(&mut editor, "review-files-previous", Args::numeric(-1)).0,
        Ok(())
    );
    assert_eq!(line(&editor), 2);
}

#[test]
fn spc_shows_the_file_at_the_reviews_top_and_ret_selects_the_review() {
    let (mut editor, review) = with_pane();
    let pane = editor.review_files(review).unwrap();
    let buffer = editor.review_buffer(review).unwrap();
    let review_window = || {
        let window = (editor.windows()).find(|window| window.buffer() == buffer);
        window.unwrap().id()
    };
    let review_window = review_window();
    assert_eq!(
        run(&mut editor, "review-files-next", Args::numeric(2)).0,
        Ok(())
    );
    assert_eq!(
        run(&mut editor, "review-files-display", Args::NONE).0,
        Ok(())
    );
    assert_eq!(editor.window().buffer(), pane, "stays");
    let shown = editor.window_by_id(review_window).unwrap();
    let text = editor.buffer(buffer).unwrap().text();
    let b_header = text.line_start(6);
    assert_eq!(shown.selections.primary().cursor, b_header);
    assert_eq!(shown.scroll().start, b_header, "at the top");
    assert_eq!(run(&mut editor, "review-files-visit", Args::NONE).0, Ok(()));
    assert_eq!(editor.window().id(), review_window, "selected");
    // The title is no file.
    let pane_window = (editor.windows()).find(|window| window.buffer() == pane);
    let pane_window = pane_window.unwrap().id();
    editor.select_window(pane_window);
    let window = editor.window().id();
    editor.restore_positions_in(
        window,
        oso_core::selection::SelectionSet::new(0),
        oso_core::window::Scroll::default(),
    );
    assert_eq!(
        run(&mut editor, "review-files-visit", Args::NONE).0,
        Err("No file here".into())
    );
}

#[test]
fn slash_keeps_a_filter_beside_the_review_and_opens_holding_it() {
    let (mut editor, review) = with_pane();
    assert_eq!(super::prompt_text(&editor, "review-filter"), None);
    assert_eq!(
        run_answered(&mut editor, "review-filter", "  src  "),
        Ok(())
    );
    assert_eq!(editor.review_filter(review).unwrap().text(), "src");
    // From the pane or the review, `/` opens holding it; no other prompt does.
    assert_eq!(
        super::prompt_text(&editor, "review-filter").as_deref(),
        Some("src")
    );
    assert_eq!(super::prompt_text(&editor, "review-commit"), None);
    let buffer = editor.review_buffer(review).unwrap();
    let window = (editor.windows()).find(|window| window.buffer() == buffer);
    editor.select_window(window.unwrap().id());
    assert_eq!(
        super::prompt_text(&editor, "review-filter").as_deref(),
        Some("src")
    );
    // An empty answer shows every file again.
    assert_eq!(run_answered(&mut editor, "review-filter", " "), Ok(()));
    assert!(editor.review_filter(review).is_none());
}

#[test]
fn g_refreshes_from_the_pane_and_question_mark_lists_its_keys() {
    let (mut editor, review) = with_pane();
    assert_eq!(run(&mut editor, "review-refresh", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.take_review_requests(),
        [oso_core::editor::ReviewRequest::Refresh(review)]
    );
    assert_eq!(run(&mut editor, "review-files-keys", Args::NONE).0, Ok(()));
    let (_, keys) = editor.current();
    assert_eq!(keys.name(), "*Review files keys*");
    let text = keys.text().to_string();
    for (key, name) in super::FILES_KEYS {
        assert!(text.contains(&format!("{key:<8}{name}")), "{key} {name}");
    }
    assert!(text.contains("Move the files pane's cursor to the Nth next file"));
    assert!(text.contains("Read the review's files again now"));
}
