//! Tests of the mode commands and `global-font-lock-mode` (ADR 0111).
//!
//! Must not depend on: `bindings`; these run the commands through the registry.

use oso_core::buffer::Mode;
use oso_core::command::Args;
use oso_core::syntax::{Highlight, Highlights};

use crate::commands::messages::kept_messages;
use crate::commands::tests::{editor, run};

#[test]
fn each_mode_command_sets_the_current_buffers_mode_and_says_so() {
    let mut editor = editor("fn a() {}\n");
    for (name, mode) in [
        ("rust-mode", Mode::Rust),
        ("markdown-mode", Mode::Markdown),
        ("toml-mode", Mode::Toml),
        ("fundamental-mode", Mode::Fundamental),
    ] {
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Ok(()), "{name}");
        assert!(effects.redraw, "{name}: the modeline names the mode");
        assert_eq!(editor.current().1.mode(), mode, "{name}");
        let said = kept_messages(&editor);
        assert_eq!(said.last().unwrap(), &format!("{} mode", mode.name()));
    }
}

#[test]
fn setting_the_same_mode_again_moves_its_revision_on() {
    let mut editor = editor("fn a() {}\n");
    let _ = run(&mut editor, "rust-mode", Args::NONE);
    let before = editor.current().1.mode_revision();
    let _ = run(&mut editor, "rust-mode", Args::NONE);
    assert_eq!(editor.current().1.mode(), Mode::Rust);
    assert!(
        editor.current().1.mode_revision() > before,
        "highlighting gets another try"
    );
}

#[test]
fn an_internal_buffers_mode_cannot_change() {
    let mut editor = editor("");
    let messages = editor.messages_buffer();
    editor.show(messages);
    assert!(editor.current().1.is_internal());
    let before = editor.current().1.mode();
    let (result, _) = run(&mut editor, "rust-mode", Args::NONE);
    assert_eq!(result, Err("This buffer's mode cannot change".to_owned()));
    assert_eq!(editor.current().1.mode(), before);
}

#[test]
fn font_lock_is_on_by_default_toggles_and_takes_an_argument() {
    let mut editor = editor("abc");
    assert!(editor.font_lock());
    let (_, effects) = run(&mut editor, "global-font-lock-mode", Args::NONE);
    assert!(effects.redraw);
    assert!(!editor.font_lock());
    let said = kept_messages(&editor);
    assert_eq!(said.last().unwrap(), "Global Font-Lock mode disabled");
    let (_, effects) = run(&mut editor, "global-font-lock-mode", Args::numeric(0));
    assert!(!editor.font_lock(), "zero turns it off");
    assert!(!effects.redraw, "nothing changed");
    let _ = run(&mut editor, "global-font-lock-mode", Args::numeric(4));
    assert!(editor.font_lock());
}

#[test]
fn font_lock_off_drops_every_buffers_highlights() {
    let mut editor = editor("fn a() {}\n");
    let _ = run(&mut editor, "rust-mode", Args::NONE);
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    let mut highlights = Highlights::new(buffer.text().len());
    highlights.replace(0..2, [(2, Some(Highlight::Keyword))]);
    assert!(buffer.take_highlights(buffer.revision(), highlights));
    let _ = run(&mut editor, "global-font-lock-mode", Args::numeric(-1));
    assert!(editor.buffer(id).unwrap().highlights().is_none());
}
