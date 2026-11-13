//! The tests of `[menus]` in [`parse`](super::parse) and of keys bound to menus (ADR 0117): what
//! a file adds, replaces and removes, and each check, with its line.
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use oso_core::keymap::Binding;
use oso_core::menu::{Definition, ForeignKeys, Head, HeadAction, Menus, Stay};

use super::Loaded;
use super::tests::{Fixture, keys, messages};

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
    assert_eq!(windows.default_stay(), Stay::Stay);
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
    assert_eq!(windows.ret_head(), Some(2));
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
        "18:3: \"i\": a label takes at most 64 cells and 512 bytes; skipped",
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
fn options_that_do_nothing_warn_and_the_head_loads() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"
[menus.m]
[menus.n]
[[menus.m.groups]]
heads = [
  ["a", { switch = "files" }, "x", { stay = false, count = 2 }],
  ["b", false, "x", { stay = true, count = 2 }],
  ["c", { menu = "n" }, "x", { stay = false, count = 2 }],
  ["d", "forward-char", "x", { stay = true, count = 2 }],
]
"#,
    );
    let said = messages(&loaded);
    let expected = [
        "6:45: \"a\": stay does nothing on a switch, which always stays; ignored",
        "6:60: \"a\": count does nothing on a switch; ignored",
        "7:30: \"b\": stay does nothing on a head that only closes the menu; ignored",
        "7:44: \"b\": count does nothing on a head that only closes the menu; ignored",
        "8:39: \"c\": stay does nothing on a head that opens a menu, which always stays; ignored",
    ];
    assert_eq!(said, expected);
    let m = menu(&loaded, "m");
    assert_eq!(m.heads().len(), 4);
    assert_eq!(m.heads()[0].count, None);
    assert_eq!(m.heads()[2].count, Some(2));
    assert_eq!(m.heads()[3].stay, Some(Stay::Stay));
}

#[test]
fn ret_is_refused_beside_a_default_head_either_way() {
    let f = Fixture::new();
    let loaded = f.parse(
        r#"
[menus.m]
[[menus.m.groups]]
heads = [
  ["a", "forward-char", "x", { default = true }],
  ["RET", "forward-char", "x"],
]
[menus.n]
[[menus.n.groups]]
heads = [
  ["RET", "forward-char", "x"],
  ["a", "forward-char", "x", { default = true }],
]
"#,
    );
    let said = messages(&loaded);
    let ret = "RET runs the menu's default head, so no other head's keys may begin with it";
    assert_eq!(
        said,
        [
            format!("6:3: \"RET\": {ret}; skipped"),
            format!("12:3: \"a\": {ret}; it is not made the default head"),
        ]
    );
    assert_eq!(menu(&loaded, "m").ret_head(), Some(0));
    assert_eq!(heads(menu(&loaded, "m")).len(), 1);
    assert_eq!(menu(&loaded, "n").ret_head(), None);
    assert_eq!(heads(menu(&loaded, "n")).len(), 2);
}

#[test]
fn heads_past_the_limit_are_skipped_with_one_message() {
    let f = Fixture::new();
    // 130 heads of two keys each, then a group of one more.
    let mut text = String::from("[menus.m]\n[[menus.m.groups]]\nheads = [\n");
    let letters = "abcdefghijklmnoprstuvwxyz";
    let mut n = 0;
    'all: for a in letters.chars() {
        for b in letters.chars() {
            if n == 130 {
                break 'all;
            }
            text.push_str(&format!("  [\"{a} {b}\", \"forward-char\", \"x\"],\n"));
            n += 1;
        }
    }
    text.push_str("]\n[[menus.m.groups]]\nheads = [[\"Z\", \"forward-char\", \"x\"]]\n");
    let loaded = f.parse(&text);
    assert_eq!(
        messages(&loaded),
        ["132:3: a menu has at most 128 heads; this head and the rest are skipped"]
    );
    assert_eq!(menu(&loaded, "m").heads().len(), 128);
}

