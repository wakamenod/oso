//! Scenario tests of what ends an open menu, and of `RET`, through the
//! [`Dispatcher`](super::Dispatcher) (ADR 0117): a menu's key during a search, a jump or the
//! layer of `C-x TAB`, the window's close button, a replay that left a menu open, and a default
//! head bound to `RET`; and the switches a head's command reads after a prompt or the clipboard.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Editor;
use oso_core::kill::FromClipboard;
use oso_core::menu::{Definition, ForeignKeys, Head, HeadAction, Menus, Stay};

use super::Dispatcher;
use super::menu_tests::{command, dispatcher, echo, opening, with_menus};
use super::motion_tests::{at, cursors, drain, send};
use crate::{bindings, commands};

#[test]
fn a_menus_key_ends_a_search_or_the_rigid_layer_and_opens_but_a_jump_ends_alone() {
    // isearch keeps the match it reached, as any key it does not bind leaves it.
    let (mut d, mut editor) = with_menus("abcabc", 0, ForeignKeys::Warn);
    // `b` is romaji, whose search may go on on the worker past the key's few milliseconds.
    send(&mut d, &mut editor, "C-s b");
    drain(&mut editor);
    send(&mut d, &mut editor, "C-c m");
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

/// Says what it read and whether the switch `files` was on, for the tests.
fn say_files(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let read = ctx
        .answer()
        .map(str::to_owned)
        .or_else(|| ctx.clipboard().map(|_| "clipboard".to_owned()))
        .unwrap_or_default();
    let said = format!("{read} files={}", ctx.switch("files"));
    ctx.editor.message(said);
    Ok(())
}

/// A menu on `C-c t` with the switch `- f`, `t` running a command that reads text first and
/// `y` one that waits for the clipboard first.
fn reading_heads() -> (Dispatcher, Editor) {
    let (_, editor) = at("abc", &[0]);
    let mut registry = commands::registry();
    let mut add = |name, args| {
        registry
            .register(Command {
                name,
                doc: "Say what was read and the switch, for the tests.",
                args,
                reach: Reach::Look,
                run: say_files,
                state: None,
            })
            .unwrap();
    };
    add(
        "say-text",
        ArgSpec::Text {
            prompt: "Say: ",
            history: "say",
        },
    );
    add("say-clipboard", ArgSpec::CountAndClipboard);
    let global = bindings::global_keymap(&registry);
    let mut d = Dispatcher::with(registry, global);
    let mut menus = Menus::new();
    let id = menus.id("reads").unwrap();
    let mut menu = Definition::new("reads", "Reads").unwrap();
    menu.add_group(None, 1).unwrap();
    menu.add_head(Head {
        keys: "- f".parse().unwrap(),
        label: "files".to_owned(),
        action: HeadAction::Switch("files".into()),
        stay: None,
        count: None,
    })
    .unwrap();
    menu.add_head(command(&d, "t", "say-text", None)).unwrap();
    menu.add_head(command(&d, "y", "say-clipboard", None))
        .unwrap();
    menus.define(id, menu).unwrap();
    d.set_menus(Some(menus));
    d.set_user_keymap(opening("C-c t", id));
    (d, editor)
}

#[test]
fn a_heads_command_reads_its_menus_switches_after_its_prompt_or_the_clipboard() {
    let (mut d, mut editor) = reading_heads();
    send(&mut d, &mut editor, "C-c t - f t");
    assert_eq!(d.menu_name(), None, "the prompt closed the menu");
    send(&mut d, &mut editor, "h i RET");
    assert_eq!(echo(&editor), "hi files=true");
    let (mut d, mut editor) = reading_heads();
    send(&mut d, &mut editor, "C-c t - f y");
    assert!(d.waits_for_clipboard());
    d.clipboard_answered(&mut editor, &FromClipboard::Nothing);
    assert_eq!(echo(&editor), "clipboard files=true");
}

#[test]
fn a_replay_that_left_a_menu_open_is_over_and_the_next_one_closes_it() {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let (mut d, mut editor) = with_menus("abcdefghij", 0, ForeignKeys::Run);
    // `f` types before the menu opens, and `C-x )` runs through it.
    send(&mut d, &mut editor, "C-x ( f C-c m C-x ) C-g");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![1], None));
    send(&mut d, &mut editor, "C-x e");
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!((cursors(&editor), d.menu_name()), (vec![2], Some("test")));
    // `e`, the repeat key, is the menu's head on `e` now: the end of `ffabcdefghij`.
    send(&mut d, &mut editor, "e");
    assert_eq!((cursors(&editor), d.menu_name()), (vec![12], None));
    let (mut d, mut editor) = with_menus("abcdefghij", 0, ForeignKeys::Run);
    send(&mut d, &mut editor, "C-x ( f C-c m C-x ) C-g C-x e");
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    // The next replay closes the menu as it begins, so its `f` types, and it leaves the menu
    // open again.
    send(&mut d, &mut editor, "C-x e");
    while editor.macro_replaying() {
        d.replay(&mut editor, until);
    }
    assert_eq!((cursors(&editor), d.menu_name()), (vec![3], Some("test")));
}
