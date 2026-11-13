//! The tests of `[menus]` in [`parse`](super::parse) and of keys bound to menus (ADR 0117): what
//! a file adds, replaces and removes, and each check, with its line.
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use oso_core::keymap::Binding;
use oso_core::menu::{Definition, ForeignKeys, Head, HeadAction, Menus, Stay};

use super::Loaded;
use super::tests::{Fixture, messages};

/// A fixture whose built-in menus are `toggles`, of one head, and `jumps`.
fn with_builtins() -> Fixture {
    let mut fixture = Fixture::new();
    let mut menus = Menus::new();
    for name in ["toggles", "jumps"] {
        let id = menus.id(name).unwrap();
        let mut menu = Definition::new(name, "Built in").unwrap();
        menu.add_group(None, 1).unwrap();
        menu.add_head(Head {
            keys: "w".parse().unwrap(),
            label: "whitespace".to_owned(),
            action: HeadAction::Command(fixture.registry.lookup("global-whitespace-mode").unwrap()),
            stay: None,
            count: None,
        })
        .unwrap();
        menus.define(id, menu).unwrap();
    }
    fixture.menus = menus;
    fixture
}

fn menus(loaded: &Loaded) -> &Menus {
    loaded.menus.as_ref().unwrap()
}

fn menu<'l>(loaded: &'l Loaded, name: &str) -> &'l Definition {
    let menus = menus(loaded);
    menus.get(menus.lookup(name).unwrap()).unwrap()
}

/// Each head's keys and label.
fn heads(menu: &Definition) -> Vec<(String, &str)> {
    menu.heads()
        .iter()
        .map(|head| (head.keys.to_string(), head.label.as_str()))
        .collect()
}

const WINDOWS: &str = r#"
[menus.windows]
title = "Windows"
foreign-keys = "run"
default-head = "stay"

[[menus.windows.groups]]
title = "Size"
heads = [
  ["<left>", "shrink-window-horizontally", "narrower", { stay = true, count = 3 }],
  ["b", "balance-windows", "balance", { stay = false }],
]

[[menus.windows.groups]]
title = "Split"
heads = [
  ["-", "split-window-below", "below", { default = true }],
  ["t", { menu = "more" }, "more…"],
  ["F", { switch = "files" }, "only files"],
  ["x", false, "close"],
]

[[menus.windows.groups]]
title = "Buffer"
row = 2
heads = [
  ["b", "switch-to-buffer", "switch"],
  [",", "previous-buffer", "previous"],
]

[menus.more]
[[menus.more.groups]]
heads = [["a", "other-window", "other"]]
"#;

#[test]
fn a_file_adds_menus_with_their_groups_heads_and_rules() {
    let f = Fixture::new();
    let loaded = f.parse(WINDOWS);
    let windows = menu(&loaded, "windows");
    assert_eq!(windows.title(), "Windows");
    assert_eq!(windows.foreign_keys(), ForeignKeys::Run);
    assert_eq!(windows.default_head(), Stay::Stay);
    assert_eq!(
        heads(windows),
        [
            ("<left>".to_owned(), "narrower"),
            ("b".to_owned(), "balance"),
            ("-".to_owned(), "below"),
            ("t".to_owned(), "more…"),
            ("F".to_owned(), "only files"),
            ("x".to_owned(), "close"),
            (",".to_owned(), "previous"),
        ]
    );
    let groups: Vec<(Option<&str>, u8)> = windows
        .groups()
        .iter()
        .map(|g| (g.title(), g.row()))
        .collect();
    assert_eq!(
        groups,
        [(Some("Size"), 1), (Some("Split"), 1), (Some("Buffer"), 2)]
    );
    let narrower = &windows.heads()[0];
    assert_eq!((narrower.stay, narrower.count), (Some(Stay::Stay), Some(3)));
    assert_eq!(windows.stay(1), Stay::Exit);
    assert_eq!(windows.default(), Some(2));
    // A head can name a menu the file defines after it.
    let more = menus(&loaded).lookup("more").unwrap();
    assert_eq!(windows.heads()[3].action, HeadAction::Menu(more));
    assert_eq!(
        windows.heads()[4].action,
        HeadAction::Switch("files".to_owned())
    );
    assert_eq!(windows.heads()[5].action, HeadAction::Quit);
    // A menu with no title is titled with its name.
    assert_eq!(menu(&loaded, "more").title(), "more");
    // The head that used `b` twice is refused with its line, and the rest loads.
    let said = messages(&loaded);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].starts_with("27:3: \"b\": another head of this menu has these keys"),
        "{said:?}"
    );
}

