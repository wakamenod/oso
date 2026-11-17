//! A pull request's message and runs: what it starts from, its title and body, and `gh`'s
//! arguments, every value bound to its flag.
//!
//! Must not depend on: `gh` or git.

use oso_core::editor::MergeMethod;
use proptest::prelude::*;

use super::{BranchCommit, PrRun, draft_text, log_line, read_commits, title_and_body};

fn commit(subject: &str, message: &str) -> BranchCommit {
    BranchCommit {
        subject: subject.into(),
        message: message.into(),
    }
}

#[test]
fn a_message_starts_from_the_single_commit_or_the_subjects_then_the_template() {
    let one = [commit("Add x", "Add x\n\nWhy x.\n")];
    assert_eq!(draft_text(&one, None), "Add x\n\nWhy x.\n");
    assert_eq!(
        draft_text(&one, Some("## Checklist\n")),
        "Add x\n\nWhy x.\n\n## Checklist\n"
    );
    // Newest first, as git lists them; the message lists them oldest first.
    let two = [commit("Second", "Second"), commit("First", "First")];
    assert_eq!(draft_text(&two, None), "First\n\n- First\n- Second\n");
    assert_eq!(draft_text(&[], Some("T")), "\n\nT\n");
}

#[test]
fn a_message_is_its_first_line_and_the_rest_with_nothing_stripped() {
    let (title, body) = title_and_body("  Add x \n\n## Why\n# not a comment\n\n").unwrap();
    assert_eq!(title, "Add x");
    assert_eq!(body, "## Why\n# not a comment");
    assert!(title_and_body("\nbody").is_err());
    assert!(title_and_body(&"t".repeat(257)).is_err());
    assert!(title_and_body(&"t".repeat(256)).is_ok());
}

#[test]
fn every_value_goes_bound_to_its_flag_and_the_body_on_standard_input() {
    let create = PrRun::Create {
        repo: "github.com/o/r".into(),
        base: "main".into(),
        head: "-x".into(),
        title: "--draft".into(),
        body: "Body".into(),
        draft: false,
    };
    let (args, input) = create.args();
    assert_eq!(
        args,
        [
            "pr",
            "create",
            "--repo=github.com/o/r",
            "--title=--draft",
            "--body-file=-",
            "--base=main",
            "--head=-x"
        ]
    );
    assert_eq!(input.as_deref(), Some(b"Body".as_slice()));
    let merge = PrRun::Merge {
        repo: "github.com/o/r".into(),
        number: 12,
        method: MergeMethod::Squash,
        base: "main".into(),
        head_commit: "abc".into(),
        delete_branch: true,
    };
    let (args, input) = merge.args();
    assert_eq!(
        args,
        [
            "pr",
            "merge",
            "12",
            "--repo=github.com/o/r",
            "--squash",
            "--match-head-commit=abc",
            "--delete-branch"
        ]
    );
    assert!(input.is_none());
    assert_eq!(merge.done(), "Merged #12");
}

#[test]
fn a_branch_s_commits_read_from_git_s_records() {
    let out = b"Second\x1fSecond\n\nwhy\n\0\nFirst\x1fFirst\n\0no separator\0";
    let commits = read_commits(out);
    assert_eq!(commits.len(), 2, "a record of another shape is left out");
    assert_eq!(
        (commits[0].subject.as_str(), commits[1].subject.as_str()),
        ("Second", "First")
    );
    assert_eq!(commits[0].message, "Second\n\nwhy\n");
}

#[test]
fn the_log_line_quotes_each_value() {
    let args = ["pr", "create", "--title=Fix --base=main", "--head=x"].map(String::from);
    assert_eq!(
        log_line(&args),
        "$ gh pr create --title='Fix --base=main' --head='x'\n"
    );
}

proptest! {
    #[test]
    fn any_git_output_reads_without_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        prop_assert!(read_commits(&bytes).len() <= super::MAX_COMMITS);
    }

    #[test]
    fn any_message_reads_without_a_panic(text in any::<String>()) {
        if let Ok((title, _)) = title_and_body(&text) {
            prop_assert!(!title.is_empty() && !title.contains('\n'));
        }
    }
}
