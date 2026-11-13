//! Tests of the menu model (ADR 0117): definitions and their checks, key lookup, the set of
//! menus, open menus' switches and notes.
//!
//! Must not depend on: `keymap`; menus are tested as data.

use std::sync::Arc;

use super::{
    Definition, ForeignKeys, Head, HeadAction, Lookup, MAX_HEADS, MAX_MENUS, MenuError, Menus,
    Note, Notes, Open, Stay,
};
use crate::command::{ArgSpec, Args, Command, CommandError, CommandId, Ctx, Reach, Registry};
use crate::key::KeySequence;

fn noop(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    Ok(())
}

/// A command id from a registry of one command.
fn command() -> CommandId {
    let mut registry = Registry::new();
    registry
        .register(Command {
            name: "noop",
            doc: "Does nothing.",
            args: ArgSpec::None,
            reach: Reach::Look,
            run: noop,
            state: None,
        })
        .unwrap()
}

fn keys(spelled: &str) -> KeySequence {
    spelled.parse().unwrap()
}

fn head(spelled: &str, action: HeadAction) -> Head {
    Head {
        keys: keys(spelled),
        label: format!("label of {spelled}"),
        action,
        stay: None,
        count: None,
    }
}

fn run(spelled: &str) -> Head {
    head(spelled, HeadAction::Command(command()))
}

fn menu() -> Definition {
    let mut menu = Definition::new("windows", "Windows").unwrap();
    menu.add_group(Some("Size"), 1).unwrap();
    menu
}

#[test]
fn a_menu_is_named_in_kebab_case_and_titled_in_64_cells() {
    assert!(Definition::new("windows-and-buffers", "Windows").is_ok());
    for bad in [
        "",
        "Windows",
        "a--b",
        "-a",
        "a-",
        "a_b",
        "日本",
        &"a".repeat(65),
    ] {
        assert_eq!(
            Definition::new(bad, "T").unwrap_err(),
            MenuError::BadName,
            "{bad:?}"
        );
    }
    assert!(Definition::new("w", &"x".repeat(64)).is_ok());
    assert_eq!(
        Definition::new("w", &"x".repeat(65)).unwrap_err(),
        MenuError::LongTitle
    );
    // Cells, not bytes or characters: a wide character takes two.
    assert_eq!(
        Definition::new("w", &"窓".repeat(33)).unwrap_err(),
        MenuError::LongTitle
    );
}

#[test]
fn a_head_needs_a_group_and_goes_in_the_last() {
    let mut menu = Definition::new("w", "W").unwrap();
    assert_eq!(menu.add_head(run("a")), Err(MenuError::NoGroup));
    menu.add_group(None, 1).unwrap();
    assert_eq!(menu.add_head(run("a")), Ok(0));
    menu.add_group(Some("Two"), 2).unwrap();
    assert_eq!(menu.add_head(run("b")), Ok(1));
    assert_eq!(menu.add_head(run("c")), Ok(2));
    let groups = menu.groups();
    assert_eq!(
        (groups[0].title(), groups[0].row(), groups[0].heads()),
        (None, 1, 0..1)
    );
    assert_eq!(
        (groups[1].title(), groups[1].row(), groups[1].heads()),
        (Some("Two"), 2, 1..3)
    );
    assert_eq!(menu.heads().len(), 3);
}

#[test]
fn groups_take_rows_1_to_3_of_at_most_8_columns() {
    let mut menu = Definition::new("w", "W").unwrap();
    assert_eq!(menu.add_group(None, 0), Err(MenuError::BadRow));
    assert_eq!(menu.add_group(None, 4), Err(MenuError::BadRow));
    for _ in 0..8 {
        menu.add_group(None, 1).unwrap();
    }
    assert_eq!(menu.add_group(None, 1), Err(MenuError::TooManyColumns));
    assert!(menu.add_group(None, 3).is_ok());
    assert_eq!(
        menu.add_group(Some(&"x".repeat(65)), 2),
        Err(MenuError::LongTitle)
    );
}

