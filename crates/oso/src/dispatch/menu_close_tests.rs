//! Scenario tests of what ends an open menu, and of `RET`, through the
//! [`Dispatcher`](super::Dispatcher) (ADR 0117): a menu's key during a search, a jump or the
//! layer of `C-x TAB`, the window's close button, and a default head bound to `RET`.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::Editor;
use oso_core::menu::{Definition, ForeignKeys, Menus, Stay};

use super::Dispatcher;
use super::menu_tests::{command, dispatcher, opening, with_menus};
use super::motion_tests::{at, cursors, send};

#[test]
fn a_menus_key_ends_a_search_or_the_rigid_layer_and_opens_but_a_jump_ends_alone() {
    // isearch keeps the match it reached, as any key it does not bind leaves it.
    let (mut d, mut editor) = with_menus("abcabc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-s b C-c m");
    assert_eq!(d.menu_name(), Some("test"));
    assert!(editor.isearch().is_none());
    assert_eq!(cursors(&editor), [2]);
    send(&mut d, &mut editor, "f");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![3], Some("test")));
    let (mut d, mut editor) = with_menus("abcabc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-x TAB C-c m");
    assert!(!editor.indent_rigidly_layer());
    assert_eq!(d.menu_name(), Some("test"));
    // A key a jump's keymap does not bind ends it and does not run (ADR 0116).
    let (mut d, mut editor) = with_menus("abcabc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "M-j a RET C-c");
    assert!(editor.jump().is_none());
    assert_eq!(d.menu_name(), None);
}

/// A menu with a head on `y`, over a modified buffer.
fn on_y(foreign: ForeignKeys) -> (Dispatcher, Editor) {
    let (mut d, mut editor) = with_menus("abc", 0, foreign);
    send(&mut d, &mut editor, "x");
    let mut menus = Menus::new();
    let id = menus.id("yes").unwrap();
    let mut menu = Definition::new("yes", "Yes").unwrap();
    menu.add_group(None, 1).unwrap();
    menu.add_head(command(&d, "y", "forward-char", Some(Stay::Stay)))
        .unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c y", id));
    send(&mut d, &mut editor, "C-c y");
    assert_eq!(d.menu_name(), Some("yes"));
    (d, editor)
}

#[test]
fn the_close_buttons_question_closes_the_menu_and_takes_its_keys() {
    let (mut d, mut editor) = on_y(ForeignKeys::Warn);
    assert!(!d.close_requested(&mut editor).exit);
    assert_eq!(d.menu_name(), None);
    assert!(
        editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
    );
    assert!(send(&mut d, &mut editor, "y").exit, "the question's y");
    let (mut d, mut editor) = on_y(ForeignKeys::Warn);
    d.close_requested(&mut editor);
    send(&mut d, &mut editor, "C-g");
    assert!(editor.prompt().is_none(), "C-g ends the question");
    assert_eq!(d.menu_name(), None);
}

#[test]
fn ret_runs_and_marks_a_default_head_on_ret() {
    let (_, mut editor) = at("abcdef", &[0]);
    let mut d = dispatcher();
    let mut menus = Menus::new();
    let id = menus.id("ret").unwrap();
    let mut menu = Definition::new("ret", "Ret").unwrap();
    menu.add_group(None, 1).unwrap();
    let on_ret = menu
        .add_head(command(&d, "RET", "forward-char", None))
        .unwrap();
    menu.set_ret_head(on_ret).unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c r", id));
    send(&mut d, &mut editor, "C-c r");
    let mut notes = super::Described::default();
    let menu = d.menu().unwrap();
    menu.notes_into(&editor, &mut notes);
    let definition = menu.open().definition();
    assert_eq!(notes.marked_head(definition), Some(on_ret));
    send(&mut d, &mut editor, "RET");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], None));
}
