//! Scenario tests of `consult-line` through the [`Dispatcher`] (ADR 0154): `M-s l` and the search
//! menu's `l` open it, what is typed searches, `RET` goes to the match with the place before on
//! the mark ring, `C-g` leaves the cursor, and it refuses inside a prompt and in a terminal.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window. The search runs here, as the host's thread would.

use oso_core::editor::{Editor, LinesEnd};

use super::Dispatcher;
use super::motion_tests::{at, send};
use crate::commands::kept_messages;

/// Runs the search the prompt asks for and takes in all it found.
fn search(editor: &mut Editor) {
    let request = editor.take_line_search_request().expect("a search");
    let mut rows = Vec::new();
    let LinesEnd::Ended(end) = request.run(&mut |row| {
        rows.extend(row);
        true
    }) else {
        panic!("stopped");
    };
    editor.lines_found(request.generation, rows, Some(end));
}

fn shown(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

fn opened(keys: &str) -> (Dispatcher, Editor) {
    let (mut d, mut editor) = at("alpha\nbeta\ngamma beta\n", &[2]);
    send(&mut d, &mut editor, keys);
    assert_eq!(editor.prompt().unwrap().label(), "Go to line:");
    search(&mut editor);
    (d, editor)
}

#[test]
fn m_s_l_lists_the_lines_and_ret_goes_to_the_match() {
    let (mut d, mut editor) = opened("M-s l");
    assert_eq!(shown(&editor), ["1: alpha", "2: beta", "3: gamma beta"]);
    send(&mut d, &mut editor, "b e t a");
    search(&mut editor);
    assert_eq!(shown(&editor), ["2: beta", "3: gamma beta"]);
    send(&mut d, &mut editor, "C-n RET");
    assert!(editor.prompt().is_none());
    let primary = editor.window().selections.primary();
    assert_eq!((primary.cursor, primary.mark), (17, Some(2)));
}

#[test]
fn the_search_menus_l_opens_it_and_c_g_leaves_the_cursor() {
    let (mut d, mut editor) = at("alpha\nbeta\n", &[3]);
    let _ = d.open_menu_named(&mut editor, "search");
    send(&mut d, &mut editor, "l");
    assert_eq!(d.menu_name(), None);
    assert_eq!(editor.prompt().unwrap().label(), "Go to line:");
    search(&mut editor);
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none());
    assert_eq!(editor.window().selections.primary().cursor, 3);
}

#[test]
fn it_refuses_inside_a_prompt() {
    let (mut d, mut editor) = opened("M-s l");
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, "M-s l");
    assert!(editor.prompt().unwrap().label().ends_with("M-x"));
    assert!(editor.take_line_search_request().is_none());
}

#[test]
fn a_terminal_is_searched_in_copy_mode() {
    let (mut d, mut editor) = at("", &[0]);
    let id = editor.add_terminal_buffer("*terminal*");
    editor.show(id);
    // A terminal's keys go to its program, so it is reached by name.
    send(&mut d, &mut editor, "M-x");
    send(&mut d, &mut editor, "c o n s u l t - l i n e RET");
    assert!(editor.prompt().is_none());
    assert_eq!(
        kept_messages(&editor).pop().as_deref(),
        Some("A terminal's lines are searched in copy mode, C-c C-j")
    );
}