#[test]
fn a_head_has_one_to_three_keys_and_never_cancel_or_a_prefix_argument() {
    let mut menu = menu();
    let empty = Head {
        keys: KeySequence(Vec::new()),
        ..run("a")
    };
    assert_eq!(menu.add_head(empty), Err(MenuError::NoKeys));
    assert_eq!(menu.add_head(run("a b c d")), Err(MenuError::TooManyKeys));
    assert!(menu.add_head(run("a b c")).is_ok());
    for cancel in ["C-g", "x C-g", "C-G"] {
        assert_eq!(
            menu.add_head(run(cancel)),
            Err(MenuError::Cancel),
            "{cancel}"
        );
    }
    for reserved in ["C-u", "M-5", "M-0", "M--", "x C-u"] {
        assert_eq!(
            menu.add_head(run(reserved)),
            Err(MenuError::ReservedKey),
            "{reserved}"
        );
    }
    // Plain digits and `-` are ordinary keys in a menu (Q15), as is `M-u`.
    for plain in ["5", "-", "M-u", "C-5"] {
        assert!(menu.add_head(run(plain)).is_ok(), "{plain}");
    }
}

#[test]
fn keys_used_twice_or_beginning_another_heads_are_refused() {
    let mut menu = menu();
    menu.add_head(run("Z Z")).unwrap();
    assert_eq!(menu.add_head(run("Z Z")), Err(MenuError::KeyUsedTwice));
    assert_eq!(menu.add_head(run("Z")), Err(MenuError::KeyIsPrefix));
    assert_eq!(menu.add_head(run("Z Z Z")), Err(MenuError::KeyIsPrefix));
    assert!(menu.add_head(run("Z A")).is_ok());
    // A refused head changes nothing.
    assert_eq!(menu.heads().len(), 2);
}

#[test]
fn labels_counts_and_switches_have_limits() {
    let mut menu = menu();
    let long = Head {
        label: "x".repeat(65),
        ..run("a")
    };
    assert_eq!(menu.add_head(long), Err(MenuError::LongLabel));
    for (count, ok) in [
        (1000, true),
        (-1000, true),
        (0, true),
        (1001, false),
        (-1001, false),
    ] {
        let head = Head {
            count: Some(count),
            ..run(&format!("c {}", menu.heads().len()))
        };
        assert_eq!(menu.add_head(head).is_ok(), ok, "{count}");
    }
    let switch = |spelled: &str, name: &str| head(spelled, HeadAction::Switch(name.to_owned()));
    assert_eq!(
        menu.add_head(switch("- f", "Files")),
        Err(MenuError::BadSwitchName)
    );
    assert!(menu.add_head(switch("- f", "files")).is_ok());
    assert_eq!(
        menu.add_head(switch("- g", "files")),
        Err(MenuError::SwitchUsedTwice)
    );
}

#[test]
fn a_menu_has_at_most_128_heads() {
    let mut menu = menu();
    for i in 0..MAX_HEADS {
        let letter = |n: usize| char::from(b'a' + u8::try_from(n % 26).unwrap());
        menu.add_head(run(&format!("{} {}", letter(i / 26), letter(i))))
            .unwrap();
    }
    assert_eq!(menu.add_head(run("z")), Err(MenuError::TooManyHeads));
}

