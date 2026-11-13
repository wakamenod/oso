//! Scenario tests of keys bound to menus through the [`Dispatcher`](super::Dispatcher), before
//! menus open (ADR 0117 step 1): what such a key says, and that the menus go in with the user's
//! keymap that names them.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::editor::{Editor, Status};
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;
use oso_core::menu::{Definition, MenuId, Menus};

use super::motion_tests::{at, cursors, send};

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// A set of one menu, `name`, if `defined`, and its id.
fn one_menu(name: &str, defined: bool) -> (Menus, MenuId) {
    let mut menus = Menus::new();
    let id = menus.id(name).unwrap();
    if defined {
        menus
            .define(id, Definition::new(name, "T").unwrap())
            .unwrap();
    }
    (menus, id)
}

/// A user keymap binding `keys` to `menu`.
fn opening(keys: &str, menu: MenuId) -> Keymap {
    let mut keymap = Keymap::new();
    let keys: KeySequence = keys.parse().unwrap();
    keymap.bind_menu(&keys.0, menu).unwrap();
    keymap
}

#[test]
fn a_key_bound_to_a_menu_says_menus_cannot_open_yet() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let (menus, toggles) = one_menu("toggles", true);
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c t", toggles));
    send(&mut d, &mut editor, "C-u C-c t");
    assert_eq!(echo(&editor), "Menus cannot open yet");
    // It ends the prefix argument, as an undefined key does.
    send(&mut d, &mut editor, "C-f");
    assert_eq!(cursors(&editor), [4]);
}

#[test]
fn a_key_whose_menu_is_not_in_force_is_undefined() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let (menus, gone) = one_menu("gone", false);
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c g", gone));
    send(&mut d, &mut editor, "C-c g");
    assert_eq!(echo(&editor), "That key is undefined");
}

#[test]
fn menus_wait_for_a_pending_prefix_with_the_keymap_and_none_puts_back_the_built_in_ones() {
    let (mut d, mut editor) = at("abcdef", &[3]);
    let builtin = d.menus().len();
    let (first, _) = one_menu("first", true);
    assert!(d.set_menus(Some(first)).is_some());
    assert!(d.menus().lookup("first").is_some());
    send(&mut d, &mut editor, "C-x");
    let (second, _) = one_menu("second", true);
    assert!(d.set_menus(Some(second)).is_none());
    assert!(
        d.menus().lookup("first").is_some(),
        "the prefix is still pending"
    );
    send(&mut d, &mut editor, "C-g");
    assert!(d.menus().lookup("second").is_some());
    let replaced = d.take_replaced_menus().unwrap();
    assert!(replaced.lookup("first").is_some());
    assert!(d.take_replaced_menus().is_none());
    d.set_menus(None);
    assert_eq!(d.menus().len(), builtin);
}
