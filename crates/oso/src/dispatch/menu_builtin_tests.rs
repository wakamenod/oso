//! Scenario tests of the built-in menus through the [`Dispatcher`](super::Dispatcher) (ADR 0117,
//! step 3): `C-c t` opens the toggles, whose heads show their state and close it, the windows'
//! resize heads stay and move by 3, and a removed menu's key is undefined.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;
use oso_core::window::ViewSize;

use super::Dispatcher;
use super::menu_tests::echo;
use super::motion_tests::{at, send};

/// The check of the open menu's head labelled `label`.
fn check(d: &Dispatcher, editor: &Editor, label: &str) -> Option<bool> {
    let menu = d.menu()?;
    let head = menu
        .open()
        .definition()
        .heads()
        .iter()
        .position(|head| head.label == label)?;
    let mut checks = Vec::new();
    menu.checks_into(editor, &mut checks);
    checks[head]
}

#[test]
fn c_c_t_opens_the_toggles_whose_heads_show_their_state_and_close_it() {
    let (mut d, mut editor) = at("abc", &[0]);
    assert!(send(&mut d, &mut editor, "C-c t").redraw);
    assert_eq!(d.menu_name(), Some("toggles"));
    assert_eq!(check(&d, &editor, "whitespace"), Some(true));
    assert_eq!(check(&d, &editor, "line numbers"), Some(false));
    assert_eq!(check(&d, &editor, "truncate lines"), Some(true));
    assert_eq!(check(&d, &editor, "whitespace cleanup"), None);
    send(&mut d, &mut editor, "w");
    assert_eq!(d.menu_name(), None, "a toggle closes the menu");
    assert!(!editor.visual_aids().whitespace);
    send(&mut d, &mut editor, "C-c t");
    assert_eq!(check(&d, &editor, "whitespace"), Some(false));
    send(&mut d, &mut editor, "n");
    assert!(editor.visual_aids().line_numbers);
    send(&mut d, &mut editor, "C-c t S");
    assert!(!editor.font_lock());
}

#[test]
fn the_windows_resize_heads_stay_and_move_by_three() {
    let (mut d, mut editor) = at("abc", &[0]);
    editor.set_view_size(ViewSize::new(20, 80));
    let full = editor.window().size().columns;
    send(&mut d, &mut editor, "C-x 3");
    let before = editor.window().size().columns;
    let _ = d.open_menu_named(&mut editor, "windows");
    send(&mut d, &mut editor, "<left>");
    assert_eq!(editor.window().size().columns, before - 3);
    assert_eq!(d.menu_name(), Some("windows"));
    send(&mut d, &mut editor, "<right> <right>");
    assert_eq!(editor.window().size().columns, before + 3);
    // An exit head closes it.
    send(&mut d, &mut editor, "O");
    assert_eq!(d.menu_name(), None);
    assert_eq!(editor.window().size().columns, full);
}

#[test]
fn a_removed_menus_default_key_is_undefined() {
    let (mut d, mut editor) = at("abc", &[0]);
    let mut menus = d.menus_in_force().clone();
    menus.remove(menus.lookup("toggles").unwrap());
    d.set_menus(Some(menus));
    // As the configuration unbinds it.
    let mut user = Keymap::new();
    user.unbind(&"C-c t".parse::<KeySequence>().unwrap().0)
        .unwrap();
    d.set_user_keymap(user);
    send(&mut d, &mut editor, "C-c t");
    assert_eq!(d.menu_name(), None);
    assert_eq!(echo(&editor), "C-c t is undefined");
    // Shift does not find the removed menu's key, and the rest of `C-c` is the defaults'.
    send(&mut d, &mut editor, "C-c T");
    assert_eq!(d.menu_name(), None);
    assert_eq!(echo(&editor), "C-c T is undefined");
    send(&mut d, &mut editor, "C-c C-<");
    assert_ne!(echo(&editor), "C-c C-< is undefined");
}