#[test]
fn menus_that_are_not_tables_or_break_a_limit_are_skipped() {
    let f = Fixture::new();
    let mut text = String::from("[menus]\nBad = {}\nnumber = 3\ntoo = true\n");
    // One more than the built-in menus leave room for.
    for i in 0..257 - f.menus.len() {
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

#[test]
fn removing_a_built_in_menu_unbinds_its_default_key_only() {
    let f = Fixture::new();
    let loaded = f.parse("[menus]\ntoggles = false\n");
    assert!(messages(&loaded).is_empty(), "{:?}", messages(&loaded));
    assert!(menus(&loaded).lookup("toggles").is_none());
    let user = keys(&loaded);
    assert_eq!(f.bound(user, "C-c t"), Some("undefined"));
    // The rest of `C-c` is the defaults' still.
    assert_eq!(f.bound(user, "C-c C-<"), None);
    // A menu with no default key unbinds nothing.
    let loaded = f.parse("[menus]\nwindows = false\n");
    assert!(keys(&loaded).is_empty());
    assert!(menus(&loaded).lookup("windows").is_none());
}

#[test]
fn a_replaced_built_in_menu_keeps_its_default_key() {
    let f = Fixture::new();
    let toggles = f.menus.lookup("toggles").unwrap();
    let loaded = f.parse(
        "[menus.toggles]\ntitle = \"Mine\"\n[[menus.toggles.groups]]\nheads = [[\"x\", \"goto-line\", \"go\"]]\n",
    );
    assert!(messages(&loaded).is_empty(), "{:?}", messages(&loaded));
    assert!(keys(&loaded).is_empty());
    assert_eq!(menus(&loaded).lookup("toggles"), Some(toggles));
    assert_eq!(menu(&loaded, "toggles").title(), "Mine");
    let keys: oso_core::key::KeySequence = "C-c t".parse().unwrap();
    assert_eq!(f.defaults.lookup(&keys.0), Some(Binding::Menu(toggles)));
}

#[test]
fn a_removed_menus_key_the_file_binds_is_the_files() {
    let f = Fixture::new();
    for (value, bound) in [("\"goto-line\"", "goto-line"), ("false", "undefined")] {
        let text = format!("[menus]\ntoggles = false\n[keys.global]\n\"C-c t\" = {value}\n");
        let loaded = f.parse(&text);
        assert_eq!(f.bound(keys(&loaded), "C-c t"), Some(bound));
    }
    // One beside it under the same prefix keeps both.
    let loaded = f.parse("[menus]\ntoggles = false\n[keys.global]\n\"C-c a\" = \"goto-line\"\n");
    assert_eq!(f.bound(keys(&loaded), "C-c a"), Some("goto-line"));
    assert_eq!(f.bound(keys(&loaded), "C-c t"), Some("undefined"));
    // One the file binds to a command over its prefix leaves the default key unreachable.
    let loaded = f.parse("[menus]\ntoggles = false\n[keys.global]\n\"C-c\" = \"goto-line\"\n");
    assert_eq!(f.bound(keys(&loaded), "C-c"), Some("goto-line"));
}

/// The sample in `ui.md`'s Menus section, which must load with no problem.
const UI_SAMPLE: &str = r#"
[menus]
search = false

[menus.toggles]
title = "Toggles"
[[menus.toggles.groups]]
title = "Aids"
heads = [
  ["w", "global-whitespace-mode", "whitespace"],
  ["n", "global-display-line-numbers-mode", "line numbers", { stay = true }],
]

[menus.buffers]
title = "Buffers"
[[menus.buffers.groups]]
heads = [
  ["b", "switch-to-buffer", "switch"],
  [",", "previous-buffer", "previous", { stay = true }],
  [".", "next-buffer", "next", { stay = true }],
  ["w", { menu = "windows" }, "windows…"],
]

[keys.global]
"C-c w" = { menu = "windows" }
"C-c b" = { menu = "buffers" }
"#;

#[test]
fn the_sample_in_the_docs_loads_with_no_problem() {
    let f = Fixture::new();
    let loaded = f.parse(UI_SAMPLE);
    assert!(messages(&loaded).is_empty(), "{:?}", messages(&loaded));
    assert!(menus(&loaded).lookup("search").is_none());
    assert_eq!(heads(menu(&loaded, "toggles")).len(), 2);
    assert_eq!(heads(menu(&loaded, "buffers")).len(), 4);
    for sequence in ["C-c w", "C-c b"] {
        assert_eq!(f.bound(keys(&loaded), sequence), Some("menu"));
    }
}

#[test]
fn a_removed_menus_key_may_become_a_prefix_and_shift_finds_nothing_there() {
    let f = Fixture::new();
    // Without the removal, keys under `C-c t` are refused: it opens a menu by default.
    let kept = f.parse("[keys.global]\n\"C-c t w\" = \"global-whitespace-mode\"\n");
    assert_eq!(f.bound(keys(&kept), "C-c t w"), None);
    assert_eq!(messages(&kept).len(), 1, "{:?}", messages(&kept));
    let loaded = f.parse(
        "[menus]\ntoggles = false\n[keys.global]\n\"C-c t w\" = \"global-whitespace-mode\"\n",
    );
    assert!(messages(&loaded).is_empty(), "{:?}", messages(&loaded));
    assert_eq!(
        f.bound(keys(&loaded), "C-c t w"),
        Some("global-whitespace-mode")
    );
    assert_eq!(f.bound(keys(&loaded), "C-c t"), Some("prefix"));
    // `C-c T` = false says nothing of `C-c t`, which runs nothing now.
    let loaded = f.parse("[menus]\ntoggles = false\n[keys.global]\n\"C-c T\" = false\n");
    assert!(messages(&loaded).is_empty(), "{:?}", messages(&loaded));
    assert_eq!(f.bound(keys(&loaded), "C-c t"), Some("undefined"));
}
