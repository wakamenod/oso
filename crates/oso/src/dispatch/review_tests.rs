//! The review menu's notes as the built-in menu draws them (ADR 0117, ADR 0122): each count in one
//! width, the last scope, the base, the line under the title, why a head cannot run, the same
//! in a review buffer, and every head of the built-in table known.
//!
//! Must not depend on: `app` or winit; these read the built-in menu and its describe function.

use std::sync::Arc;

use oso_core::editor::{Count, Editor, ScopeKind};
use oso_core::menu::{Notes, Open};

use crate::commands::review::tests::in_project;

/// The notes the review menu shows now.
fn notes(editor: &Editor) -> Notes {
    let registry = crate::commands::registry();
    let menus = crate::bindings::built_in_menus(&registry);
    let definition = menus.get(menus.lookup("review").unwrap()).unwrap();
    crate::commands::review::review_menu(editor, &Open::new(Arc::clone(definition)))
}

fn note(notes: &Notes, head: usize) -> (&str, Option<&str>) {
    let note = notes.get(head).unwrap();
    (note.text.as_str(), note.unavailable.as_deref())
}

#[test]
fn the_menu_says_each_count_in_one_width_and_why_a_head_cannot_run() {
    let (editor, _) = in_project(false);
    let shown = notes(&editor);
    for head in 0..8 {
        assert_eq!(
            note(&shown, head).1,
            Some("reviewing runs git, which this project is not trusted to run: M-x trust-project"),
            "{head}"
        );
    }
    let editor = Editor::default();
    let shown = notes(&editor);
    assert_eq!(note(&shown, 1).1, Some("not in a git repository"));
    assert_eq!(shown.line, None);
    // Unix only from here, as the trust tests elsewhere are: without a drive, the path is not
    // absolute on Windows, and a record of it trusts nothing there.
    if cfg!(not(unix)) {
        return;
    }
    let (mut editor, project) = in_project(true);
    let generation = editor.start_scope_counts(
        project,
        &[ScopeKind::Uncommitted, ScopeKind::Staged, ScopeKind::Branch],
    );
    let shown = notes(&editor);
    // Heads: D w u s b, c r, B.
    assert_eq!(note(&shown, 1), ("           \u{2026}       ", None));
    assert_eq!(note(&shown, 0).1, Some("no baseline: B takes one"));
    assert_eq!(shown.ret_head, Some(1), "uncommitted with no last scope");
    editor.set_scope_count(
        project,
        generation,
        ScopeKind::Uncommitted,
        Count::Files(12),
    );
    editor.set_scope_count(project, generation, ScopeKind::Staged, Count::More);
    editor.set_scope_count(project, generation, ScopeKind::Branch, Count::Failed);
    editor.set_branch_base(project, generation, Some("origin/main".into()));
    editor.set_last_scope(project, ScopeKind::Staged);
    let shown = notes(&editor);
    assert_eq!(note(&shown, 1).0, "    12 files       ");
    assert_eq!(note(&shown, 3).0, ">10000 files (last)");
    assert_eq!(note(&shown, 4).0, "           ?        \u{b7} origin/main");
    assert_eq!(
        note(&shown, 1).0.chars().count(),
        note(&shown, 3).0.chars().count()
    );
    assert_eq!(shown.ret_head, Some(3));
    let line = shown.line.unwrap();
    assert!(
        line.starts_with("Review \u{b7} oso \u{b7} ~/src/oso"),
        "{line}"
    );
    assert!(!line.contains("kill ring"), "{line}");
}

// Unix only, as the trust tests elsewhere are: without a drive, the path is not absolute on
// Windows, and a record of it trusts nothing there.
#[cfg(unix)]
#[test]
fn in_a_review_buffer_the_menu_is_the_reviews_projects() {
    let (mut editor, project) = in_project(true);
    let generation = editor.start_scope_counts(project, &[ScopeKind::Uncommitted]);
    editor.set_scope_count(project, generation, ScopeKind::Uncommitted, Count::Files(2));
    let (_, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    let window = editor.tiled().0.id();
    editor.show_in(window, buffer);
    let shown = notes(&editor);
    assert_eq!(note(&shown, 1), ("     2 files       ", None));
    assert!(
        shown
            .line
            .is_some_and(|line| line.starts_with("Review \u{b7} oso"))
    );
}

#[test]
fn every_head_of_the_built_in_menu_is_known_by_its_keys_and_command() {
    let registry = crate::commands::registry();
    let menus = crate::bindings::built_in_menus(&registry);
    let definition = menus.get(menus.lookup("review").unwrap()).unwrap();
    let heads: Vec<(String, &str)> = definition
        .heads()
        .iter()
        .map(|head| {
            let oso_core::menu::HeadAction::Command(id) = head.action else {
                panic!("a review head runs a command");
            };
            (head.keys.to_string(), registry.get(id).unwrap().name)
        })
        .collect();
    let known: Vec<(String, &str)> = crate::commands::review::menu_heads()
        .map(|(keys, command)| (keys.to_owned(), command))
        .collect();
    assert_eq!(heads, known);
    let shown = notes(&in_project(true).0);
    assert_eq!(shown.heads.len(), known.len());
}

#[test]
fn a_long_branch_base_or_project_name_is_cut_in_the_notes() {
    let (mut editor, project) = in_project(true);
    let generation = editor.start_scope_counts(project, &[ScopeKind::Branch]);
    let base = format!("{}\n{}", "b".repeat(300), "x");
    editor.set_branch_base(project, generation, Some(base));
    let shown = notes(&editor);
    let (text, _) = note(&shown, 4);
    assert!(!text.contains('\n'));
    assert!(text.chars().count() < 120, "{text}");
}

#[test]
fn c_c_a_v_asks_the_host_to_continue_and_opens_the_menu_at_once_with_c_u() {
    use oso_core::editor::ReviewRequest;

    let (mut editor, project) = in_project(true);
    let mut d = super::Dispatcher::new();
    // The old key is gone (ADR 0184): the agents menu's `v` is the review key.
    super::motion_tests::send(&mut d, &mut editor, &["C-x v", "r"].join(" "));
    assert_eq!(d.menu_name(), None);
    assert!(
        editor.take_review_requests().is_empty(),
        "the old key is bound"
    );
    // No review open: the host continues the review left last, or opens the menu.
    super::motion_tests::send(&mut d, &mut editor, "C-c a v");
    assert_eq!(d.menu_name(), None);
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Layout { menu: false }]
    );
    // A review open: the same.
    editor.add_review(project, "*review: uncommitted oso*".into());
    super::motion_tests::send(&mut d, &mut editor, "C-c a v");
    assert_eq!(d.menu_name(), None);
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Layout { menu: false }]
    );
    // `C-u` before the menu's key, or inside the menu, opens the review menu at once, so that
    // a macro's next key goes to it.
    super::motion_tests::send(&mut d, &mut editor, "C-u C-c a v");
    assert_eq!(d.menu_name(), Some("review"));
    assert!(editor.take_review_requests().is_empty());
    super::motion_tests::send(&mut d, &mut editor, "C-g");
    super::motion_tests::send(&mut d, &mut editor, "C-c a C-u v");
    assert_eq!(d.menu_name(), Some("review"));
    assert!(editor.take_review_requests().is_empty());
    super::motion_tests::send(&mut d, &mut editor, "C-g");
    // `C-c a V` is the review menu.
    super::motion_tests::send(&mut d, &mut editor, "C-c a V");
    assert_eq!(d.menu_name(), Some("review"));
}

