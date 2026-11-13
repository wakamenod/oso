//! Tests of `brief`: the first prompt's text with and without a hand-off, the facts dropped to
//! fit, and the 64 KiB limit to the byte.
//!
//! Must not depend on: the file system, processes or `porcelain`.

use std::path::{Path, PathBuf};

use proptest::prelude::*;

use super::{BriefError, Handoff, Header, MAX_BRIEF, compose};

fn header() -> Header<'static> {
    Header {
        repo: Path::new("/repo"),
        path: Path::new("/repo/.claude/worktrees/feat-x"),
        branch: "feat/x",
        base: "main",
    }
}

const HEAD: &str = "You are in a git worktree of /repo at /repo/.claude/worktrees/feat-x, on branch feat/x from main.";

#[test]
fn without_a_handoff() {
    assert_eq!(
        compose(&header(), None, "Do the thing.\n\n  ").unwrap(),
        format!("{HEAD}\n\nDo the thing.")
    );
    // A hand-off with nothing in it adds nothing.
    assert_eq!(
        compose(&header(), Some(&Handoff::default()), "Go").unwrap(),
        format!("{HEAD}\n\nGo")
    );
}

#[test]
fn with_a_name_and_every_fact() {
    let plans = [PathBuf::from("/home/.claude/plans/p.md")];
    let uncommitted = [" M src/a.rs".to_owned(), "?? notes.txt".to_owned()];
    let touched = [PathBuf::from("/repo/src/a.rs"), PathBuf::from("/repo/b.rs")];
    let handoff = Handoff {
        name: Some("lead"),
        transcript: Some(Path::new("/home/.claude/projects/-repo/1.jsonl")),
        plans: &plans,
        uncommitted: &uncommitted,
        touched: &touched,
    };
    let expected = format!(
        "{HEAD}\n\
This work was handed to you by session lead; report back with SendMessage (to: lead).\n\
\n\
Hand-off facts from that session:\n\
- Its transcript: /home/.claude/projects/-repo/1.jsonl\n\
- Its plan files:\n  /home/.claude/plans/p.md\n\
- Its uncommitted changes, which did not come along:\n   M src/a.rs\n  ?? notes.txt\n\
- Files it touched recently:\n  /repo/src/a.rs\n  /repo/b.rs\n\
\n\
The brief."
    );
    assert_eq!(
        compose(&header(), Some(&handoff), "The brief.").unwrap(),
        expected
    );
}

#[test]
fn a_name_without_facts() {
    let handoff = Handoff {
        name: Some("lead"),
        ..Handoff::default()
    };
    let text = compose(&header(), Some(&handoff), "b").unwrap();
    assert_eq!(
        text,
        format!(
            "{HEAD}\nThis work was handed to you by session lead; report back with SendMessage (to: lead).\n\nb"
        )
    );
}

