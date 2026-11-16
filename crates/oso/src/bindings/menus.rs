//! The built-in menus (ADR 0117): defaults that `config.toml` adds to, replaces whole by name, or
//! removes. They come first in every set, so their ids, which default keys name, never change.
//! Each head names a command oso has; the author's heads whose commands do not exist yet stay
//! out, and their keys stay free for them.
//!
//! Must not depend on: `dispatch`, `app` or winit; menus are data, and a describe function reads
//! only the editor.

use oso_core::command::Registry;
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::menu::{Definition, Head, HeadAction, Note, Notes, Open, Stay};
use oso_core::menu::{Describe, Menus};

use crate::commands::agent_sessions::{ARCHIVE_LABEL, PIN_LABEL, agents_menu};

mod vcs;

/// The keys bound to built-in menus in the default global keymap, as `(keys, menu)`: the
/// toggles (ADR 0117, Q25), the review menu (ADR 0122) and the Agents menu (ADR 0143); the rest
/// are reached from another menu, with `M-x open-menu` or a key in `config.toml`.
pub const MENU_KEYS: &[(&str, &str)] = &[
    ("C-c t", "toggles"),
    ("C-x v r", "review"),
    ("C-c a", "agents"),
];

/// One head of a built-in menu: its keys, command (or switch's or menu's name), label, whether
/// it stays, a fixed count, and whether it flips a switch or opens a menu rather than run a
/// command.
struct BuiltInHead {
    keys: &'static str,
    command: &'static str,
    label: &'static str,
    stay: bool,
    count: Option<i16>,
    switch: bool,
    menu: bool,
}

/// A head that closes the menu.
const fn exit(keys: &'static str, command: &'static str, label: &'static str) -> BuiltInHead {
    BuiltInHead {
        keys,
        command,
        label,
        stay: false,
        count: None,
        switch: false,
        menu: false,
    }
}

/// A head that opens the built-in menu `name` over this one (ADR 0117, Q16).
const fn open(keys: &'static str, name: &'static str, label: &'static str) -> BuiltInHead {
    BuiltInHead {
        menu: true,
        ..exit(keys, name, label)
    }
}

/// A head that flips the switch `name`, as transient's arguments (ADR 0117).
const fn switch(keys: &'static str, name: &'static str, label: &'static str) -> BuiltInHead {
    BuiltInHead {
        switch: true,
        ..exit(keys, name, label)
    }
}

/// A head that keeps the menu open.
const fn stay(keys: &'static str, command: &'static str, label: &'static str) -> BuiltInHead {
    BuiltInHead {
        stay: true,
        ..exit(keys, command, label)
    }
}

/// A head that keeps the menu open and runs with a fixed count, as `C-u 3` would.
const fn stay_by(
    keys: &'static str,
    command: &'static str,
    label: &'static str,
    count: i16,
) -> BuiltInHead {
    BuiltInHead {
        count: Some(count),
        ..stay(keys, command, label)
    }
}

/// A built-in menu: its name, title, groups of heads (each in the first row), and its describe
/// function.
struct BuiltInMenu {
    name: &'static str,
    title: &'static str,
    groups: &'static [(&'static str, &'static [BuiltInHead])],
    describe: Option<Describe>,
}

/// The label of the toggles' theme head, which its describe function finds it by. `toggle-theme`
/// switches the pair `[theme] pair` names, so the label names no theme; the note does.
const THEME_LABEL: &str = "theme";

/// The cells of the longest built-in theme's name, which the theme head's note is padded to, so
/// that a theme changed while the menu is open, as a reload of `config.toml` may, keeps the
/// note's width and does not lay the menu out again (ADR 0117, step 2); the theme head itself
/// closes the menu. A test keeps it in step with the renderer's themes.
const THEME_NOTE_CELLS: usize = 5;

