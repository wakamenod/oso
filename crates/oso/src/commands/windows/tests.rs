//! The window commands through the registry: what each changes, what it says when it cannot,
//! and that none runs inside a prompt (ADR 0112).
//!
//! Must not depend on: `app`, winit or the GPU.

use oso_core::buffer::Buffer;
use oso_core::command::{Args, Ctx, Effects, ExecuteError};
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::minibuffer::Kind;
use oso_core::window::WindowId;

fn editor() -> Editor {
    let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut editor = Editor::with_buffer(Buffer::from_text("test", &text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 81,
    });
    editor
}

/// Runs the window command `name` through a registry that has them, as the app's will from
/// ADR 0112's step 3.
fn run(editor: &mut Editor, name: &str, args: Args) -> (Result<(), String>, Effects) {
    let mut registry = crate::commands::registry();
    registry.register_all(super::COMMANDS).unwrap();
    let id = registry.lookup(name).unwrap();
    let mut ctx = Ctx::new(editor);
    let result = registry.execute(id, &mut ctx, args).map_err(|e| match e {
        ExecuteError::Failed { error, .. } => error.message().to_owned(),
        other => panic!("{other}"),
    });
    (result, ctx.effects())
}

/// Runs `name`, which must work.
fn ok(editor: &mut Editor, name: &str, args: Args) {
    assert_eq!(run(editor, name, args).0, Ok(()), "{name}");
}

fn ids(editor: &Editor) -> Vec<WindowId> {
    editor.frame().windows().iter().map(|(id, _)| *id).collect()
}

fn selected(editor: &Editor) -> WindowId {
    editor.window().id()
}

#[test]
fn the_splits_keep_or_select_the_new_window() {
    let mut editor = editor();
    for (name, select) in [
        ("split-window-below", false),
        ("split-window-right", false),
        ("split-window-below-and-select", true),
        ("split-window-right-and-select", true),
    ] {
        editor.delete_other_windows();
        editor.select_window(ids(&editor)[0]);
        let before = selected(&editor);
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Ok(()), "{name}");
        assert!(effects.redraw, "{name}");
        assert_eq!(ids(&editor).len(), 2, "{name}");
        assert_eq!(selected(&editor) != before, select, "{name}");
    }
}

#[test]
fn a_split_too_small_says_so() {
    let mut editor = editor();
    editor.set_frame_size(FrameSize {
        rows: 6,
        columns: 81,
    });
    let (result, effects) = run(&mut editor, "split-window-below", Args::NONE);
    assert_eq!(result, Err("Window too small for splitting".to_owned()));
    assert!(!effects.redraw);
    assert_eq!(ids(&editor).len(), 1);
}

#[test]
fn deleting_the_sole_window_is_refused_and_deleting_the_others_of_it_does_nothing() {
    let mut editor = editor();
    let (result, _) = run(&mut editor, "delete-window", Args::NONE);
    assert_eq!(result, Err("Attempt to delete the sole window".to_owned()));
    let (result, effects) = run(&mut editor, "delete-other-windows", Args::NONE);
    assert_eq!(result, Ok(()));
    assert!(!effects.redraw);

    ok(&mut editor, "split-window-below", Args::NONE);
    ok(&mut editor, "split-window-right", Args::NONE);
    let (result, effects) = run(&mut editor, "delete-window", Args::NONE);
    assert_eq!((result, effects.redraw), (Ok(()), true));
    assert_eq!(ids(&editor).len(), 2);
    let (result, effects) = run(&mut editor, "delete-other-windows", Args::NONE);
    assert_eq!((result, effects.redraw), (Ok(()), true));
    assert_eq!(ids(&editor).len(), 1);
}

#[test]
fn other_window_takes_a_count_and_redraws_only_when_it_moves() {
    let mut editor = editor();
    let (_, effects) = run(&mut editor, "other-window", Args::NONE);
    assert!(!effects.redraw, "one window");
    ok(&mut editor, "split-window-below", Args::NONE);
    ok(&mut editor, "split-window-below", Args::NONE);
    let order = ids(&editor);
    let (_, effects) = run(&mut editor, "other-window", Args::NONE);
    assert!(effects.redraw);
    assert_eq!(selected(&editor), order[1]);
    ok(&mut editor, "other-window", Args::numeric(-2));
    assert_eq!(selected(&editor), order[2]);
    let (_, effects) = run(&mut editor, "other-window", Args::numeric(3));
    assert!(!effects.redraw, "all the way around");
    for count in [isize::MIN, isize::MAX, 0] {
        assert_eq!(
            run(&mut editor, "other-window", Args::numeric(count)).0,
            Ok(())
        );
    }
}