#[test]
fn q_in_a_review_buries_it_and_asks_the_host_to_restore_the_layout() {
    use oso_core::editor::ReviewRequest;

    let (mut editor, project) = in_project(true);
    let (_, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    editor.show(buffer);
    let mut d = super::Dispatcher::new();
    super::motion_tests::send(&mut d, &mut editor, "q");
    assert_ne!(editor.tiled().0.buffer(), buffer);
    assert_eq!(editor.take_review_requests(), [ReviewRequest::Quit]);
}

#[test]
fn an_agents_q_buries_the_review_and_asks_for_no_restore() {
    use oso_core::command::Reach;
    use oso_core::editor::ReviewRequest;

    let (mut editor, project) = in_project(true);
    let (_, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    editor.show(buffer);
    let mut d = super::Dispatcher::new();
    let ran = d.run_for_agent(
        &mut editor,
        "review-quit",
        super::args::Given::Nothing,
        Reach::Change,
    );
    assert_eq!(ran.unwrap().1, Ok(()));
    assert_ne!(editor.tiled().0.buffer(), buffer);
    // The restore would choose where the person's next keys land: the person's `q` only.
    assert!(!editor.take_review_requests().contains(&ReviewRequest::Quit));
    // `review-layout` is the person's alone.
    let refused = d.run_for_agent(
        &mut editor,
        "review-layout",
        super::args::Given::Nothing,
        Reach::Change,
    );
    assert!(refused.is_err());
}

#[test]
fn in_a_space_with_an_orchestrator_the_host_opens_c_c_a_v_s_menu() {
    use oso_core::editor::ReviewRequest;

    let (mut editor, _) = in_project(true);
    let mut d = super::Dispatcher::new();
    d.set_review_menu_by_host(true);
    super::motion_tests::send(&mut d, &mut editor, "C-u C-c a v");
    assert_eq!(d.menu_name(), None);
    assert_eq!(
        editor.take_review_requests(),
        [ReviewRequest::Layout { menu: true }]
    );
    d.set_review_menu_by_host(false);
    super::motion_tests::send(&mut d, &mut editor, "C-u C-c a v");
    assert_eq!(d.menu_name(), Some("review"));
}

/// The notes `review-start` shows now.
#[cfg(unix)]
fn start_notes(editor: &Editor) -> Notes {
    let registry = crate::commands::registry();
    let menus = crate::bindings::built_in_menus(&registry);
    let definition = menus.get(menus.lookup("review-start").unwrap()).unwrap();
    crate::commands::review::review_start_menu(editor, &Open::new(Arc::clone(definition)))
}

#[cfg(unix)]
#[test]
fn review_start_names_the_repository_and_needs_two_worktrees_for_w() {
    use oso_core::complete::Candidate;

    use crate::commands::review::REVIEW_WORKTREES_LIST;

    let (mut editor, project) = in_project(true);
    // Heads: p w .
    let shown = start_notes(&editor);
    assert_eq!(shown.line, None);
    assert!((0..3).all(|head| note(&shown, head).1.is_some()));
    editor.set_review_target(Some(project));
    let shown = start_notes(&editor);
    assert_eq!(
        shown.line.as_deref(),
        Some("Review \u{b7} oso \u{b7} ~/src/oso")
    );
    assert!(shown.get(0).is_none());
    assert_eq!(note(&shown, 1).1, Some("one worktree: . reviews it"));
    let two: Arc<[Candidate]> = ["/home/me/src/oso", "/home/me/src/oso-wt"]
        .iter()
        .map(|path| Candidate::new(*path, "", ""))
        .collect();
    editor.set_listed(REVIEW_WORKTREES_LIST, two);
    assert!(start_notes(&editor).get(1).is_none());
}
