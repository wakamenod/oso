//! The tests of `[terminal]` in [`parse`](super::parse): the shell, login, the environment's
//! names and the scrollback, and what is reported of bad ones (ADR 0120).
//!
//! Must not depend on: `app`, `dispatch` or the file system; these run on strings alone.

use std::path::PathBuf;

use super::tests::{Fixture, messages};

#[test]
fn every_setting_is_read() {
    let f = Fixture::new();
    let shell = if cfg!(windows) {
        "C:/sh.exe"
    } else {
        "/bin/zsh"
    };
    let loaded = f.parse(&format!(
        "[terminal]\nshell = \"{shell}\"\nlogin = false\nenv = [\"LANG\", \"EDITOR\", \"LANG\"]\nscrollback = 2000\n"
    ));
    assert_eq!(messages(&loaded), Vec::<String>::new());
    let terminal = &loaded.settings.terminal;
    assert_eq!(terminal.shell, Some(PathBuf::from(shell)));
    assert_eq!(terminal.login, Some(false));
    assert_eq!(
        terminal.env.as_deref(),
        Some(&["LANG".to_owned(), "EDITOR".to_owned()][..])
    );
    assert_eq!(terminal.scrollback, Some(2000));
}

#[test]
fn a_relative_shell_is_reported_and_left_out() {
    let f = Fixture::new();
    let loaded = f.parse("[terminal]\nshell = \"zsh\"\n");
    assert_eq!(
        messages(&loaded),
        ["2:9: [terminal] shell takes an absolute path; the next shell is used"]
    );
    assert_eq!(loaded.settings.terminal.shell, None);
}

#[test]
fn names_the_process_module_refuses_or_a_terminal_sets_are_skipped() {
    let f = Fixture::new();
    let loaded = f.parse(
        "[terminal]\nenv = [\"PATH\", \"LD_PRELOAD\", \"DYLD_LIBRARY_PATH\", \"A=B\", \"\", \"TERM\", \"KEEP\"]\n",
    );
    let said = messages(&loaded);
    assert_eq!(said.len(), 6, "{said:?}");
    assert!(said[0].contains("\"PATH\": PATH is oso's own"), "{said:?}");
    assert!(said[1].contains("loader's variables"), "{said:?}");
    assert!(said[2].contains("loader's variables"), "{said:?}");
    assert!(said[3].contains("no =, NUL"), "{said:?}");
    assert!(said[4].contains("an empty name"), "{said:?}");
    assert!(said[5].contains("sets it itself"), "{said:?}");
    assert_eq!(
        loaded.settings.terminal.env.as_deref(),
        Some(&["KEEP".to_owned()][..])
    );
}

#[test]
fn scrollback_is_a_number_of_lines_up_to_100000() {
    let f = Fixture::new();
    for (value, kept) in [
        ("0", Some(0)),
        ("100000", Some(100_000)),
        ("100001", None),
        ("-1", None),
        ("1.5", None),
        ("\"many\"", None),
    ] {
        let loaded = f.parse(&format!("[terminal]\nscrollback = {value}\n"));
        assert_eq!(loaded.settings.terminal.scrollback, kept, "{value}");
        assert_eq!(messages(&loaded).is_empty(), kept.is_some(), "{value}");
    }
}

#[test]
fn login_takes_true_or_false() {
    let f = Fixture::new();
    let loaded = f.parse("[terminal]\nlogin = \"yes\"\n");
    assert_eq!(loaded.settings.terminal.login, None);
    assert_eq!(messages(&loaded).len(), 1);
}
