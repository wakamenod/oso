//! Scenario tests of open menus through the [`Dispatcher`](super::Dispatcher) (ADR 0117): the
//! keys that open them, the rules of their heads (stay, exit, switches, submenus), keys they do
//! not bind, `q` and `C-g`, the prefix argument, describe functions, and what closes them.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, Status};
use oso_core::key::KeySequence;
use oso_core::keymap::Keymap;
use oso_core::menu::{
    Definition, ForeignKeys, Head, HeadAction, MAX_DEPTH, MenuId, Menus, Note, Notes, Open, Stay,
};

use super::Dispatcher;
use super::motion_tests::{at, cursors, send};
use crate::{bindings, commands};

fn echo(editor: &Editor) -> String {
    let mut status = Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

/// Says which of the switches `files` and `other` its menu had on, for the tests.
fn say_switches(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let on = |name| if ctx.switch(name) { name } else { "-" };
    let said = format!(
        "files={} other={} n={}",
        on("files"),
        on("other"),
        args.count()
    );
    ctx.editor.message(said);
    Ok(())
}

/// A dispatcher with the default commands and keys, and `say-switches`.
fn dispatcher() -> Dispatcher {
    let mut registry = commands::registry();
    registry
        .register(Command {
            name: "say-switches",
            doc: "Say the switches on, for the tests.",
            args: ArgSpec::Count,
            reach: Reach::Look,
            run: say_switches,
            state: None,
        })
        .unwrap();
    let global = bindings::global_keymap(&registry);
    Dispatcher::with(registry, global)
}

/// A head on `keys` for `action`.
fn head(keys: &str, label: &str, action: HeadAction) -> Head {
    Head {
        keys: keys.parse().unwrap(),
        label: label.to_owned(),
        action,
        stay: None,
        count: None,
    }
}

/// The command `name`'s head, staying or not.
fn command(d: &Dispatcher, keys: &str, name: &str, stay: Option<Stay>) -> Head {
    Head {
        stay,
        ..head(
            keys,
            name,
            HeadAction::Command(d.registry.lookup(name).unwrap()),
        )
    }
}

/// The menus `test` and `sub`, with `C-c m` opening `test`:
/// - `test`: `f` forward-char (stay), `b` backward-char (stay), `e` end-of-buffer (exit), `Z Z`
///   beginning-of-buffer, `1` forward-char by 3 (stay), `- f` the switch `files`, `w`
///   say-switches, `s` the submenu `sub`, `x` closes it;
/// - `sub`: `n` forward-char (stay), `w` say-switches, `- o` the switch `other`, `e`
///   end-of-buffer (exit).
fn menus(d: &Dispatcher, foreign: ForeignKeys) -> (Menus, MenuId) {
    let mut menus = Menus::new();
    let test = menus.id("test").unwrap();
    let sub = menus.id("sub").unwrap();
    let mut t = Definition::new("test", "Test").unwrap();
    t.set_foreign_keys(foreign);
    t.add_group(Some("Motion"), 1).unwrap();
    t.add_head(command(d, "f", "forward-char", Some(Stay::Stay)))
        .unwrap();
    t.add_head(command(d, "b", "backward-char", Some(Stay::Stay)))
        .unwrap();
    t.add_head(command(d, "e", "end-of-buffer", None)).unwrap();
    t.add_head(command(d, "Z Z", "beginning-of-buffer", None))
        .unwrap();
    t.add_head(Head {
        count: Some(3),
        ..command(d, "1", "forward-char", Some(Stay::Stay))
    })
    .unwrap();
    t.add_group(Some("Other"), 1).unwrap();
    t.add_head(head("- f", "files", HeadAction::Switch("files".into())))
        .unwrap();
    t.add_head(command(d, "w", "say-switches", None)).unwrap();
    t.add_head(head("s", "sub", HeadAction::Menu(sub))).unwrap();
    t.add_head(head("x", "close", HeadAction::Quit)).unwrap();
    menus.define(test, t).unwrap();
    let mut s = Definition::new("sub", "Sub").unwrap();
    s.add_group(None, 1).unwrap();
    s.add_head(command(d, "n", "forward-char", Some(Stay::Stay)))
        .unwrap();
    s.add_head(command(d, "w", "say-switches", None)).unwrap();
    s.add_head(head("- o", "other", HeadAction::Switch("other".into())))
        .unwrap();
    s.add_head(command(d, "e", "end-of-buffer", None)).unwrap();
    menus.define(sub, s).unwrap();
    (menus, test)
}

/// A user keymap binding `keys` to `menu`.
fn opening(keys: &str, menu: MenuId) -> Keymap {
    let mut keymap = Keymap::new();
    let keys: KeySequence = keys.parse().unwrap();
    keymap.bind_menu(&keys.0, menu).unwrap();
    keymap
}

/// A dispatcher with the test menus in force, `C-c m` opening `test`, and an editor showing
/// `text` with its cursor at `cursor`.
fn with_menus(text: &str, cursor: usize, foreign: ForeignKeys) -> (Dispatcher, Editor) {
    let (_, editor) = at(text, &[cursor]);
    let mut d = dispatcher();
    let (menus, test) = menus(&d, foreign);
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c m", test));
    (d, editor)
}

#[test]
fn a_menus_key_opens_it_its_stay_heads_keep_it_and_an_exit_head_closes_it_then_runs() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Warn);
    assert!(send(&mut d, &mut editor, "C-c m").redraw);
    assert_eq!(d.menu_name(), Some("test"));
    assert!(d.menu_open() && d.reads_keys() && !d.types_text(&editor));
    send(&mut d, &mut editor, "f f");
    assert_eq!(cursors(&editor), [2]);
    assert_eq!(d.menu_name(), Some("test"));
    send(&mut d, &mut editor, "b");
    assert_eq!(cursors(&editor), [1]);
    send(&mut d, &mut editor, "e");
    assert_eq!(cursors(&editor), [6]);
    assert_eq!(d.menu_name(), None);
    assert!(!d.reads_keys());
    // The keys are the global ones again.
    send(&mut d, &mut editor, "C-b");
    assert_eq!(cursors(&editor), [5]);
}

