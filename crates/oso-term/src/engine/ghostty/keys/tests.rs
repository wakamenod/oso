//! Keys through the library's encoder, against the modes a program sets: the legacy forms,
//! application cursor keys (DECCKM), Alt's ESC prefix, and the kitty keyboard protocol's
//! `CSI u` forms once a program pushes its flags. The modifier bits, which the library's
//! manifest leaves out, are checked here through xterm's modifier parameter.
//!
//! Must not depend on: the policy or the thread.

use oso_core::key::KeyPress;

use crate::engine::Engine;
use crate::engine::ghostty::Ghostty;
use crate::engine::ghostty::tests::engine;
use crate::limits;

fn encode(e: &mut Ghostty, key: &str) -> Vec<u8> {
    let press: KeyPress = key.parse().expect("a key");
    let mut out = [0; limits::KEY_BYTES_MAX];
    let n = e.encode_key(press, &mut out);
    out[..n].to_vec()
}

#[track_caller]
fn sends(e: &mut Ghostty, key: &str, bytes: &[u8]) {
    let sent = encode(e, key);
    assert_eq!(
        sent,
        bytes,
        "{key}: {:?}, not {:?}",
        String::from_utf8_lossy(&sent),
        String::from_utf8_lossy(bytes)
    );
}

#[test]
fn arrows_follow_the_cursor_key_mode() {
    let mut e = engine(80, 24);
    sends(&mut e, "<up>", b"\x1b[A");
    sends(&mut e, "<left>", b"\x1b[D");
    e.feed(b"\x1b[?1h");
    sends(&mut e, "<up>", b"\x1bOA");
    sends(&mut e, "<down>", b"\x1bOB");
    sends(&mut e, "<right>", b"\x1bOC");
    sends(&mut e, "<left>", b"\x1bOD");
    e.feed(b"\x1b[?1l");
    sends(&mut e, "<up>", b"\x1b[A");
}

#[test]
fn control_and_meta_in_the_legacy_encoding() {
    let mut e = engine(80, 24);
    sends(&mut e, "C-a", b"\x01");
    sends(&mut e, "C-c", b"\x03");
    sends(&mut e, "C-SPC", b"\x00");
    sends(&mut e, "M-b", b"\x1bb");
    sends(&mut e, "M-B", b"\x1bB");
    sends(&mut e, "C-M-b", b"\x1b\x02");
    sends(&mut e, "M-.", b"\x1b.");
}

#[test]
fn function_and_editing_keys() {
    let mut e = engine(80, 24);
    sends(&mut e, "<f1>", b"\x1bOP");
    sends(&mut e, "<f5>", b"\x1b[15~");
    sends(&mut e, "<f12>", b"\x1b[24~");
    sends(&mut e, "RET", b"\r");
    sends(&mut e, "TAB", b"\t");
    sends(&mut e, "DEL", b"\x7f");
    sends(&mut e, "ESC", b"\x1b");
    sends(&mut e, "<delete>", b"\x1b[3~");
    sends(&mut e, "<home>", b"\x1b[H");
    sends(&mut e, "<prior>", b"\x1b[5~");
    // Past F25 the library has no key: nothing is sent.
    sends(&mut e, "<f30>", b"");
}

#[test]
fn text_is_sent_as_typed() {
    let mut e = engine(80, 24);
    sends(&mut e, "a", b"a");
    sends(&mut e, "A", b"A");
    sends(&mut e, "!", b"!");
    sends(&mut e, "SPC", b" ");
    sends(&mut e, "あ", "あ".as_bytes());
    sends(&mut e, "\u{1F600}", "\u{1F600}".as_bytes());
}

/// Each `GHOSTTY_MODS_*` bit oso sets is the modifier xterm's parameter names: 1 + Shift 1,
/// Alt 2, Control 4, Super 8.
#[test]
fn the_modifier_bits_are_the_librarys() {
    let mut e = engine(80, 24);
    sends(&mut e, "S-<up>", b"\x1b[1;2A");
    sends(&mut e, "M-<up>", b"\x1b[1;3A");
    sends(&mut e, "C-<up>", b"\x1b[1;5A");
    sends(&mut e, "s-<up>", b"\x1b[1;9A");
    sends(&mut e, "C-S-<up>", b"\x1b[1;6A");
}

#[test]
fn return_with_shift_or_control_in_the_kitty_protocol() {
    let mut e = engine(80, 24);
    e.feed(b"\x1b[>1u");
    sends(&mut e, "S-RET", b"\x1b[13;2u");
    sends(&mut e, "C-RET", b"\x1b[13;5u");
    sends(&mut e, "RET", b"\r");
    sends(&mut e, "a", b"a");
    sends(&mut e, "A", b"A");
    sends(&mut e, "あ", "あ".as_bytes());
    sends(&mut e, "C-a", b"\x1b[97;5u");
    sends(&mut e, "M-b", b"\x1b[98;3u");
    e.feed(b"\x1b[<u\x1b[>31u");
    sends(&mut e, "S-RET", b"\x1b[13;2u");
    sends(&mut e, "C-RET", b"\x1b[13;5u");
    sends(&mut e, "RET", b"\x1b[13u");
    sends(&mut e, "a", b"\x1b[97;;97u");
    // Popped, the legacy forms are back: xterm's for Shift and Return.
    e.feed(b"\x1b[<u");
    sends(&mut e, "S-RET", b"\x1b[27;2;13~");
    sends(&mut e, "C-a", b"\x01");
}
