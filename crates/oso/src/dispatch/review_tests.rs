//! The review menu's notes as the built-in menu draws them (ADR 0117, ADR 0122): each count in one
//! width, the last scope, the base, the line under the title, and why a head cannot run.
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
    assert!(line.ends_with("comments go to the kill ring until sending exists"));

    let (editor, _) = in_project(false);
    let shown = notes(&editor);
    for head in 0..8 {
        assert_eq!(
            note(&shown, head).1,
            Some("not trusted to run git: M-x trust-project"),
            "{head}"
        );
    }
    let editor = Editor::default();
    let shown = notes(&editor);
    assert_eq!(note(&shown, 1).1, Some("not in a git repository"));
    assert_eq!(shown.line, None);
}