#[test]
fn a_heads_keys_may_be_a_sequence_whose_first_key_waits_in_the_echo_area() {
    let (mut d, mut editor) = with_menus("abcdef", 3, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m Z");
    assert_eq!(echo(&editor), "Z-");
    assert_eq!(cursors(&editor), [3]);
    send(&mut d, &mut editor, "Z");
    assert_eq!(cursors(&editor), [0]);
    assert_eq!(d.menu_name(), None);
}

#[test]
fn a_foreign_key_is_refused_and_drops_the_prefix_argument_by_default() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m C-u C-f");
    assert_eq!(echo(&editor), "C-f is not in this menu: q to quit");
    assert_eq!(cursors(&editor), [0]);
    assert_eq!(d.menu_name(), Some("test"));
    // The prefix argument went with it: `f` moves one.
    send(&mut d, &mut editor, "f");
    assert_eq!(cursors(&editor), [1]);
    // So does a sequence that begins a head's and goes astray.
    send(&mut d, &mut editor, "Z y");
    assert_eq!(echo(&editor), "Z y is not in this menu: q to quit");
}

#[test]
fn a_foreign_key_may_close_the_menu_and_run_or_run_and_keep_it() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Exit);
    send(&mut d, &mut editor, "C-c m C-f");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], None));
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Run);
    send(&mut d, &mut editor, "C-c m C-f");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], Some("test")));
    // A prefix key run so goes on through the usual keymaps, and the menu stays.
    send(&mut d, &mut editor, "C-x");
    assert_eq!(echo(&editor), "C-x-");
    send(&mut d, &mut editor, "C-x");
    assert_eq!(d.menu_name(), Some("test"));
    send(&mut d, &mut editor, "f");
    assert_eq!(cursors(&editor), [2], "the menu's keys come first again");
}

#[test]
fn text_in_a_menu_follows_its_rule_for_foreign_keys() {
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m");
    d.insert_text(&mut editor, "日本");
    assert_eq!(echo(&editor), "That text is not in this menu: q to quit");
    assert_eq!(super::motion_tests::text(&editor), "abc");
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Exit);
    send(&mut d, &mut editor, "C-c m");
    d.commit_text(&mut editor, "日本");
    assert_eq!(super::motion_tests::text(&editor), "日本abc");
    assert_eq!(d.menu_name(), None);
}

