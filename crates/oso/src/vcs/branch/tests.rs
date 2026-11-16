//! Tests of branches' pure parts: the names listed, what each run's arguments are, the
//! worktree a branch is checked out in, starts refused, and the delete's question, escaped.
//!
//! Must not depend on: git or the file system.

use super::{
    check_start, checked_out, checked_out_in, checkout_args, create_args, delete_args,
    delete_question, failed, names, rename_args,
};

#[test]
fn names_leave_out_remote_heads_and_what_could_read_as_an_option() {
    let out = b"main\nfeature/x\norigin/HEAD\n-x\norigin/main\n\n";
    assert_eq!(names(out, 10), ["main", "feature/x", "origin/main"]);
    assert_eq!(names(out, 1), ["main"]);
}

#[test]
fn each_run_says_what_git_does() {
    assert_eq!(
        checkout_args("main", false),
        ["switch", "--no-guess", "main"]
    );
    assert_eq!(
        checkout_args("origin/x", true),
        ["switch", "--track", "refs/remotes/origin/x"]
    );
    assert_eq!(
        create_args("x", "HEAD", true),
        ["switch", "--create", "x", "HEAD"]
    );
    assert_eq!(create_args("x", "main", false), ["branch", "x", "main"]);
    assert_eq!(rename_args("a", "b"), ["branch", "--move", "a", "b"]);
    assert_eq!(delete_args("x", false), ["branch", "-d", "x"]);
    assert_eq!(delete_args("x", true), ["branch", "-D", "x"]);
}

#[test]
fn a_start_that_could_read_as_an_option_is_refused() {
    assert!(check_start("main~2").is_ok());
    for bad in ["", "--orphan", "a\nb"] {
        assert!(check_start(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn the_worktree_a_branch_is_checked_out_in_is_found() {
    let out = b"worktree /r\0HEAD 1111\0branch refs/heads/main\0\0\
                worktree /r/wt\0HEAD 2222\0branch refs/heads/side\0\0\
                worktree /r/bare\0bare\0\0";
    assert_eq!(checked_out_in(out, "side").as_deref(), Some("/r/wt"));
    assert_eq!(checked_out_in(out, "main").as_deref(), Some("/r"));
    assert_eq!(checked_out_in(out, "mai"), None);
    let said = checked_out("side", "/r/w\u{202e}t", false);
    assert!(said.contains("worktree-remove"), "{said}");
    assert!(!said.contains('\u{202e}'), "{said:?}");
    assert!(checked_out("main", "/r", true).contains("checked out here"));
}

#[test]
fn the_delete_question_names_the_commits_only_it_holds() {
    let named = vec!["1234567 one".to_owned(), "89abcde two".to_owned()];
    let question = delete_question("x", Some(&(named, 7)));
    assert!(
        question.starts_with("Delete x? It holds 7 commits"),
        "{question}"
    );
    assert!(question.ends_with(", …"), "{question}");
    let none = delete_question("x", Some(&(Vec::new(), 0)));
    assert!(
        none.contains("each of its commits is on another branch"),
        "{none}"
    );
    // When git could not say, the question does not reassure.
    let unknown = delete_question("x", None);
    assert!(unknown.contains("could not tell"), "{unknown}");
}

#[test]
fn a_branch_used_by_another_worktree_points_at_worktree_remove() {
    let said = failed(
        "git switch side",
        "fatal: 'side' is already used by worktree at '/r/wt'\n",
    );
    assert!(said.contains("worktree-remove"), "{said}");
}
