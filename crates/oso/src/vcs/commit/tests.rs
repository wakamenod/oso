//! A commit's pure parts: each kind's arguments, the first text and its `#` lines, the message
//! once they are left out, its highlights, the ring, the hooks a failure names, and output as a
//! log shows it.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use oso_core::editor::{CommitKind, CommitSwitches};
use oso_core::syntax::Highlight;
use proptest::prelude::*;

use super::{
    Ring, Staged, args, failed, highlights, log_lines, log_rest, message_end, message_of, template,
};

#[test]
fn each_kind_runs_its_own_arguments_and_the_switches_add_flags() {
    let none = CommitSwitches::default();
    let base = ["-c", "core.commentChar=#", "commit", "--cleanup=strip"];
    let with = |rest: &[&'static str]| [&base[..], rest].concat();
    assert_eq!(args(CommitKind::New, none), with(&["--file=-"]));
    assert_eq!(
        args(CommitKind::Amend, none),
        with(&["--amend", "--file=-"])
    );
    assert_eq!(
        args(CommitKind::Extend, none),
        with(&["--amend", "--no-edit"])
    );
    assert_eq!(
        args(CommitKind::Reword, none),
        with(&["--amend", "--only", "--allow-empty", "--file=-"])
    );
    let all = CommitSwitches {
        no_verify: true,
        signoff: true,
        allow_empty: true,
    };
    assert_eq!(
        args(CommitKind::New, all),
        with(&["--file=-", "--no-verify", "--signoff", "--allow-empty"])
    );
    // Rewording allows an empty commit already, and says so once.
    let words = args(CommitKind::Reword, all);
    assert_eq!(words.iter().filter(|w| **w == "--allow-empty").count(), 1);
}

#[test]
fn the_first_text_names_the_staged_files_escaped_on_one_line_each() {
    let staged = [
        Staged {
            letter: 'M',
            path: b"src/a.rs",
        },
        Staged {
            letter: 'A',
            path: b"bad\nname",
        },
    ];
    let text = template("", Some("main"), &staged, 2, CommitKind::New);
    assert!(text.starts_with("\n\n# "), "{text:?}");
    assert!(text.contains("# On branch main\n"));
    assert!(text.contains("#   M  src/a.rs\n"));
    assert!(text.contains("#   A  bad\\nname\n"), "{text:?}");
    // Every line after the message is a `#` line, so none of it is committed.
    assert_eq!(message_of(&text), "");
    let amend = template("Subject\n\nBody\n", None, &staged, 2, CommitKind::Amend);
    assert!(amend.starts_with("Subject\n\nBody\n\n# "));
    assert_eq!(message_of(&amend), "Subject\n\nBody\n");
    let reword = template("Subject\n", Some("main"), &staged, 2, CommitKind::Reword);
    assert!(!reword.contains("src/a.rs"));
}

#[test]
fn many_staged_files_are_named_up_to_the_limit() {
    let paths: Vec<String> = (0..super::MAX_LISTED + 5)
        .map(|n| format!("f{n}"))
        .collect();
    let staged: Vec<Staged<'_>> = (paths.iter())
        .map(|path| Staged {
            letter: 'M',
            path: path.as_bytes(),
        })
        .collect();
    let text = template("", None, &staged, staged.len(), CommitKind::New);
    assert!(text.contains("#   … and 5 more\n"));
    // What the caller left out is counted too.
    let text = template(
        "",
        None,
        &staged[..super::MAX_LISTED],
        staged.len(),
        CommitKind::New,
    );
    assert!(text.contains("#   … and 5 more\n"));
    assert!(!text.contains(&format!("  f{}\n", super::MAX_LISTED)));
}

#[test]
fn a_message_leaves_out_comments_and_extra_blank_lines() {
    assert_eq!(
        message_of("\n\nSubject  \n\n\n\nBody\n# note\n\n"),
        "Subject\n\nBody\n"
    );
    assert_eq!(message_of("# only\n#\n"), "");
    assert_eq!(message_of("  \n\t\n"), "");
    assert_eq!(
        message_end("Sub\n\nBody\n# a\nnot\n"),
        "Sub\n\nBody\n".len()
    );
    assert_eq!(message_end("no comment"), "no comment".len());
}

#[test]
fn past_fifty_and_seventy_two_characters_and_a_second_line_are_marked() {
    let summary = "s".repeat(52);
    let body = "b".repeat(73);
    let text = format!("{summary}\nnot empty\n{body}\n# comment\n");
    let lit = highlights(&text);
    assert_eq!(lit.len(), text.len());
    assert_eq!(lit.at(49), None);
    assert_eq!(lit.at(50), Some(Highlight::GitCommitOverlong));
    assert_eq!(lit.at(52), None, "the line break");
    assert_eq!(lit.at(53), Some(Highlight::GitCommitOverlong));
    let third = 53 + "not empty\n".len();
    assert_eq!(lit.at(third + 71), None);
    assert_eq!(lit.at(third + 72), Some(Highlight::GitCommitOverlong));
    let comment = third + 74;
    assert_eq!(lit.at(comment), Some(Highlight::Comment));
    // Characters, not bytes: a summary of 50 wide characters is whole.
    let wide = "あ".repeat(50);
    let lit = highlights(&format!("{wide}\n"));
    assert_eq!(lit.at(wide.len() - 1), None);
}

#[test]
fn the_ring_keeps_twenty_newest_first_once_each() {
    let mut ring = Ring::default();
    for n in 0..25 {
        ring.keep(format!("m{n}\n"));
    }
    ring.keep("  \n".into());
    assert_eq!(ring.len(), super::MAX_RING);
    assert_eq!(ring.get(0), Some("m24\n"));
    ring.keep("m10\n".into());
    assert_eq!(ring.get(0), Some("m10\n"));
    assert_eq!(ring.len(), super::MAX_RING);
    assert_eq!(ring.get(super::MAX_RING), None);
}

#[cfg(unix)]
#[test]
fn a_failure_names_the_hooks_there_that_it_ran() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("oso-commit-hooks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let make = |name: &str, mode: u32| {
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    make("pre-commit", 0o755);
    make("commit-msg", 0o644);
    make("prepare-commit-msg", 0o755);
    assert_eq!(
        super::hooks_present(&dir, false),
        ["pre-commit", "prepare-commit-msg"]
    );
    assert_eq!(super::hooks_present(&dir, true), ["prepare-commit-msg"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failure_says_which_hooks_may_have_made_it() {
    assert_eq!(
        failed(&["pre-commit"]),
        "commit failed (hook pre-commit): $ shows the output"
    );
    assert_eq!(
        failed(&["pre-commit", "prepare-commit-msg"]),
        "commit failed (hook pre-commit or prepare-commit-msg): $ shows the output"
    );
    assert_eq!(failed(&[]), "commit failed: $ shows the output");
}

#[test]
fn output_shows_as_whole_clean_lines() {
    let mut partial = Vec::new();
    assert_eq!(
        log_lines(b"\x1b[31mred\x1b[0m line\nhalf", &mut partial),
        "red line\n"
    );
    assert_eq!(log_lines(b" done\r\n", &mut partial), "half done\n");
    assert_eq!(
        log_lines(
            b"10%\r50%\r100%\n\x1b]0;title\x07tab\there\x00\n",
            &mut partial
        ),
        "100%\ntab\there\n"
    );
    assert_eq!(log_lines(b"\xff\xfe end", &mut partial), "");
    assert_eq!(log_rest(&mut partial), "\u{fffd}\u{fffd} end\n");
    assert_eq!(log_rest(&mut partial), "");
    assert_eq!(log_lines("\u{202e}rtl\n".as_bytes(), &mut partial), "rtl\n");
}

proptest! {
    #[test]
    fn any_output_gives_lines_with_no_control_character(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let mut partial = Vec::new();
        let mut out = log_lines(&bytes, &mut partial);
        out.push_str(&log_rest(&mut partial));
        prop_assert!(out.is_empty() || out.ends_with('\n'));
        prop_assert!(!out.chars().any(|c| c.is_control() && c != '\n' && c != '\t'));
    }

    #[test]
    fn any_text_gets_highlights_that_cover_it(text in "\\PC*") {
        prop_assert_eq!(highlights(&text).len(), text.len());
        prop_assert!(!message_of(&text).lines().any(|line| line.starts_with('#')));
    }
}

#[test]
fn a_line_rewritten_over_many_pieces_keeps_only_its_last_part() {
    let mut partial = Vec::new();
    for n in 0..1000 {
        assert_eq!(log_lines(format!("{n}%\r").as_bytes(), &mut partial), "");
        assert!(partial.len() < 16, "{}", partial.len());
    }
    assert_eq!(log_lines(b"done\n", &mut partial), "done\n");
    // A carriage return last in a piece may begin a CRLF.
    assert_eq!(log_lines(b"a\r", &mut partial), "");
    assert_eq!(log_lines(b"\nb\n", &mut partial), "a\nb\n");
}