#[test]
fn heads_exit_by_default_and_a_menu_or_a_head_may_say_stay() {
    let mut menu = menu();
    let stays = Head {
        stay: Some(Stay::Stay),
        ..run("a")
    };
    menu.add_head(stays).unwrap();
    menu.add_head(run("b")).unwrap();
    menu.add_head(head("- f", HeadAction::Switch("files".to_owned())))
        .unwrap();
    menu.add_head(head("x", HeadAction::Quit)).unwrap();
    let mut menus = Menus::new();
    let other = menus.id("other").unwrap();
    menu.add_head(head("o", HeadAction::Menu(other))).unwrap();
    assert_eq!(
        menu.stay(4),
        Stay::Stay,
        "a submenu is pushed over this one"
    );
    assert_eq!(
        [0, 1, 2, 3, 99].map(|i| menu.stay(i)),
        [Stay::Stay, Stay::Exit, Stay::Stay, Stay::Exit, Stay::Exit]
    );
    menu.set_default_head(Stay::Stay);
    let exits = Head {
        stay: Some(Stay::Exit),
        ..run("c")
    };
    menu.add_head(exits).unwrap();
    assert_eq!(
        [0, 1, 5].map(|i| menu.stay(i)),
        [Stay::Stay, Stay::Stay, Stay::Exit]
    );
    assert_eq!(menu.default_head(), Stay::Stay);
    assert_eq!(menu.foreign_keys(), ForeignKeys::Warn);
    menu.set_foreign_keys(ForeignKeys::Run);
    assert_eq!(menu.foreign_keys(), ForeignKeys::Run);
}

#[test]
fn keys_look_up_heads_sequences_and_q() {
    let mut menu = menu();
    menu.add_head(run("Z Z")).unwrap();
    menu.add_head(run("<left>")).unwrap();
    let typed = |spelled: &str| menu.lookup(&keys(spelled).0);
    assert_eq!(typed("Z Z"), Lookup::Head(0));
    assert_eq!(typed("Z"), Lookup::Pending);
    assert_eq!(typed("<left>"), Lookup::Head(1));
    assert_eq!(typed("Z x"), Lookup::Unbound);
    assert_eq!(typed("x"), Lookup::Unbound);
    assert_eq!(typed("q"), Lookup::Quit);
    assert_eq!(typed("Z q"), Lookup::Unbound);
    assert_eq!(menu.lookup(&[]), Lookup::Unbound);
    assert!(menu.quits_on_q());
    // A head that binds `q`, or begins with it, takes it over.
    menu.add_head(run("q w")).unwrap();
    assert!(!menu.quits_on_q());
    assert_eq!(menu.lookup(&keys("q").0), Lookup::Pending);
}

#[test]
fn a_menu_names_one_default_head() {
    let mut menu = menu();
    menu.add_head(run("a")).unwrap();
    menu.add_head(run("b")).unwrap();
    assert_eq!(menu.default(), None);
    assert_eq!(menu.set_default(2), Err(MenuError::NoSuchHead));
    assert_eq!(menu.set_default(1), Ok(()));
    assert_eq!(menu.set_default(0), Err(MenuError::DefaultTwice));
    assert_eq!(menu.default(), Some(1));
}

#[test]
fn a_describe_function_is_kept_and_debug_leaves_it_out() {
    fn describe(_: &crate::editor::Editor, open: &Open) -> Notes {
        Notes {
            line: Some(open.definition().name().to_owned()),
            ..Notes::default()
        }
    }
    let mut menu = menu();
    assert!(menu.describe().is_none());
    menu.set_describe(describe);
    let editor = crate::editor::Editor::default();
    let open = Open::new(Arc::new(menu.clone()));
    let notes = (menu.describe().unwrap())(&editor, &open);
    assert_eq!(notes.line.as_deref(), Some("windows"));
    assert!(format!("{menu:?}").contains("describe: true"));
}

