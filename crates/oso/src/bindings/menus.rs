//! The built-in menus (ADR 0117): defaults that `config.toml` adds to, replaces whole by name, or
//! removes. They come first in every set, so their ids, which default keys name, never change.
//! Each head names a command oso has; the author's heads whose commands do not exist yet stay
//! out, and their keys stay free for them.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data.

use oso_core::command::Registry;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::menu::{Definition, Head, HeadAction, Note, Notes, Open, Stay};
use oso_core::menu::{Describe, Menus};

/// The keys bound to built-in menus in the default global keymap, as `(keys, menu)`. Only the
/// toggles have one (ADR 0117, Q25); the rest are reached with `M-x open-menu` or a key in
/// `config.toml`.
pub const MENU_KEYS: &[(&str, &str)] = &[("C-c t", "toggles")];

/// One head of a built-in menu: its keys, command, label, whether it stays, and a fixed count.
struct H {
    keys: &'static str,
    command: &'static str,
    label: &'static str,
    stay: bool,
    count: Option<i16>,
}

/// A head that closes the menu.
const fn exit(keys: &'static str, command: &'static str, label: &'static str) -> H {
    H {
        keys,
        command,
        label,
        stay: false,
        count: None,
    }
}

/// A head that keeps the menu open.
const fn stay(keys: &'static str, command: &'static str, label: &'static str) -> H {
    H {
        stay: true,
        ..exit(keys, command, label)
    }
}

/// A head that keeps the menu open and runs with a fixed count, as `C-u 3` would.
const fn stay_by(keys: &'static str, command: &'static str, label: &'static str, count: i16) -> H {
    H {
        count: Some(count),
        ..stay(keys, command, label)
    }
}

/// A built-in menu: its name, title, groups of heads (each in the first row), and its describe
/// function.
struct M {
    name: &'static str,
    title: &'static str,
    groups: &'static [(&'static str, &'static [H])],
    describe: Option<Describe>,
}

/// The label of the toggles' theme head, which its describe function finds it by.
const THEME_LABEL: &str = "dark/light";

/// The cells of the longest built-in theme's name, which the theme head's note is padded to, so
/// that switching themes keeps the note's width and does not lay the menu out again (ADR 0117,
/// step 2). A test keeps it in step with the renderer's themes.
const THEME_NOTE_CELLS: usize = 5;

/// The toggles: the visual aids, line numbers, truncation, highlighting, the theme pair, and
/// whitespace cleanup. Each toggling command has a `state`, so its head shows `[x]` or `[ ]`.
const TOGGLES: M = M {
    name: "toggles",
    title: "Toggles",
    groups: &[
        (
            "Basic",
            &[
                exit("w", "global-whitespace-mode", "whitespace"),
                exit("W", "delete-trailing-whitespace", "whitespace cleanup"),
                exit("b", "beacon-mode", "cursor flash"),
            ],
        ),
        (
            "Line & Column",
            &[
                exit("l", "toggle-truncate-lines", "truncate lines"),
                exit("n", "global-display-line-numbers-mode", "line numbers"),
            ],
        ),
        (
            "Highlight",
            &[
                exit("L", "global-hl-line-mode", "current line"),
                exit("S", "global-font-lock-mode", "syntax"),
            ],
        ),
        ("Theme", &[exit("T", "toggle-theme", THEME_LABEL)]),
    ],
    describe: Some(describe_toggles),
};

/// Windows and buffers: resizing by 3, balancing, splitting and deleting windows, buffers, and
/// finding files.
const WINDOWS: M = M {
    name: "windows",
    title: "Windows",
    groups: &[
        (
            "Transform",
            &[
                stay_by("<left>", "shrink-window-horizontally", "narrower", 3),
                stay_by("<up>", "shrink-window", "shorter", 3),
                stay_by("<down>", "enlarge-window", "taller", 3),
                stay_by("<right>", "enlarge-window-horizontally", "wider", 3),
                stay("=", "balance-windows", "balance"),
                exit("-", "split-window-below", "below"),
                exit("/", "split-window-right", "right"),
            ],
        ),
        (
            "Window",
            &[
                exit("d", "kill-buffer", "kill buffer"),
                exit("0", "delete-window", "delete"),
                exit("O", "delete-other-windows", "delete others"),
            ],
        ),
        (
            "Buffer",
            &[
                exit("b", "switch-to-buffer", "switch"),
                stay(",", "previous-buffer", "previous"),
                stay(".", "next-buffer", "next"),
            ],
        ),
        (
            "File",
            &[
                exit("f", "project-find-file", "project file"),
                exit("F", "find-file", "file"),
            ],
        ),
    ],
    describe: None,
};

