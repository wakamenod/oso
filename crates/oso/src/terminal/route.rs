//! Which keys in a terminal are oso's and which go to its program (ADR 0120, Q12): semi-char
//! mode keeps `C-x`, `M-x`, the `C-c` prefix, `C-q` and `C-y`; char mode keeps only `C-c C-k`,
//! which leaves it, and `C-c C-c`, which sends one `C-c` there too. Anything of oso's that is open, a prompt or a key sequence, takes every key.
//! In a session's terminal, semi-char mode also keeps `C-z` from the program, which would
//! suspend it with nothing to resume it (ADR 0132, Q7).
//!
//! Must not depend on: `app`, `dispatch` or the terminal's thread; this is a pure decision.

use oso_core::key::{Key, KeyPress, Modifiers};

/// How a terminal reads keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Input {
    /// Every key goes to the program but oso's few (the default).
    #[default]
    SemiChar,
    /// Every key goes to the program but `C-c C-k`.
    Char,
}

/// What waits to go to a terminal's program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToProgram {
    Keys(Vec<KeyPress>),
    /// Text an input method committed, or a key that types several characters.
    Text(String),
    Paste(String),
    /// The person said yes to pasting lines without bracketed paste.
    PasteLines,
}

/// What becomes of one key pressed in a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// oso's: the dispatcher resolves it, through the terminal's keymap.
    Oso,
    /// Sent to the program, in this order.
    Send(Vec<KeyPress>),
    /// Held until the next key: char mode's `C-c`.
    Hold,
    /// Char mode's `C-c C-k`: back to semi-char mode.
    LeaveCharMode,
    /// `C-z` in a session's terminal, kept from its program (ADR 0132, Q7): Claude Code would
    /// suspend itself, and as its process group has no shell, nothing would resume it.
    SuspendKept,
}

/// What the route of a key depends on besides the key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct State {
    pub input: Input,
    /// Something of oso's is open: a prompt, isearch, a menu, a jump, a key sequence or a
    /// prefix argument in progress. It takes every key, `C-g` among them.
    pub oso_busy: bool,
    /// `C-q` came: the next key goes to the program as it is.
    pub quoted: bool,
    /// Char mode's `C-c` came, and waits for the next key.
    pub held_c_c: bool,
    /// The terminal hosts an agent session (ADR 0132).
    pub session: bool,
}

/// `C-c`.
pub(crate) const C_C: KeyPress = KeyPress {
    modifiers: Modifiers::CONTROL,
    key: Key::Char('c'),
};

/// `C-z`.
const C_Z: KeyPress = KeyPress {
    modifiers: Modifiers::CONTROL,
    key: Key::Char('z'),
};

/// The keys semi-char mode keeps for oso: `C-x` and `M-x`, the `C-c` prefix, `C-q`, which sends
/// the next key as it is, and `C-y`, which pastes.
fn oso_keeps(key: KeyPress) -> bool {
    let control = |c| {
        key == KeyPress {
            modifiers: Modifiers::CONTROL,
            key: Key::Char(c),
        }
    };
    let meta_x = key
        == KeyPress {
            modifiers: Modifiers::META,
            key: Key::Char('x'),
        };
    control('x') || control('c') || control('q') || control('y') || meta_x
}

/// Where `key` goes. The caller clears `quoted` and `held_c_c` once a key was routed.
pub(crate) fn route(state: State, key: KeyPress) -> Route {
    if state.oso_busy {
        return Route::Oso;
    }
    if state.quoted {
        return Route::Send(vec![key]);
    }
    match state.input {
        Input::SemiChar if oso_keeps(key) => Route::Oso,
        Input::SemiChar if state.session && key == C_Z => Route::SuspendKept,
        Input::SemiChar => Route::Send(vec![key]),
        Input::Char if state.held_c_c => {
            let c_k = KeyPress {
                modifiers: Modifiers::CONTROL,
                key: Key::Char('k'),
            };
            if key == c_k {
                Route::LeaveCharMode
            } else if key == C_C {
                // `C-c C-c` sends one `C-c`, as in semi-char mode and Emacs's term.
                Route::Send(vec![C_C])
            } else {
                Route::Send(vec![C_C, key])
            }
        }
        Input::Char if key == C_C => Route::Hold,
        Input::Char => Route::Send(vec![key]),
    }
}