#[test]
fn q_and_c_g_close_one_level_and_c_g_first_ends_what_is_being_typed() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m s");
    assert_eq!(d.menu_name(), Some("sub"));
    send(&mut d, &mut editor, "q");
    assert_eq!(d.menu_name(), Some("test"));
    send(&mut d, &mut editor, "Z C-g");
    assert_eq!(
        (echo(&editor).as_str(), d.menu_name()),
        ("Quit", Some("test"))
    );
    send(&mut d, &mut editor, "C-u C-g");
    assert_eq!(d.menu_name(), Some("test"));
    send(&mut d, &mut editor, "f");
    assert_eq!(cursors(&editor), [1], "C-g ended the prefix argument");
    send(&mut d, &mut editor, "C-g");
    assert_eq!(d.menu_name(), None);
    // A head that only closes the menu.
    send(&mut d, &mut editor, "C-c m x");
    assert_eq!(d.menu_name(), None);
}

#[test]
fn a_head_on_q_runs_and_a_foreign_key_then_says_c_g() {
    let (_, mut editor) = at("abc", &[0]);
    let mut d = dispatcher();
    let mut menus = Menus::new();
    let id = menus.id("q").unwrap();
    let mut menu = Definition::new("q", "Q").unwrap();
    menu.add_group(None, 1).unwrap();
    menu.add_head(command(&d, "q", "forward-char", Some(Stay::Stay)))
        .unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c q", id));
    send(&mut d, &mut editor, "C-c q q");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], Some("q")));
    send(&mut d, &mut editor, "z");
    assert_eq!(echo(&editor), "z is not in this menu: C-g to quit");
}

#[test]
fn the_prefix_argument_goes_to_the_next_head_and_plain_digits_are_heads() {
    let (mut d, mut editor) = with_menus("abcdefghijkl", 0, ForeignKeys::Warn);
    // Typed before the menu's key, it goes to its first head.
    send(&mut d, &mut editor, "C-u 2 C-c m f");
    assert_eq!(cursors(&editor), [2]);
    // Inside it, C-u and M-<digit> build one; `1` is a head, whose count replaces it.
    send(&mut d, &mut editor, "M-3 f");
    assert_eq!(cursors(&editor), [5]);
    send(&mut d, &mut editor, "C-u 1");
    assert_eq!(cursors(&editor), [8], "the head's count, not C-u's 4");
    send(&mut d, &mut editor, "M-- f");
    assert_eq!(cursors(&editor), [7]);
    // The echo area shows no prefix argument in a menu: its title row does.
    send(&mut d, &mut editor, "C-u");
    assert_eq!(echo(&editor), "");
    let menu = d.menu().unwrap();
    let mut prefix = String::new();
    menu.prefix_into(&mut prefix);
    assert_eq!(prefix, "C-u-");
}

#[test]
fn a_heads_command_reads_the_switches_of_its_menu_which_start_off() {
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m - f");
    assert!(d.menu().unwrap().open().switch("files"));
    send(&mut d, &mut editor, "M-4 w");
    assert_eq!(echo(&editor), "files=files other=- n=4");
    send(&mut d, &mut editor, "C-c m w");
    assert_eq!(
        echo(&editor),
        "files=- other=- n=1",
        "they start off each time"
    );
    // Each level has its own.
    send(&mut d, &mut editor, "C-c m - f s - o w");
    assert_eq!(echo(&editor), "files=- other=other n=1");
}

#[test]
fn a_submenu_goes_over_its_menu_and_an_exit_head_in_it_closes_every_level() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m s n n");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![2], Some("sub")));
    send(&mut d, &mut editor, "C-g");
    assert_eq!(d.menu_name(), Some("test"));
    send(&mut d, &mut editor, "s e");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![6], None));
}

#[test]
fn a_menu_already_open_is_not_pushed_again_and_the_stack_has_a_bottom() {
    let (_, mut editor) = at("abc", &[0]);
    let mut d = dispatcher();
    let mut menus = Menus::new();
    // m0 opens m1, … m9 opens m0.
    let ids: Vec<MenuId> = (0..10)
        .map(|i| menus.id(&format!("m{i}")).unwrap())
        .collect();
    for (i, id) in ids.iter().enumerate() {
        let name = format!("m{i}");
        let mut menu = Definition::new(&name, &name).unwrap();
        menu.add_group(None, 1).unwrap();
        let next = ids[(i + 1) % ids.len()];
        menu.add_head(head("s", "next", HeadAction::Menu(next)))
            .unwrap();
        menu.add_head(head("a", "first", HeadAction::Menu(ids[0])))
            .unwrap();
        menus.define(*id, menu).unwrap();
    }
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c n", ids[0]));
    send(&mut d, &mut editor, "C-c n s a");
    assert_eq!(echo(&editor), "m0 is already open");
    assert_eq!(d.menu_name(), Some("m1"));
    send(&mut d, &mut editor, "s s s s s s s");
    assert_eq!(d.menu_name(), Some("m7"));
    assert_eq!(
        echo(&editor),
        format!("Menus open at most {MAX_DEPTH} deep")
    );
}

