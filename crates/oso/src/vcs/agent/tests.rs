//! The texts a commit asks its worktree's session with, and what a draft becomes: the fence
//! no output can end, the end of a long output, a draft cleaned as a paste is, and the
//! person's own text kept as `#` lines.
//!
//! Must not depend on: `app`, sessions or git; these are the texts' own tests.

use oso_core::editor::CommitKind;
use proptest::prelude::*;

use super::{
    MAX_DRAFT, MAX_FIX_BYTES, MAX_FIX_LINES, MAX_KEPT, cleaned, draft_request, fence_for,
    fix_request, keep_end, last_lines, with_draft,
};
use crate::vcs::commit::{message_end, message_of};

#[test]
fn a_draft_request_names_the_worktree_the_subjects_and_the_id() {
    let subjects = ["Add a\u{1b}[31m", "Fix b", "c", "d", "e", "f"].map(String::from);
    let text = draft_request(CommitKind::New, "/w/oso", &subjects, "1a2b3c4d");
    assert!(text.starts_with("Draft a commit message for the staged changes in /w/oso"));
    assert!(text.contains("- Fix b\n"));
    assert!(
        text.contains("the repository's text, are:\n```\n- Add a"),
        "{text}"
    );
    // A worktree or subject that would reorder the request is escaped.
    let path = draft_request(
        CommitKind::New,
        &super::one_line("/w/\u{202e}x", 400),
        &[],
        "ab",
    );
    assert!(!path.contains('\u{202e}'), "{path}");
    // Five subjects, each on one line with its control characters escaped.
    assert!(!text.contains("- f\n"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(
        text.ends_with("draft_set (kind: commit, id: 1a2b3c4d)."),
        "{text}"
    );
    let none = draft_request(CommitKind::Reword, "/w", &[], "ab");
    assert!(
        none.contains("the last commit in /w (`git show HEAD`)"),
        "{none}"
    );
    assert!(none.contains("- (none yet)\n"), "{none}");
}

#[test]
fn a_fix_request_holds_the_output_in_a_fence_and_names_the_hook() {
    let text = fix_request("/w", &["pre-commit"], "$ git commit\nlint: a.rs:3 bad\n").0;
    assert_eq!(
        text,
        "`git commit` failed in /w (hook pre-commit). Its output, last 200 lines:\n```\n$ git commit\nlint: a.rs:3 bad\n```\nFix the cause. Do not commit, do not skip hooks, and do not amend; I will commit again."
    );
    let two = fix_request("/w", &["pre-commit", "commit-msg"], "x\n").0;
    assert!(two.contains("(hook pre-commit or commit-msg)"), "{two}");
    assert!(
        fix_request("/w", &[], "x")
            .0
            .starts_with("`git commit` failed in /w. Its")
    );
}

#[test]
fn lines_shaped_like_secrets_are_left_out_of_a_fix_request_and_counted() {
    // Made from pieces, so that no secret scanner takes this file for one.
    let piece = |parts: &[&str]| parts.concat();
    let key = piece(&["PRIVATE", " KEY-----"]);
    let output = [
        "$ git commit\n".to_owned(),
        piece(&["gitleaks: Finding: AWS_KEY=", "AKIA", "ABCDEFGHIJKLMNOP\n"]),
        piece(&[
            "tok",
            "en = \"",
            "ghp_",
            "0123456789abcdefghijABCDEFGHIJ012345\"\n",
        ]),
        piece(&["-----BEGIN OPENSSH ", &key, "\n"]),
        "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\n".to_owned(),
        piece(&["-----END OPENSSH ", &key, "\n"]),
        piece(&["pass", "word: ", "hunter2hunter2\n"]),
        piece(&[
            "Authorization: ",
            "eyJ",
            "hbGciOiJIUzI1NiJ9.",
            "eyJ",
            "zdWIiOiIxMjM0NSJ9.c2lnbmF0dXJl\n",
        ]),
        "lint: a.rs:3 bad\n".to_owned(),
        "commit 3f0e91cbd2c1a0ffe1e2d3c4b5a69788a0b1c2d3 is fine\n".to_owned(),
        piece(&["aB3dE5fG7hJ9", "kL1mN3pQ5rS7tU9vW1xY3z\n"]),
    ]
    .concat();
    let (text, dropped) = fix_request("/w", &["pre-commit"], &output);
    // No message names the text: it would print what looks like a secret.
    assert_eq!(dropped, 8);
    assert!(text.contains("$ git commit\nlint: a.rs:3 bad\ncommit 3f0e91cb"));
    let found = [
        "AKIA",
        "ghp_",
        "PRIVATE KEY",
        "b3BlbnNz",
        "hunter2",
        "eyJhbG",
        "aB3dE5",
    ]
    .iter()
    .filter(|shape| text.contains(*shape))
    .count();
    assert_eq!(found, 0);
    assert!(text.contains("```\n8 lines that may hold secrets were left out.\nFix"));
    assert!(super::withhold::pattern().is_some());
    let (plain, none) = fix_request("/w", &[], "error: tests failed\n");
    assert_eq!(none, 0);
    assert!(!plain.contains("left out"), "{plain}");
}

#[cfg(unix)]
#[test]
fn an_id_is_eight_hex_digits_and_they_differ() {
    let (one, two) = (super::request_id().unwrap(), super::request_id().unwrap());
    assert_eq!(one.len(), 8);
    assert!(one.chars().all(|c| c.is_ascii_hexdigit()), "{one}");
    assert_ne!(one, two);
}

#[cfg(not(unix))]
#[test]
fn an_id_is_refused_with_why_where_oso_has_no_random_bytes() {
    let why = super::request_id().unwrap_err();
    assert!(why.contains("only on Unix yet"), "{why}");
}

#[test]
fn the_fence_is_longer_than_any_run_of_backticks() {
    assert_eq!(fence_for("plain"), "```");
    assert_eq!(fence_for("a ``` b"), "````");
    assert_eq!(fence_for("`````"), "``````");
    // A crafted run is cut in the request, so that its fence stays short.
    let text = fix_request("/w", &[], &"`".repeat(64 * 1024)).0;
    assert!(text.len() < 400, "{}", text.len());
    assert!(text.contains(&format!("\n{}\n", "`".repeat(16))), "{text}");
}

#[test]
fn the_end_kept_of_a_run_is_whole_lines_or_one_long_lines_end() {
    let mut kept = "one\ntwo\nthree\n".to_owned();
    keep_end(&mut kept, 10);
    assert_eq!(kept, "three\n");
    let mut long = format!("x\n{}\n", "y".repeat(300));
    keep_end(&mut long, 100);
    assert_eq!(long.len(), 100);
    assert!(long.ends_with("yy\n"));
}

#[test]
fn a_long_output_keeps_its_last_lines_and_bytes() {
    let lines: String = (0..500).map(|n| format!("line {n}\n")).collect();
    let end = last_lines(&lines, MAX_FIX_LINES, MAX_FIX_BYTES);
    assert!(end.starts_with("line 300\n"), "{}", &end[..20]);
    assert!(end.ends_with("line 499\n"));
    assert_eq!(last_lines("one\ntwo", 1, 100), "two");
    assert_eq!(last_lines("one\ntwo\n", 5, 100), "one\ntwo\n");
    // By bytes, from a line's start; a line longer than the whole keeps its end.
    assert_eq!(last_lines("aaaa\nbbbb\ncc\n", 10, 8), "cc\n");
    assert_eq!(last_lines("abcdefghij", 10, 4), "ghij");
    assert_eq!(last_lines("", 10, 4), "");
}

#[test]
fn a_draft_is_cleaned_as_a_paste_and_bounded() {
    assert_eq!(
        cleaned("Fix it\r\n\r\nBody\u{202e}\u{7}\tok\n\n\n").unwrap(),
        "Fix it\n\nBody\tok"
    );
    assert!(cleaned(" \n\u{1b}\n").is_err());
    // Nothing hides below the screen or between the letters; an emoji keeps its joiners.
    assert_eq!(
        cleaned("\n\nFix\n\n \n\t\n\n\nhid\u{200b}den\u{e0041}\n").unwrap(),
        "Fix\n\nhidden"
    );
    assert_eq!(
        cleaned("👩\u{200d}💻\u{fe0f}").unwrap(),
        "👩\u{200d}💻\u{fe0f}"
    );
    assert!(cleaned(&"x".repeat(MAX_DRAFT + 1)).is_err());
    assert!(cleaned(&"x".repeat(MAX_DRAFT)).is_ok());
}

#[test]
fn a_draft_takes_the_place_of_the_text_and_keeps_the_persons_as_comments() {
    let now = "My own words\nmore\n\n# Please enter the message\n# On branch x\n";
    let (end, put) = with_draft(now, "Fix the parser", "api\u{1b}").unwrap();
    assert_eq!(end, "My own words\nmore\n\n".len());
    assert_eq!(
        put,
        "Fix the parser\n\n# your text before the draft from api\\u{1b}:\n# My own words\n# more\n#\n"
    );
    let after = format!("{put}{}", &now[end..]);
    // The person's words are not committed unless taken back.
    assert_eq!(message_of(&after), "Fix the parser\n");
    let (end, put) = with_draft("\n\n# Please enter\n", "Fix", "api").unwrap();
    assert_eq!((end, put.as_str()), (2, "Fix\n\n"));
    // Past 64 KiB of the person's own text, a draft is refused, so that nothing doubles.
    let crafted = format!("{}# Please enter\n", "a\n".repeat(MAX_KEPT));
    assert!(with_draft(&crafted, "Fix", "api").is_err());
    let kept = format!("{}\n# Please enter\n", "a".repeat(MAX_KEPT));
    assert!(with_draft(&kept, "Fix", "api").is_ok());
}

proptest! {
    #[test]
    fn no_output_ends_the_fence_early(output in "[`a\\n]{0,80}") {
        let text = fix_request("/w", &[], &output).0;
        let fence = fence_for(last_lines(&output, MAX_FIX_LINES, MAX_FIX_BYTES));
        // The fence opens and closes the output, and no line of the output is the fence.
        prop_assert_eq!(text.matches(&format!("\n{fence}\n")).count(), 2);
        prop_assert!(!output.lines().any(|line| line.starts_with(&fence)));
    }

    #[test]
    fn the_end_kept_is_a_suffix_within_bounds(
        output in "[ab\\n]{0,400}",
        lines in 1usize..20,
        bytes in 1usize..100,
    ) {
        let end = last_lines(&output, lines, bytes);
        prop_assert!(output.ends_with(end));
        prop_assert!(end.len() <= bytes || end.lines().count() <= 1);
        prop_assert!(end.trim_end_matches('\n').matches('\n').count() < lines);
    }

    #[test]
    fn a_draft_always_ends_above_the_comment_lines(
        mine in "[a#\\n ]{0,60}",
        draft in "[a-z \\n]{1,40}",
    ) {
        let now = format!("{mine}\n# Please enter\n");
        let Ok(draft) = cleaned(&draft) else { return Ok(()); };
        let (end, put) = with_draft(&now, &draft, "s").unwrap();
        let after = format!("{put}{}", &now[end..]);
        prop_assert!(after.starts_with(&draft));
        prop_assert_eq!(message_end(&after) >= draft.len(), true);
    }
}