#[cfg(test)]
mod tests {
    use oso_core::key::KeyPress;

    use super::{C_C, Input, Route, State, route};

    fn key(name: &str) -> KeyPress {
        name.parse().unwrap()
    }

    fn semi() -> State {
        State::default()
    }

    fn char_mode() -> State {
        State {
            input: Input::Char,
            ..State::default()
        }
    }

    #[test]
    fn semi_char_keeps_oso_s_few_keys_and_sends_the_rest() {
        for name in ["C-x", "M-x", "C-c", "C-q", "C-y"] {
            assert_eq!(route(semi(), key(name)), Route::Oso, "{name}");
        }
        for name in [
            "a", "A", "あ", "RET", "S-RET", "TAB", "DEL", "C-a", "C-e", "C-g", "C-k", "C-s", "M-b",
            "M-p", "<up>", "<f5>", "C-u", "ESC",
        ] {
            assert_eq!(
                route(semi(), key(name)),
                Route::Send(vec![key(name)]),
                "{name}"
            );
        }
    }

    #[test]
    fn anything_of_oso_s_open_takes_every_key() {
        let busy = State {
            oso_busy: true,
            ..semi()
        };
        for name in ["a", "C-g", "RET", "C-x"] {
            assert_eq!(route(busy, key(name)), Route::Oso, "{name}");
        }
        let busy = State {
            oso_busy: true,
            ..char_mode()
        };
        assert_eq!(route(busy, key("C-g")), Route::Oso);
    }

    #[test]
    fn a_quoted_key_goes_to_the_program_as_it_is() {
        for state in [semi(), char_mode()] {
            let quoted = State {
                quoted: true,
                ..state
            };
            for name in ["C-x", "C-q", "M-x", "C-c"] {
                assert_eq!(
                    route(quoted, key(name)),
                    Route::Send(vec![key(name)]),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn a_session_keeps_c_z_from_its_program_and_sends_claude_code_s_keys() {
        let session = State {
            session: true,
            ..semi()
        };
        assert_eq!(route(session, key("C-z")), Route::SuspendKept);
        assert_eq!(route(semi(), key("C-z")), Route::Send(vec![key("C-z")]));
        // `C-q C-z` sends it, and char mode sends it as every key (ADR 0132, Q7).
        let quoted = State {
            quoted: true,
            ..session
        };
        assert_eq!(route(quoted, key("C-z")), Route::Send(vec![key("C-z")]));
        let char_session = State {
            session: true,
            ..char_mode()
        };
        assert_eq!(
            route(char_session, key("C-z")),
            Route::Send(vec![key("C-z")])
        );
        // The stash, the model picker and history go to Claude Code (Q6).
        for name in [
            "C-s", "M-p", "M-n", "C-u", "S-RET", "C-RET", "ESC", "S-TAB", "C-g",
        ] {
            assert_eq!(
                route(session, key(name)),
                Route::Send(vec![key(name)]),
                "{name}"
            );
        }
        for name in ["C-x", "M-x", "C-c", "C-q", "C-y"] {
            assert_eq!(route(session, key(name)), Route::Oso, "{name}");
        }
    }

    #[test]
    fn char_mode_sends_every_key_but_c_c_c_k() {
        for name in ["C-x", "M-x", "C-q", "C-y", "a", "C-g"] {
            assert_eq!(
                route(char_mode(), key(name)),
                Route::Send(vec![key(name)]),
                "{name}"
            );
        }
        assert_eq!(route(char_mode(), C_C), Route::Hold);
        let held = State {
            held_c_c: true,
            ..char_mode()
        };
        assert_eq!(route(held, key("C-k")), Route::LeaveCharMode);
        assert_eq!(route(held, C_C), Route::Send(vec![C_C]));
        assert_eq!(route(held, key("a")), Route::Send(vec![C_C, key("a")]));
    }
}