/// Jumps: the avy jumps and `goto-line`.
const JUMPS: M = M {
    name: "jumps",
    title: "Jumps",
    groups: &[
        (
            "Char",
            &[
                exit("t", "avy-goto-char-timer", "timer"),
                exit("l", "avy-goto-line", "line"),
            ],
        ),
        (
            "Line",
            &[
                exit("h", "avy-goto-line", "head"),
                exit("e", "avy-goto-end-of-line", "end"),
                exit("n", "goto-line", "number"),
            ],
        ),
    ],
    describe: None,
};

/// Search: the project's files and their lines; the rest join as their commands come.
const SEARCH: M = M {
    name: "search",
    title: "Search",
    groups: &[(
        "Project",
        &[
            exit("f", "project-find-file", "find"),
            exit("r", "project-search", "grep"),
        ],
    )],
    describe: None,
};

/// The built-in menus, in the order their ids are given.
const BUILT_IN: [&M; 4] = [&TOGGLES, &WINDOWS, &JUMPS, &SEARCH];

/// The built-in menus over the commands in `registry`. A head whose command `registry` lacks is
/// left out, as in tests that bring their own commands; with the default commands every head is
/// there, which a test checks.
///
/// # Panics
///
/// If a built-in table breaks a menu's rules. The tests build these menus, so such a mistake
/// cannot ship.
pub fn menus(registry: &Registry) -> Menus {
    let mut menus = Menus::new();
    for menu in BUILT_IN {
        let id = menus
            .id(menu.name)
            .unwrap_or_else(|e| panic!("menu {}: {e}", menu.name));
        let definition = definition(menu, registry);
        menus
            .define(id, definition)
            .unwrap_or_else(|e| panic!("menu {}: {e}", menu.name));
    }
    menus
}

/// The definition of `menu`, with the heads whose commands `registry` has.
fn definition(menu: &M, registry: &Registry) -> Definition {
    let fail = |e: &dyn std::fmt::Display| -> ! { panic!("menu {}: {e}", menu.name) };
    let mut definition = Definition::new(menu.name, menu.title).unwrap_or_else(|e| fail(&e));
    if let Some(describe) = menu.describe {
        definition.set_describe(describe);
    }
    for &(title, heads) in menu.groups {
        definition
            .add_group(Some(title), 1)
            .unwrap_or_else(|e| fail(&e));
        for head in heads {
            let Some(command) = registry.lookup(head.command) else {
                continue;
            };
            let keys: KeySequence = head
                .keys
                .parse()
                .unwrap_or_else(|e| fail(&format_args!("{}: {e}", head.keys)));
            definition
                .add_head(Head {
                    keys,
                    label: head.label.to_owned(),
                    action: HeadAction::Command(command),
                    stay: head.stay.then_some(Stay::Stay),
                    count: head.count,
                })
                .unwrap_or_else(|e| fail(&format_args!("{}: {e}", head.keys)));
        }
    }
    definition
}

/// The toggles' notes: the theme head shows the theme drawn now, padded to the longest theme's
/// name, so its width stays the same.
fn describe_toggles(editor: &Editor, open: &Open) -> Notes {
    let theme = open
        .definition()
        .heads()
        .iter()
        .position(|head| head.label == THEME_LABEL);
    let (Some(head), Some(name)) = (theme, editor.theme_name()) else {
        return Notes::default();
    };
    Notes {
        heads: vec![(
            head,
            Note {
                text: format!("{name:<THEME_NOTE_CELLS$}"),
                ..Note::default()
            },
        )],
        ..Notes::default()
    }
}

#[cfg(test)]
mod tests;
