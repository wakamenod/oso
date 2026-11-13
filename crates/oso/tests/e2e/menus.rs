//! Menus in a running instance (ADR 0117): a menu from the configuration file opens on its key,
//! its float is drawn at once, its stay heads run and keep it, a switch flips, a submenu goes
//! over it and `C-g` goes back, a key it does not bind is refused, and an exit head closes it.
//! The input method is off and the state names it while it is open.
//!
//! Must not depend on: the app's internals, or logic a lower layer can test. The rules of each
//! key are in the dispatcher's tests.

use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::config::set_mode;
use super::cursor;
use super::harness::{App, Scratch};

/// A test menu on `C-c m`, with a submenu.
const CONFIG: &[u8] = br#"[keys.global]
"C-c m" = { menu = "test" }

[menus.test]
title = "Test"

[[menus.test.groups]]
title = "Move"
heads = [
  ["f", "forward-char", "forward", { stay = true }],
  ["b", "backward-char", "back", { stay = true }],
  ["e", "end-of-buffer", "end"],
]

[[menus.test.groups]]
title = "More"
heads = [
  ["- f", { switch = "files" }, "only files"],
  ["s", { menu = "sub" }, "sub"],
]

[menus.sub]
title = "Sub"

[[menus.sub.groups]]
heads = [
  ["n", "next-line", "next", { stay = true }],
]
"#;

fn menu(state: &Value) -> &Value {
    &state["menu"]
}

#[test]
#[ignore = "opens windows; run with `just e2e`"]
fn a_configured_menu_opens_runs_its_heads_nests_and_closes() {
    let scratch = Scratch::new("menus-flow");
    set_mode(scratch.path(), 0o700);
    let config = scratch.write("config.toml", CONFIG);
    set_mode(&config, 0o644);
    let file = scratch.write("notes.txt", b"one\ntwo\nthree\n");
    let mut app = App::start_with_config(Some(&file), scratch.path(), &config);
    app.visual_aids_off();
    app.wait_until_ready();
    let before = app.screenshot();

    // The file is read off the UI thread: its key works once it is in.
    let started = Instant::now();
    loop {
        app.send_keys("C-c m");
        if menu(&app.state()) == "test" {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the menu never opened"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let state = app.state();
    assert_eq!(state["ime"]["wanted"], false, "the input method is off");
    let open = app.screenshot();
    assert!(
        open.differs_in(&before, 0..open.height) > 0,
        "the float is drawn at once"
    );

    app.send_keys("f f");
    let state = app.state();
    assert_eq!(
        (cursor(&state), menu(&state)),
        ((0, 2), &Value::from("test"))
    );

    app.send_keys("- f");
    let flipped = app.screenshot();
    assert!(
        flipped.differs_in(&open, 0..open.height) > 0,
        "the switch's chip"
    );

    app.send_keys("s");
    assert_eq!(menu(&app.state()), "sub");
    app.send_keys("n");
    assert_eq!(cursor(&app.state()), (1, 2));
    app.send_keys("C-g");
    assert_eq!(menu(&app.state()), "test");

    app.send_keys("x");
    let state = app.state();
    assert_eq!(state["echo"], "x is not in this menu: q to quit");
    assert_eq!(menu(&state), "test");

    app.send_keys("e");
    let state = app.state();
    assert_eq!((cursor(&state), menu(&state)), ((3, 0), &Value::Null));
    let closed = app.screenshot();
    assert_eq!(closed.differs_in(&before, 0..1), 0, "no float is left");

    app.send_keys("C-c m C-g");
    assert_eq!(menu(&app.state()), &Value::Null);
}