#[test]
fn control_characters_are_escaped() {
    let touched = [PathBuf::from("/repo/a\u{1b}[2J\nb")];
    let handoff = Handoff {
        name: Some("x\u{202e}y"),
        touched: &touched,
        ..Handoff::default()
    };
    let text = compose(&header(), Some(&handoff), "b").unwrap();
    assert!(text.contains("session x\\u{202e}y;"));
    assert!(text.contains("  /repo/a\\u{1b}[2J\\u{a}b\n"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn bad_briefs_are_refused() {
    assert_eq!(compose(&header(), None, ""), Err(BriefError::Empty));
    assert_eq!(compose(&header(), None, " \n\t"), Err(BriefError::Empty));
    assert_eq!(compose(&header(), None, "a\0b"), Err(BriefError::Nul));
    assert_eq!(BriefError::Empty.to_string(), "The brief is empty");
    assert_eq!(BriefError::Nul.to_string(), "The brief holds a NUL byte");
    assert_eq!(BriefError::TooLong.to_string(), "The brief is over 64 KiB");
}

#[test]
fn the_limit_is_exact() {
    let room = MAX_BRIEF - HEAD.len() - 2;
    let brief = "x".repeat(room);
    let text = compose(&header(), None, &brief).unwrap();
    assert_eq!(text.len(), MAX_BRIEF);
    let over = "x".repeat(room + 1);
    assert_eq!(compose(&header(), None, &over), Err(BriefError::TooLong));
    // Trailing whitespace does not count.
    assert!(compose(&header(), None, &format!("{brief}   \n")).is_ok());
}

#[test]
fn facts_are_dropped_until_it_fits() {
    let touched: Vec<PathBuf> = (0..100)
        .map(|i| PathBuf::from(format!("/repo/file{i:03}.rs")))
        .collect();
    let uncommitted: Vec<String> = (0..10).map(|i| format!(" M u{i}")).collect();
    let plans = [PathBuf::from("/p.md")];
    let handoff = Handoff {
        name: None,
        transcript: Some(Path::new("/t.jsonl")),
        plans: &plans,
        uncommitted: &uncommitted,
        touched: &touched,
    };
    let full = compose(&header(), Some(&handoff), "b").unwrap();
    let excess = 30 * 21;
    let brief = "b".repeat(MAX_BRIEF - full.len() + 1 + excess);
    let text = compose(&header(), Some(&handoff), &brief).unwrap();
    assert!(text.len() <= MAX_BRIEF);
    // Touched files go first, from the end; the rest stay.
    assert!(text.contains("/repo/file000.rs"));
    assert!(!text.contains("/repo/file099.rs"));
    let more = text.lines().find(|line| line.contains("more")).unwrap();
    assert!(more.starts_with("  … and ") && more.ends_with(" more"));
    assert_eq!(text.matches("more").count(), 1);
    assert!(text.contains(" M u9") && text.contains("/p.md") && text.contains("/t.jsonl"));
}

#[test]
fn every_fact_may_go() {
    let touched = [PathBuf::from("/a")];
    let uncommitted = [" M b".to_owned()];
    let plans = [PathBuf::from("/c.md")];
    let handoff = Handoff {
        name: Some("n"),
        transcript: Some(Path::new("/t")),
        plans: &plans,
        uncommitted: &uncommitted,
        touched: &touched,
    };
    let bare = compose(
        &header(),
        Some(&Handoff {
            name: Some("n"),
            ..Handoff::default()
        }),
        "b",
    )
    .unwrap();
    let brief = "b".repeat(MAX_BRIEF - bare.len() + 1);
    let text = compose(&header(), Some(&handoff), &brief).unwrap();
    assert_eq!(text.len(), MAX_BRIEF);
    assert!(!text.contains("Hand-off facts"));
    assert!(text.contains("report back"));
    // One byte short of the transcript's line: only the transcript is left.
    let with_transcript =
        bare.len() + "\n\nHand-off facts from that session:\n- Its transcript: /t".len();
    let brief = "b".repeat(MAX_BRIEF - with_transcript + 1);
    let text = compose(&header(), Some(&handoff), &brief).unwrap();
    assert_eq!(text.len(), MAX_BRIEF);
    assert!(text.contains("- Its transcript: /t\n\n"));
    assert!(!text.contains("plan files"));
}

proptest! {
    #[test]
    fn any_input_is_within_the_limit(
        brief in ".{0,200}",
        lines in proptest::collection::vec(".{0,40}", 0..50),
        name in proptest::option::of(".{0,10}"),
        pad in 0usize..MAX_BRIEF,
    ) {
        let touched: Vec<PathBuf> = lines.iter().map(PathBuf::from).collect();
        let handoff = Handoff {
            name: name.as_deref(),
            transcript: Some(Path::new("/t")),
            plans: &touched,
            uncommitted: &lines,
            touched: &touched,
        };
        let brief = format!("{brief}{}", "z".repeat(pad));
        if let Ok(text) = compose(&header(), Some(&handoff), &brief) {
            prop_assert!(text.len() <= MAX_BRIEF);
            let escape = '\u{1b}';
            prop_assert!(!text.contains(escape) || brief.contains(escape));
        }
    }
}