/// The toggles: the visual aids, line numbers, truncation, highlighting, the theme pair, and
/// whitespace cleanup. Each toggling command has a `state`, so its head shows `[x]` or `[ ]`.
const TOGGLES: BuiltInMenu = BuiltInMenu {
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
const WINDOWS: BuiltInMenu = BuiltInMenu {
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
const JUMPS: BuiltInMenu = BuiltInMenu {
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
const SEARCH: BuiltInMenu = BuiltInMenu {
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

/// Reviews (ADR 0122): a review of each scope, and the session's baseline, with the counts and
/// the reasons its describe function says.
const REVIEW: BuiltInMenu = BuiltInMenu {
    name: "review",
    title: "Review",
    groups: &[
        (
            "Review",
            &[
                exit("D", "review-session", "session changes"),
                exit("w", "review-uncommitted", "uncommitted"),
                exit("u", "review-unstaged", "unstaged"),
                exit("s", "review-staged", "staged"),
                exit("b", "review-branch", "branch against base"),
            ],
        ),
        (
            "Revisions",
            &[
                exit("c", "review-commit", "a commit\u{2026}"),
                exit("r", "review-range", "a range\u{2026}"),
            ],
        ),
        (
            "Session",
            &[stay("B", "review-mark-baseline", "take a baseline")],
        ),
    ],
    describe: Some(crate::commands::review::review_menu),
};

/// Issue spaces (ADR 0136, step 1b): the space in front's repositories, instructions,
/// directory and name, and closing and reopening. `o` and `O` are kept for the orchestrator's
/// start and replacement (steps 3 and 5).
const SPACE: BuiltInMenu = BuiltInMenu {
    name: "space",
    title: "Space",
    groups: &[
        (
            "Repositories",
            &[
                exit("a", "space-add-repo", "add\u{2026}"),
                exit("r", "space-remove-repo", "remove\u{2026}"),
                exit("u", "space-use-repo", "use for commands\u{2026}"),
            ],
        ),
        (
            "Space",
            &[
                exit("i", "space-instructions", "instructions"),
                exit("d", "space-directory", "directory"),
                exit("n", "space-rename", "rename\u{2026}"),
            ],
        ),
        (
            "Orchestrator",
            &[
                exit("o", "space-orchestrator-start", "start"),
                exit("O", "space-orchestrator-replace", "replace"),
            ],
        ),
        (
            "Open",
            &[
                exit("c", "space-close", "close"),
                exit("R", "space-reopen", "reopen\u{2026}"),
            ],
        ),
    ],
    describe: None,
};

/// The agent sessions' commands (ADR 0143), on `C-c a`, with the letters `C-x a` had (ADR 0121,
/// Q7, ADR 0136). Session mode's `C-c C-<letter>` runs the head of the same letter.
const AGENTS: BuiltInMenu = BuiltInMenu {
    name: "agents",
    title: "Agents",
    groups: &[
        (
            "Start",
            &[
                exit("c", "claude", "new session"),
                exit("w", "session-worktree", "in a worktree\u{2026}"),
                exit("r", "session-resume", "resume\u{2026}"),
                exit("a", "session-adopt", "adopt\u{2026}"),
            ],
        ),
        (
            "Go to",
            &[
                exit("s", "sessions", "pane"),
                exit("b", "session-switch", "session\u{2026}"),
                exit("n", "session-next-waiting", "next waiting"),
                exit("d", "session-next-done", "next done"),
                exit("f", "session-follow-message", "message's sender"),
            ],
        ),
        (
            "Talk",
            &[
                exit("m", "session-send", "send\u{2026}"),
                exit("!", "session-send-diagnostics", "send diagnostics\u{2026}"),
                exit("x", "session-fix-error", "fix error\u{2026}"),
                exit("@", "session-mention", "mention\u{2026}"),
                exit("e", "session-explain", "explain"),
            ],
        ),
        (
            "Space",
            &[
                exit("j", "space-switch", "switch\u{2026}"),
                exit("z", "space-zoom", "zoom"),
                exit("O", "space-new", "new\u{2026}"),
                open("S", "space", "space menu"),
            ],
        ),
        (
            "Session",
            &[
                exit("K", "session-stop", "stop"),
                exit("P", "session-pin", PIN_LABEL),
                exit("A", "session-archive", ARCHIVE_LABEL),
                exit("v", "review-session", "review changes"),
                open("V", "review", "review menu"),
            ],
        ),
    ],
    describe: Some(agents_menu),
};

/// The built-in menus, in the order their ids are given.
const BUILT_IN: [&BuiltInMenu; 13] = [
    &TOGGLES,
    &WINDOWS,
    &JUMPS,
    &SEARCH,
    &REVIEW,
    &SPACE,
    &vcs::COMMIT,
    &vcs::PUSH,
    &vcs::PULL,
    &vcs::FETCH,
    &vcs::STASH,
    &vcs::BRANCH,
    &AGENTS,
];

/// The built-in menus over the commands in `registry`. A head whose command `registry` lacks is
/// left out, as in tests that bring their own commands; with the default commands every head is
/// there, which a test checks.
///
/// # Panics
///
/// If a built-in table breaks a menu's rules. The tests build these menus, so such a mistake
/// cannot ship.
pub fn built_in_menus(registry: &Registry) -> Menus {
    let mut menus = Menus::new();
    for menu in BUILT_IN {
        let id = menus
            .id(menu.name)
            .unwrap_or_else(|e| panic!("menu {}: {e}", menu.name));
        let definition = definition(menu, registry, &mut menus);
        menus
            .define(id, definition)
            .unwrap_or_else(|e| panic!("menu {}: {e}", menu.name));
    }
    menus
}

/// The definition of `menu`, with the heads whose commands `registry` has; a head that opens a
/// menu takes that menu's id from `menus`.
fn definition(menu: &BuiltInMenu, registry: &Registry, menus: &mut Menus) -> Definition {
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
            let action = if head.switch {
                HeadAction::Switch(head.command.to_owned())
            } else if head.menu {
                HeadAction::Menu(menus.id(head.command).unwrap_or_else(|e| fail(&e)))
            } else {
                let Some(command) = registry.lookup(head.command) else {
                    continue;
                };
                HeadAction::Command(command)
            };
            let keys: KeySequence = head
                .keys
                .parse()
                .unwrap_or_else(|e| fail(&format_args!("{}: {e}", head.keys)));
            definition
                .add_head(Head {
                    keys,
                    label: head.label.to_owned(),
                    action,
                    stay: (head.stay && !head.switch).then_some(Stay::Stay),
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
