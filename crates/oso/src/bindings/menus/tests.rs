//! Tests of the built-in menus (ADR 0117): every head of their tables is there with the default
//! commands, their ids are stable, `C-c t` opens the toggles, the toggling heads show a state,
//! and the theme's note keeps its width.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data.

use std::sync::Arc;

use oso_core::command::Registry;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::keymap::Binding;
use oso_core::menu::{Definition, HeadAction, Open, Stay};
use oso_core::minibuffer::cells_of;

use super::{BUILT_IN, MENU_KEYS, THEME_LABEL, THEME_NOTE_CELLS, built_in_menus, describe_toggles};
use crate::{bindings, commands};

fn built_in<'m>(menus: &'m oso_core::menu::Menus, name: &str) -> &'m Arc<Definition> {
    menus.get(menus.lookup(name).unwrap()).unwrap()
}

#[test]
fn every_head_of_the_tables_is_there_with_the_default_commands() {
    let registry = commands::registry();
    let menus = built_in_menus(&registry);
    assert_eq!(menus.len(), BUILT_IN.len());
    for menu in BUILT_IN {
        let definition = built_in(&menus, menu.name);
        assert_eq!(definition.title(), menu.title);
        let heads: Vec<_> = menu.groups.iter().flat_map(|(_, heads)| *heads).collect();
        assert_eq!(definition.heads().len(), heads.len(), "{}", menu.name);
        assert_eq!(definition.groups().len(), menu.groups.len());
        for (head, want) in definition.heads().iter().zip(heads) {
            let keys: KeySequence = want.keys.parse().unwrap();
            assert_eq!(head.keys, keys);
            assert_eq!(head.label, want.label);
            let HeadAction::Command(id) = head.action else {
                panic!("{} is not a command head", want.keys);
            };
            assert_eq!(registry.get(id).unwrap().name, want.command);
            assert_eq!(head.stay == Some(Stay::Stay), want.stay);
            assert_eq!(head.count, want.count);
        }
    }
}

#[test]
fn their_ids_follow_the_tables_whatever_the_registry_has() {
    let full = built_in_menus(&commands::registry());
    // A registry without their commands, as some tests bring, keeps the menus and their ids,
    // with no heads.
    let empty = built_in_menus(&Registry::new());
    for (index, menu) in BUILT_IN.iter().enumerate() {
        let id = full.lookup(menu.name).unwrap();
        assert_eq!(empty.lookup(menu.name), Some(id));
        assert_eq!(full.iter().nth(index).map(|(id, _)| id), Some(id));
        assert!(built_in(&empty, menu.name).heads().is_empty());
    }
}

#[test]
fn the_default_keys_open_their_built_in_menus() {
    let registry = commands::registry();
    let keymap = bindings::global_keymap(&registry);
    let menus = built_in_menus(&registry);
    for (keys, name) in MENU_KEYS {
        let keys: KeySequence = keys.parse().unwrap();
        assert_eq!(
            keymap.lookup(&keys.0),
            Some(Binding::Menu(menus.lookup(name).unwrap()))
        );
    }
    let mut bound = Vec::new();
    keymap.menu_bindings(|keys, id| bound.push((keys.to_vec(), id)));
    assert_eq!(bound.len(), MENU_KEYS.len(), "only the toggles have a key");
}

#[test]
fn the_toggling_heads_show_a_state_and_the_others_none() {
    let registry = commands::registry();
    let menus = built_in_menus(&registry);
    let without = ["delete-trailing-whitespace", "toggle-theme"];
    for head in built_in(&menus, "toggles").heads() {
        let HeadAction::Command(id) = head.action else {
            unreachable!()
        };
        let command = registry.get(id).unwrap();
        assert_eq!(
            command.state.is_some(),
            !without.contains(&command.name),
            "{}",
            command.name
        );
    }
}

#[test]
fn the_theme_note_names_the_theme_in_the_same_width_for_every_theme() {
    let themes = oso_render::theme::themes();
    let names: Vec<(&'static str, &'static str)> =
        themes.iter().map(|theme| (theme.name(), "dark")).collect();
    let longest = names.iter().map(|(name, _)| cells_of(name)).max();
    assert_eq!(longest, Some(THEME_NOTE_CELLS));
    let menus = built_in_menus(&commands::registry());
    let toggles = built_in(&menus, "toggles");
    let theme_head = toggles
        .heads()
        .iter()
        .position(|head| head.label == THEME_LABEL)
        .unwrap();
    let open = Open::new(Arc::clone(toggles));
    for index in 0..names.len() {
        let mut editor = Editor::default();
        editor.set_themes(names.clone(), index);
        let notes = describe_toggles(&editor, &open);
        assert_eq!(notes.heads.len(), 1);
        let note = notes.get(theme_head).unwrap();
        assert_eq!(note.text.trim_end(), names[index].0);
        assert_eq!(cells_of(&note.text), THEME_NOTE_CELLS);
        assert!(note.unavailable.is_none() && !note.hidden);
    }
    // With no themes there is nothing to say.
    assert!(describe_toggles(&Editor::default(), &open).heads.is_empty());
}

#[test]
fn no_head_names_a_dev_command() {
    // Dev commands are named `dev-…` (`commands/dev_tools.rs`), and a menu a release build
    // ships must not differ in a dev build.
    for menu in BUILT_IN {
        for (_, heads) in menu.groups {
            for head in *heads {
                assert!(!head.command.starts_with("dev-"), "{}", head.command);
            }
        }
    }
}