#[test]
fn a_file_replaces_a_built_in_menu_whole_and_removes_one_with_false() {
    let f = with_builtins();
    let builtin_toggles = f.menus.lookup("toggles").unwrap();
    let loaded = f.parse(
        r#"
        menus.jumps = false
        menus.gone = false

        [menus.toggles]
        title = "Mine"
        [[menus.toggles.groups]]
        heads = [["n", "global-display-line-numbers-mode", "numbers"]]
        "#,
    );
    let toggles = menu(&loaded, "toggles");
    assert_eq!(toggles.title(), "Mine");
    assert_eq!(heads(toggles), [("n".to_owned(), "numbers")]);
    // It keeps the built-in id, which default keys name.
    assert_eq!(menus(&loaded).lookup("toggles"), Some(builtin_toggles));
    assert_eq!(menus(&loaded).lookup("jumps"), None);
    assert_eq!(
        messages(&loaded),
        ["3:15: \"gone\": no built-in menu has this name, so none is removed"]
    );
    // With no [menus], the built-in ones stay.
    let plain = f.parse("");
    assert_eq!(menus(&plain).len(), 2);
}

#[test]
fn each_head_that_breaks_a_rule_is_skipped_with_its_line() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"
[menus.m]
title = "M"
colour = "red"
foreign-keys = "sometimes"
[[menus.m.groups]]
heads = [
  ["a", "no-such-command", "x"],
  ["b", { menu = "nowhere" }, "x"],
  ["c", { menu = "m" }, "x"],
  ["C-g", "forward-char", "x"],
  ["C-u", "forward-char", "x"],
  ["a b c d", "forward-char", "x"],
  ["e", "forward-char", "x", { count = 1001 }],
  ["f", "forward-char", "x", { colour = 1 }],
  ["g", "forward-char"],
  ["h", { open = "m" }, "x"],
  ["i", "forward-char", "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"],
  ["j", { switch = "Bad" }, "x"],
  ["<nokey>", "forward-char", "x"],
  ["k", "forward-char", "fine"],
]
[[menus.m.groups]]
row = 4
heads = [["l", "forward-char", "x"]]
"#,
    );
    let said = messages(&loaded);
    let expected = [
        "4:1: unknown name \"colour\" in [menus.m], skipped",
        "5:16: foreign-keys is one of \"warn\", \"exit\" or \"run\"",
        "8:9: \"a\": no command is named \"no-such-command\"",
        "9:9: \"b\": no menu is named \"nowhere\"",
        "10:9: \"c\": a menu cannot open itself",
        "11:3: \"C-g\": C-g is not configurable; it always cancels; skipped",
        "12:3: \"C-u\": C-u, M-<digit> and M-- build a prefix argument in a menu, so no head can use them; skipped",
        "13:3: \"a b c d\": a head takes at most 3 keys; skipped",
        "14:3: \"e\": a count is at most 1000 either way; skipped",
        "15:32: unknown name \"colour\" in a head's options, skipped",
        "16:3: a head is [keys, action, label], with an optional table of stay, count and default",
        "17:9: \"h\": a head's action is a command's name, { menu = \"name\" }, { switch = \"name\" }, or false to only close the menu",
        "18:3: \"i\": a label takes at most 64 cells; skipped",
        "19:3: \"j\": a switch's name must be lower-case kebab case, such as \"windows\", of at most 64 bytes; skipped",
        "20:3: \"<nokey>\": \"<nokey>\" is not a key; write keys as Emacs does, such as C-x, M-<f5> or SPC",
        "23:1: a group's row is 1 to 3; the group is skipped",
    ];
    assert_eq!(said, expected);
    // What is left loads: `f`, whose unknown option is skipped, and `k`. The second group is
    // skipped with its heads.
    let m = menu(&loaded, "m");
    assert_eq!(heads(m), [("f".to_owned(), "x"), ("k".to_owned(), "fine")]);
    assert_eq!(m.groups().len(), 1);
    assert_eq!(m.foreign_keys(), ForeignKeys::Warn);
}