#[test]
fn a_reload_while_a_menu_is_open_changes_neither_it_nor_its_submenus() {
    let (mut d, mut editor) = with_menus("abcdef", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m");
    // A new set where the id `sub` had is another menu, with no heads.
    let mut other = Menus::new();
    let first = other.id("first").unwrap();
    let second = other.id("second").unwrap();
    for (id, name) in [(first, "first"), (second, "second")] {
        let mut menu = Definition::new(name, name).unwrap();
        menu.add_group(None, 1).unwrap();
        menus_head(&mut menu);
        other.define(id, menu).unwrap();
    }
    d.set_menus(Some(other));
    send(&mut d, &mut editor, "s");
    assert_eq!(d.menu_name(), Some("sub"), "the set the menu came from");
    send(&mut d, &mut editor, "C-g C-g");
    // The next opening takes the new set, where C-c m now names `first`.
    send(&mut d, &mut editor, "C-c m");
    assert_eq!(d.menu_name(), Some("first"));
}

fn menus_head(menu: &mut Definition) {
    menu.add_head(head("z", "close", HeadAction::Quit)).unwrap();
}

#[test]
fn a_head_that_opens_a_prompt_or_a_search_closes_the_menu_and_a_failing_one_keeps_it() {
    let (mut d, mut editor) = with_menus("abc", 3, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m f");
    assert_eq!(d.menu_name(), Some("test"), "End of buffer, and it stays");
    assert_eq!(echo(&editor), "End of buffer");
    // A foreign key under `run` that opens a prompt closes the menu as the prompt opens.
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Run);
    send(&mut d, &mut editor, "C-c m C-x C-f");
    assert!(editor.prompt().is_some());
    assert_eq!(d.menu_name(), None);
    send(&mut d, &mut editor, "C-g");
    send(&mut d, &mut editor, "C-c m C-s");
    assert!(editor.isearch().is_some());
    assert_eq!(d.menu_name(), None);
}

#[test]
fn no_menu_opens_over_a_prompt() {
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-x C-f");
    send(&mut d, &mut editor, "C-c m");
    assert_eq!(d.menu_name(), None);
    assert!(editor.prompt().is_some());
}

#[test]
fn a_key_whose_menu_is_not_in_force_is_undefined() {
    let (_, mut editor) = at("abcdef", &[3]);
    let mut d = dispatcher();
    let mut menus = Menus::new();
    let gone = menus.id("gone").unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c g", gone));
    send(&mut d, &mut editor, "C-c g");
    assert_eq!(echo(&editor), "That key is undefined");
    assert_eq!(d.menu_name(), None);
}

/// A describe function for [`described`]: head 0 hidden, head 1 unavailable, head 2 noted and
/// the one `RET` runs, and a line.
fn describe(_: &Editor, _: &Open) -> Notes {
    let note = |hidden, unavailable: Option<&str>, text: &str| Note {
        text: text.to_owned(),
        unavailable: unavailable.map(str::to_owned),
        hidden,
    };
    Notes {
        line: Some("about this".to_owned()),
        ret_head: Some(2),
        heads: vec![
            (0, note(true, None, "")),
            (1, note(false, Some("Not now"), "")),
            (2, note(false, None, "(last)")),
        ],
    }
}

/// A built-in-like menu with [`describe`]: `a`, `b` and `c` move to the end, the start and
/// one forward.
fn described(d: &mut Dispatcher) -> MenuId {
    let mut menus = Menus::new();
    let id = menus.id("live").unwrap();
    let mut menu = Definition::new("live", "Live").unwrap();
    menu.set_describe(describe);
    menu.add_group(None, 1).unwrap();
    menu.add_head(command(d, "a", "end-of-buffer", None))
        .unwrap();
    menu.add_head(command(d, "b", "beginning-of-buffer", None))
        .unwrap();
    menu.add_head(command(d, "c", "forward-char", None))
        .unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c l", id));
    id
}

#[test]
fn a_describe_function_hides_heads_refuses_unavailable_ones_and_names_the_ret_head() {
    let (_, mut editor) = at("abcdef", &[3]);
    let mut d = dispatcher();
    described(&mut d);
    send(&mut d, &mut editor, "C-c l a");
    assert_eq!(echo(&editor), "a is not in this menu: q to quit", "hidden");
    send(&mut d, &mut editor, "b");
    assert_eq!(echo(&editor), "Not now");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![3], Some("live")));
    send(&mut d, &mut editor, "RET");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![4], None));
    let notes = {
        send(&mut d, &mut editor, "C-c l");
        d.menu().unwrap().notes(&editor)
    };
    assert_eq!(notes.line.as_deref(), Some("about this"));
}