#[test]
fn menus_keep_ids_by_name_and_a_removed_menu_keeps_its_id() {
    let mut menus = Menus::new();
    assert!(menus.is_empty());
    let windows = menus.id("windows").unwrap();
    assert_eq!(menus.id("windows"), Ok(windows));
    // Reserved, not defined.
    assert_eq!(menus.lookup("windows"), None);
    assert!(menus.get(windows).is_none());
    menus.define(windows, menu()).unwrap();
    assert_eq!(menus.lookup("windows"), Some(windows));
    assert_eq!(menus.get(windows).unwrap().title(), "Windows");
    assert_eq!(menus.define(windows, menu()), Err(MenuError::NameTaken));
    let toggles = menus.id("toggles").unwrap();
    assert_ne!(windows, toggles);
    assert_eq!(menus.define(toggles, menu()), Err(MenuError::WrongName));
    assert_eq!(menus.len(), 1);
    // Replacing one whole is removing it, then defining it.
    menus.remove(windows);
    assert_eq!(menus.lookup("windows"), None);
    assert_eq!(menus.len(), 0);
    let mut other = Definition::new("windows", "Other").unwrap();
    other.add_group(None, 1).unwrap();
    menus.define(windows, other).unwrap();
    assert_eq!(menus.get(windows).unwrap().title(), "Other");
    assert_eq!(menus.id("Bad"), Err(MenuError::BadName));
    let names: Vec<&str> = menus.iter().map(|(_, menu)| menu.name()).collect();
    assert_eq!(names, ["windows"]);
}

#[test]
fn an_id_from_another_set_defines_nothing() {
    let mut first = Menus::new();
    let mut second = Menus::new();
    first.id("a").unwrap();
    let b = first.id("b").unwrap();
    second.id("a").unwrap();
    let definition = Definition::new("b", "B").unwrap();
    assert_eq!(second.define(b, definition), Err(MenuError::WrongName));
    assert!(second.get(b).is_none());
    second.remove(b);
}

#[test]
fn there_are_at_most_256_menus() {
    let mut menus = Menus::new();
    for i in 0..MAX_MENUS {
        menus.id(&format!("m{i}")).unwrap();
    }
    assert_eq!(menus.id("more"), Err(MenuError::TooManyMenus));
    assert!(menus.id("m0").is_ok());
}

#[test]
fn an_open_menu_flips_its_switches_which_start_off() {
    let mut menu = menu();
    menu.add_head(run("a")).unwrap();
    menu.add_head(head("- f", HeadAction::Switch("files".to_owned())))
        .unwrap();
    menu.add_head(head("- e", HeadAction::Switch("other-style".to_owned())))
        .unwrap();
    let definition = Arc::new(menu);
    let mut open = Open::new(Arc::clone(&definition));
    assert!(!open.switch("files"));
    assert_eq!(open.switches_on().count(), 0);
    assert!(open.flip("other-style"));
    assert!(open.flip("files"));
    assert!(open.switch("files"));
    assert_eq!(
        open.switches_on().collect::<Vec<_>>(),
        ["files", "other-style"]
    );
    assert!(!open.flip("files"));
    assert_eq!(open.switches_on().collect::<Vec<_>>(), ["other-style"]);
    // A name no head flips is off, and flipping it changes nothing.
    assert!(!open.flip("nothing"));
    assert!(!open.switch("nothing"));
    assert_eq!(open.definition().name(), "windows");
    // Opening again starts with every switch off (Q18).
    assert_eq!(Open::new(definition).switches_on().count(), 0);
}

#[test]
fn notes_are_found_by_head() {
    let note = |text: &str| Note {
        text: text.to_owned(),
        ..Note::default()
    };
    let notes = Notes {
        heads: vec![
            (2, note("12 files")),
            (0, note("(last)")),
            (2, note("later")),
        ],
        ..Notes::default()
    };
    assert_eq!(notes.get(2).unwrap().text, "12 files");
    assert_eq!(notes.get(0).unwrap().text, "(last)");
    assert!(notes.get(1).is_none());
}

#[test]
fn errors_read_as_sentences() {
    for error in [
        MenuError::BadName,
        MenuError::ReservedKey,
        MenuError::Cancel,
        MenuError::BadCount,
        MenuError::TooManyMenus,
    ] {
        let message = error.to_string();
        assert!(!message.ends_with('.'), "{message}");
    }
    assert_eq!(
        MenuError::BadCount.to_string(),
        "a count is at most 1000 either way"
    );
}
