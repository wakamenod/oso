//! The texts a commit asks its worktree's session with, and what a draft becomes: the fence
//! no output can end, the end of a long output, a draft cleaned as a paste is, and the
//! person's own text kept as `#` lines.
//!
//! Must not depend on: `app`, sessions or git; these are the texts' own tests.

use oso_core::editor::CommitKind;
use proptest::prelude::*;

use super::{
    MAX_DRAFT, MAX_FIX_BYTES, MAX_FIX_LINES, cleaned, draft_request, fence_for, fix_request,
    last_lines, with_draft,
};
use crate::vcs::commit::{message_end, message_of};

#[test]
fn a_draft_request_names_the_worktree_the_subjects_and_the_id() {
    let subjects = ["Add a\u{1b}[31m", "Fix b", "c", "d", "e", "f"].map(String::from);
    let text = draft_request(CommitKind::New, "/w/oso", &subjects, "1a2b3c4d");
    assert!(text.starts_with("Draft a commit message for the staged changes in /w/oso"));
    assert!(text.contains("- Fix b\n"));
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
    let text = fix_request("/w", &["pre-commit"], "$ git commit\nlint: a.rs:3 bad\n");
    assert_eq!(
        text,
        "`git commit` failed in /w (hook pre-commit). Its output, last 200 lines:\n```\n$ git commit\nlint: a.rs:3 bad\n```\nFix the cause. Do not commit, do not skip hooks, and do not amend; I will commit again."
    );
    let two = fix_request("/w", &["pre-commit", "commit-msg"], "x\n");
    assert!(two.contains("(hook pre-commit or commit-msg)"), "{two}");
    assert!(fix_request("/w", &[], "x").starts_with("`git commit` failed in /w. Its"));
}

#[test]
fn the_fence_is_longer_than_any_run_of_backticks() {
    assert_eq!(fence_for("plain"), "```");
    assert_eq!(fence_for("a ``` b"), "````");
    assert_eq!(fence_for("`````"), "``````");
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
    assert!(cleaned(&"x".repeat(MAX_DRAFT + 1)).is_err());
    assert!(cleaned(&"x".repeat(MAX_DRAFT)).is_ok());
    assert_eq!(MAX_DRAFT, crate::channel::protocol::sessions::MAX_DRAFT);
}

#[test]
fn a_draft_takes_the_place_of_the_text_and_keeps_the_persons_as_comments() {
    let now = "My own words\nmore\n\n# Please enter the message\n# On branch x\n";
    let (end, put) = with_draft(now, "Fix the parser", "api\u{1b}");
    assert_eq!(end, "My own words\nmore\n\n".len());
    assert_eq!(
        put,
        "Fix the parser\n\n# your text before the draft from api\\u{1b}:\n# My own words\n# more\n#\n"
    );
    let after = format!("{put}{}", &now[end..]);
    // The person's words are not committed unless taken back.
    assert_eq!(message_of(&after), "Fix the parser\n");
    let (end, put) = with_draft("\n\n# Please enter\n", "Fix", "api");
    assert_eq!((end, put.as_str()), (2, "Fix\n\n"));
}

proptest! {
    #[test]
    fn no_output_ends_the_fence_early(output in "[`a\\n]{0,80}") {
        let text = fix_request("/w", &[], &output);
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
        let (end, put) = with_draft(&now, &draft, "s");
        let after = format!("{put}{}", &now[end..]);
        prop_assert!(after.starts_with(&draft));
        prop_assert_eq!(message_end(&after) >= draft.len(), true);
    }
}
