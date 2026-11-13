//! Default key bindings of the prefix argument commands (ADR 0033, ADR 0034,
//! ADR 0058).
//!
//! Must not depend on: `dispatch`, `app` or winit; bindings are data.

/// Key sequences and the commands they run.
pub const BINDINGS: &[(&str, &str)] = &[
    ("C-u", "universal-argument"),
    ("M--", "negative-argument"),
    ("M-0", "digit-argument"),
    ("M-1", "digit-argument"),
    ("M-2", "digit-argument"),
    ("M-3", "digit-argument"),
    ("M-4", "digit-argument"),
    ("M-5", "digit-argument"),
    ("M-6", "digit-argument"),
    ("M-7", "digit-argument"),
    ("M-8", "digit-argument"),
    ("M-9", "digit-argument"),
];

/// Keys that build on a prefix argument while it reads digits, in the keymap
/// active then. Any other key does what it always does, with the prefix as its
/// argument.
pub const DIGITS: &[(&str, &str)] = &[
    ("0", "digit-argument"),
    ("1", "digit-argument"),
    ("2", "digit-argument"),
    ("3", "digit-argument"),
    ("4", "digit-argument"),
    ("5", "digit-argument"),
    ("6", "digit-argument"),
    ("7", "digit-argument"),
    ("8", "digit-argument"),
    ("9", "digit-argument"),
];

/// What builds on a prefix argument with no number yet, in a keymap active only
/// then: after a number `-` is text, as in Emacs.
pub const MINUS: &[(&str, &str)] = &[("-", "negative-argument")];

#[cfg(test)]
mod tests {
    use oso_core::key::KeySequence;
    use oso_core::menu::builds_prefix;

    use super::BINDINGS;

    /// An open menu reads as the prefix argument's the keys these bind, and only those
    /// (ADR 0117 step 2).
    #[test]
    fn a_menu_reads_these_keys_as_the_prefix_arguments() {
        let bound = |spelled: &str| BINDINGS.iter().any(|(keys, _)| *keys == spelled);
        for (keys, _) in BINDINGS {
            let keys: KeySequence = keys.parse().unwrap();
            assert!(
                matches!(&keys.0[..], [key] if builds_prefix(*key)),
                "{keys}"
            );
        }
        let chars = ('a'..='z').chain('0'..='9').chain(['-']);
        for c in chars {
            for modifier in ["", "C-", "M-", "C-M-", "S-"] {
                let spelled = format!("{modifier}{c}");
                let Ok(keys) = spelled.parse::<KeySequence>() else {
                    continue;
                };
                assert_eq!(builds_prefix(keys.0[0]), bound(&spelled), "{spelled}");
            }
        }
    }
}