#[test]
fn windmove_says_when_no_window_is_there() {
    let mut editor = editor();
    ok(&mut editor, "split-window-right", Args::NONE);
    let order = ids(&editor);
    for (name, message) in [
        ("windmove-left", "No window left from selected window"),
        ("windmove-up", "No window up from selected window"),
        ("windmove-down", "No window down from selected window"),
    ] {
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Err(message.to_owned()));
        assert!(!effects.redraw);
    }
    let (result, effects) = run(&mut editor, "windmove-right", Args::NONE);
    assert_eq!((result, effects.redraw), (Ok(()), true));
    assert_eq!(selected(&editor), order[1]);
    assert_eq!(
        run(&mut editor, "windmove-right", Args::NONE).0,
        Err("No window right from selected window".to_owned())
    );
}

#[test]
fn resizing_moves_by_the_count_and_says_when_nothing_can() {
    let mut editor = editor();
    let rows = |editor: &Editor| editor.tiled_regions().text.rows;
    let columns = |editor: &Editor| editor.tiled_regions().text.columns;
    let (result, _) = run(&mut editor, "enlarge-window", Args::NONE);
    assert_eq!(result, Err("Cannot enlarge the window".to_owned()));
    let (result, _) = run(&mut editor, "shrink-window-horizontally", Args::NONE);
    assert_eq!(result, Err("Cannot shrink the window".to_owned()));
    ok(&mut editor, "split-window-below", Args::NONE);
    ok(&mut editor, "split-window-right", Args::NONE);
    let (before_rows, before_columns) = (rows(&editor), columns(&editor));
    let (result, effects) = run(&mut editor, "enlarge-window", Args::numeric(3));
    assert_eq!((result, effects.redraw), (Ok(()), true));
    assert_eq!(rows(&editor), before_rows + 3);
    ok(&mut editor, "shrink-window", Args::numeric(2));
    assert_eq!(rows(&editor), before_rows + 1);
    ok(&mut editor, "shrink-window", Args::numeric(-1));
    assert_eq!(rows(&editor), before_rows + 2, "a negative count enlarges");
    ok(&mut editor, "enlarge-window-horizontally", Args::numeric(4));
    assert_eq!(columns(&editor), before_columns + 4);
    ok(&mut editor, "shrink-window-horizontally", Args::numeric(5));
    assert_eq!(columns(&editor), before_columns - 1);
    let (result, effects) = run(&mut editor, "enlarge-window", Args::numeric(0));
    assert_eq!((result, effects.redraw), (Ok(()), false));
    for count in [isize::MIN, isize::MAX] {
        let _ = run(&mut editor, "enlarge-window", Args::numeric(count));
        let _ = run(
            &mut editor,
            "shrink-window-horizontally",
            Args::numeric(count),
        );
    }
    let (result, effects) = run(&mut editor, "balance-windows", Args::NONE);
    assert_eq!((result, effects.redraw), (Ok(()), true));
    let (_, effects) = run(&mut editor, "balance-windows", Args::NONE);
    assert!(!effects.redraw, "already balanced");
}

#[test]
fn no_window_command_runs_inside_a_prompt() {
    let mut editor = editor();
    ok(&mut editor, "split-window-below", Args::NONE);
    editor
        .open_prompt("Name: ", Kind::Text { history: None })
        .unwrap();
    let before = ids(&editor);
    for name in [
        "split-window-below",
        "split-window-right",
        "split-window-below-and-select",
        "split-window-right-and-select",
        "delete-window",
        "delete-other-windows",
        "other-window",
        "balance-windows",
        "enlarge-window",
        "shrink-window",
        "enlarge-window-horizontally",
        "shrink-window-horizontally",
        "windmove-left",
        "windmove-right",
        "windmove-up",
        "windmove-down",
    ] {
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Err("Not in the minibuffer".to_owned()), "{name}");
        assert!(!effects.redraw, "{name}");
    }
    assert_eq!(ids(&editor), before);
}