#[test]
fn ret_runs_the_definitions_default_head_where_no_head_binds_it() {
    let (mut d, mut editor) = with_menus("abcdef", 3, ForeignKeys::Warn);
    let mut menus = Menus::new();
    let id = menus.id("ret").unwrap();
    let mut menu = Definition::new("ret", "Ret").unwrap();
    menu.add_group(None, 1).unwrap();
    let head = menu
        .add_head(command(&d, "a", "end-of-buffer", None))
        .unwrap();
    menu.set_ret_head(head).unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c r", id));
    send(&mut d, &mut editor, "C-c r RET");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![6], None));
    // With none, RET is a foreign key.
    let (mut d, mut editor) = with_menus("abcdef", 3, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-c m RET");
    assert_eq!(echo(&editor), "RET is not in this menu: q to quit");
}

#[test]
fn open_menu_reads_a_menus_name_with_its_key_beside_it_and_opens_it() {
    let (mut d, mut editor) = with_menus("abc", 0, ForeignKeys::Warn);
    let mut user = opening("C-c m", d.menus().lookup("test").unwrap());
    let keys: KeySequence = "C-c o".parse().unwrap();
    user.bind(&keys.0, d.registry.lookup("open-menu").unwrap())
        .unwrap();
    d.set_user_keymap(user);
    send(&mut d, &mut editor, "C-u 2 C-c o");
    let list = editor.prompt().unwrap().candidates().unwrap();
    let mut candidates: Vec<(String, String)> = (0..list.len())
        .filter_map(|place| list.get(place))
        .map(|c| (c.text().to_owned(), c.binding().to_owned()))
        .collect();
    candidates.sort();
    assert_eq!(
        candidates,
        [
            ("sub".to_owned(), String::new()),
            ("test".to_owned(), "C-c m".to_owned())
        ]
    );
    d.insert_text(&mut editor, "test");
    send(&mut d, &mut editor, "RET");
    assert_eq!(d.menu_name(), Some("test"));
    // The prefix argument typed before goes to its first head.
    send(&mut d, &mut editor, "f");
    assert_eq!(cursors(&editor), [2]);
}

#[test]
fn a_macro_records_a_menus_keys_and_its_replay_presses_them_again() {
    let (mut d, mut editor) = with_menus("abcdefghij", 0, ForeignKeys::Warn);
    send(&mut d, &mut editor, "C-x ( C-c m f f q C-x )");
    assert_eq!(cursors(&editor), [2]);
    send(&mut d, &mut editor, "C-x e");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!(cursors(&editor), [4]);
    assert_eq!(d.menu_name(), None);
    // A replay that ends with a menu open leaves it open: here `C-x )` runs through a menu
    // that runs foreign keys.
    let (mut d, mut editor) = with_menus("abcdefghij", 0, ForeignKeys::Run);
    send(&mut d, &mut editor, "C-x ( C-c m f C-x ) C-g");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], None));
    send(&mut d, &mut editor, "C-x e");
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!((cursors(&editor), d.menu_name()), (vec![2], Some("test")));
}

#[test]
fn menus_wait_for_a_pending_prefix_with_the_keymap_and_none_puts_back_the_built_in_ones() {
    let (_, mut editor) = at("abcdef", &[3]);
    let mut d = dispatcher();
    let builtin = d.menus().len();
    let one = |name: &str| {
        let mut menus = Menus::new();
        let id = menus.id(name).unwrap();
        menus
            .define(id, Definition::new(name, "T").unwrap())
            .unwrap();
        menus
    };
    assert!(d.set_menus(Some(one("first"))).is_some());
    assert!(d.menus().lookup("first").is_some());
    send(&mut d, &mut editor, "C-x");
    assert!(d.set_menus(Some(one("second"))).is_none());
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