#[test]
fn menus_that_are_not_tables_or_break_a_limit_are_skipped() {
    let f = Fixture::new();
    let mut text = String::from("[menus]\nBad = {}\nnumber = 3\ntoo = true\n");
    for i in 0..257 {
        text.push_str(&format!("m{i} = {{}}\n"));
    }
    let loaded = f.parse(&text);
    let said = messages(&loaded);
    assert_eq!(
        &said[..3],
        [
            "2:1: \"Bad\": a menu's name must be lower-case kebab case, such as \"windows\", of at most 64 bytes; skipped",
            "3:1: \"number\": a menu is a table, or false to remove a built-in one, not a number",
            "4:1: \"too\": a menu is a table, or false to remove a built-in one, not true or false",
        ]
    );
    let past: Vec<&String> = said
        .iter()
        .filter(|s| s.contains("there are at most 256 menus; skipped"))
        .collect();
    assert_eq!(past.len(), 1, "{said:?}");
    assert_eq!(menus(&loaded).len(), 256);
}

#[test]
fn a_key_opens_a_menu_the_file_or_the_defaults_define() {
    let f = with_builtins();
    let loaded = f.parse(
        r#"
        [keys.global]
        "C-c t" = { menu = "toggles" }
        "C-c w" = { menu = "windows" }
        "C-c n" = { menu = "nowhere" }
        "C-c j" = { menu = "jumps" }
        "C-c x" = { open = "toggles" }
        "C-c y" = { menu = 3 }

        [menus]
        jumps = false
        windows = {}
        "#,
    );
    let keys = loaded.keys.as_ref().unwrap();
    let menus = menus(&loaded);
    let bound =
        |spelled: &str| keys.lookup(&spelled.parse::<oso_core::key::KeySequence>().unwrap().0);
    assert_eq!(
        bound("C-c t"),
        Some(Binding::Menu(menus.lookup("toggles").unwrap()))
    );
    assert_eq!(
        bound("C-c w"),
        Some(Binding::Menu(menus.lookup("windows").unwrap()))
    );
    assert_eq!(bound("C-c n"), None);
    assert_eq!(bound("C-c j"), None);
    assert_eq!(
        messages(&loaded),
        [
            "5:19: \"C-c n\": no menu is named \"nowhere\"",
            "6:19: \"C-c j\": no menu is named \"jumps\"",
            "7:19: \"C-c x\": a key's table is { menu = \"name\" }, which opens that menu",
            "8:19: \"C-c y\": a menu is named with a string",
        ]
    );
}

#[test]
fn a_rejected_file_keeps_the_menus_in_force() {
    let f = Fixture::new();
    let loaded = f.parse("[menus.m\n");
    assert!(loaded.rejected());
    assert!(loaded.menus.is_none());
}

#[test]
fn an_action_table_of_another_kind_never_quotes_its_key() {
    let f = Fixture::new();
    let loaded = f.parse(
        "[menus.m]\n[[menus.m.groups]]\nheads = [[\"a\", { \"\\u202e\\u001b[2J\" = 1 }, \"x\"], [\"b\", { menu = 1 }, \"x\"]]\n",
    );
    let said = messages(&loaded);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said[0].ends_with("or false to only close the menu"),
        "{said:?}"
    );
    assert!(
        !said[0].contains('\u{202e}') && !said[0].contains('\u{1b}'),
        "{said:?}"
    );
    assert!(
        said[1].ends_with("\"b\": a menu is named with a string"),
        "{said:?}"
    );
}

#[test]
fn a_row_past_any_number_skips_its_group() {
    let f = Fixture::new();
    let loaded = f.parse(
        "[menus.m]\n[[menus.m.groups]]\nrow = 300\nheads = [[\"a\", \"forward-char\", \"x\"]]\n",
    );
    assert_eq!(
        messages(&loaded),
        ["2:1: a group's row is 1 to 3; the group is skipped"]
    );
    assert!(menu(&loaded, "m").heads().is_empty());
}

#[test]
fn unbinding_a_shifted_key_warns_when_shift_still_finds_a_menu() {
    let f = with_builtins();
    let loaded = f.parse(
        r#"
        [keys.global]
        "C-c C-t" = { menu = "toggles" }
        "C-c C-T" = false
        "#,
    );
    let said = messages(&loaded);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("changes nothing while \"C-c C-t\" is bound"),
        "{said:?}"
    );
}
