//! Tests of what a remote's runs need that runs no git: the tracking read, a push's arguments,
//! its question, `REMOTE/BRANCH`, branch names, progress and why one failed, with property
//! tests over any bytes and any text.
//!
//! Must not depend on: `app`, `dispatch` or winit; nothing here runs git.

use oso_core::editor::PushSwitches;
use proptest::prelude::*;

use super::{End, Push, Tracking, Wanted, check_branch, failed, progress, split_target, wanted};

fn push(switches: PushSwitches) -> Push {
    Push {
        branch: "topic".into(),
        head: "abc".into(),
        remote: "origin".into(),
        there: "refs/heads/topic".into(),
        set_upstream: false,
        switches,
        lease: None,
        dropped: Some((Vec::new(), 0)),
    }
}

#[test]
fn the_tracking_read_names_where_the_branch_pushes_and_pulls() {
    let out = b"fork\0refs/heads/topic\0refs/remotes/fork/topic\0origin\0refs/heads/main\0refs/remotes/origin/main\n";
    let tracking = Tracking::parse(out);
    assert_eq!(
        tracking.push,
        Some(End {
            remote: "fork".into(),
            there: Some("refs/heads/topic".into()),
            track: Some("refs/remotes/fork/topic".into()),
        })
    );
    assert_eq!(
        tracking.upstream.and_then(|end| end.there).as_deref(),
        Some("refs/heads/main")
    );
    // No push target and no upstream: both empty.
    assert_eq!(Tracking::parse(b"\0\0\0\0\0\n"), Tracking::default());
    // `push.default=simple` leaves the ref there empty: the tracking ref names it.
    let simple = Tracking::parse(b"origin\0\0refs/remotes/origin/topic\0\0\0\n");
    assert_eq!(
        simple.push.and_then(|end| end.there).as_deref(),
        Some("refs/heads/topic")
    );
    // A ref there that is not a branch is not taken; nor is a wrong count of parts.
    let odd = Tracking::parse(b"o\0main\0\0\0\0\n").push;
    assert_eq!(odd.map(|end| end.there), Some(None));
    assert_eq!(Tracking::parse(b"o\0refs/heads/a\n"), Tracking::default());
}

#[test]
fn a_push_names_its_refs_in_full_after_its_remote() {
    let plain = push(PushSwitches::default());
    assert_eq!(
        plain.args(),
        [
            "push",
            "-v",
            "--progress",
            "--",
            "origin",
            "refs/heads/topic:refs/heads/topic"
        ]
    );
    let mut forced = push(PushSwitches {
        force_with_lease: true,
        no_verify: true,
    });
    forced.set_upstream = true;
    forced.lease = Some("abc123".into());
    let args = forced.args();
    assert!(args.contains(&"--no-verify".to_owned()));
    assert!(args.contains(&"--set-upstream".to_owned()));
    assert!(args.contains(&"--force-with-lease=refs/heads/topic:abc123".to_owned()));
    assert!(!args.iter().any(|arg| arg == "--force" || arg == "-f"));
    // A lease on a branch oso saw none of says the remote must have none.
    forced.lease = Some(String::new());
    assert!(
        forced
            .args()
            .contains(&"--force-with-lease=refs/heads/topic:".to_owned())
    );
    assert_eq!(forced.destination(), "origin/topic");
    assert_eq!(forced.title(), "git push --force-with-lease origin/topic");
}

#[test]
fn a_forced_push_s_question_names_what_it_drops() {
    let mut forced = push(PushSwitches {
        force_with_lease: true,
        no_verify: false,
    });
    assert!(forced.question().contains("drops nothing"));
    // When git could not count them, it never says it drops nothing.
    forced.dropped = None;
    assert!(
        forced.question().contains("could not tell"),
        "{}",
        forced.question()
    );
    forced.dropped = Some((Vec::new(), 3));
    assert!(forced.question().ends_with("dropping 3 commits there?"));
    forced.dropped = Some((vec!["abc first".into(), "def second".into()], 7));
    let question = forced.question();
    assert!(question.contains("dropping 7 commits"), "{question}");
    assert!(
        question.contains("abc first; def second, and 5 more"),
        "{question}"
    );
}

#[test]
fn remote_branch_answers_split_at_the_longest_remote() {
    let remotes = ["origin".to_owned(), "origin/mirror".to_owned()];
    assert_eq!(
        split_target("origin/topic", &remotes),
        Ok(("origin", "topic"))
    );
    assert_eq!(
        split_target("origin/mirror/topic", &remotes),
        Ok(("origin/mirror", "topic"))
    );
    assert_eq!(
        split_target("origin/feature/x", &remotes[..1]),
        Ok(("origin", "feature/x"))
    );
    assert!(split_target("elsewhere/topic", &remotes).is_err());
    assert!(split_target("origin/", &remotes).is_err());
    assert!(split_target("origin/-f", &remotes).is_err());
}

#[test]
fn branch_names_git_would_refuse_are_refused() {
    for good in ["main", "feature/x", "a.b", "v1.2-rc"] {
        assert!(check_branch(good).is_ok(), "{good}");
    }
    for bad in [
        "",
        "-f",
        "/a",
        "a/",
        "a.",
        "a.lock",
        "a..b",
        "a@{1}",
        "a//b",
        "a b",
        "a:b",
        "a~1",
        "a^",
        "a?",
        "a*",
        "a[",
        "a\\b",
        "@",
        ".a",
        "a/.b",
        "a\u{7}",
        "a.lock/b",
        "a\u{202e}b",
        "a\u{2066}b",
    ] {
        assert!(check_branch(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn an_address_shows_without_its_user_and_password() {
    use super::shown_remote;
    assert_eq!(shown_remote("origin"), "origin");
    assert_eq!(
        shown_remote("https://user:secret@example.invalid/x.git"),
        "https://example.invalid/x.git"
    );
    assert_eq!(
        shown_remote("ssh://git@example.invalid/x.git"),
        "ssh://example.invalid/x.git"
    );
    assert_eq!(
        shown_remote("https://example.invalid/a@b"),
        "https://example.invalid/a@b"
    );
}

#[test]
fn a_ref_there_that_is_not_a_branch_name_is_not_taken() {
    let out = b"o\0refs/heads/a:b\0\0\0\0\n";
    assert_eq!(Tracking::parse(out).push.map(|end| end.there), Some(None));
    let bidi = "o\0refs/heads/a\u{202e}b\0\0\0\0\n";
    assert_eq!(
        Tracking::parse(bidi.as_bytes()).push.map(|end| end.there),
        Some(None)
    );
}

#[test]
fn progress_is_the_last_phase_with_a_percentage() {
    let out = b"Enumerating objects: 5, done.\nCounting objects:  40% (2/5)\rCounting objects: 100% (5/5), done.\nWriting objects:  42% (3/7)\r";
    assert_eq!(progress(out).as_deref(), Some("Writing objects 42%"));
    // The server's own lines are not echoed: the last of git's own is.
    assert_eq!(
        progress(b"Writing objects:  50% (1/2)\nremote: Resolving deltas:  12% (1/8)\r").as_deref(),
        Some("Writing objects 50%")
    );
    assert_eq!(progress(b"To example\n   abc..def  main -> main\n"), None);
    assert_eq!(progress(b"100% done\n"), None);
    assert_eq!(progress(b"phase: 1000%\n"), None);
    assert_eq!(progress(b"\x1b[31m\x1b: 5%\n").as_deref(), Some("[31m 5%"));
}

#[test]
fn a_failure_for_want_of_a_password_or_a_key_says_how_to_get_past_it() {
    let password =
        "fatal: could not read Username for 'https://example': terminal prompts disabled\n";
    assert_eq!(wanted(password), Some(Wanted::Credentials));
    assert!(failed("git push origin/main", password).contains("credential helper"));
    let key = "git@example: Permission denied (publickey).\nfatal: Could not read from remote repository.\n";
    assert_eq!(wanted(key), Some(Wanted::Credentials));
    let host = "Host key verification failed.\nfatal: Could not read from remote repository.\n";
    assert_eq!(wanted(host), Some(Wanted::HostKey));
    let server = "fatal: remote error: anything the server says\n";
    assert_eq!(
        failed("git fetch", server),
        "git fetch failed: $ shows the output"
    );
    let rejected = "To example\n ! [rejected]        main -> main (fetch first)\nerror: failed to push some refs\n";
    assert_eq!(wanted(rejected), None);
    assert_eq!(
        failed("git push origin/main", rejected),
        "git push origin/main failed: error: failed to push some refs; $ shows the output"
    );
    assert_eq!(
        failed("git fetch", "nothing to see\n"),
        "git fetch failed: $ shows the output"
    );
}

proptest! {
    #[test]
    fn the_readers_take_any_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Tracking::parse(&bytes);
        if let Some(shown) = progress(&bytes) {
            prop_assert!(shown.ends_with('%'));
            prop_assert!(!shown.chars().any(char::is_control));
            prop_assert!(shown.chars().count() <= 65);
        }
    }

    #[test]
    fn the_text_readers_take_any_text(text in "\\PC{0,200}", remote in "[a-z/]{1,8}") {
        let _ = wanted(&text);
        let said = failed("git push", &text);
        prop_assert!(!said.contains('\n'));
        if let Ok((found, branch)) = split_target(&text, std::slice::from_ref(&remote)) {
            prop_assert_eq!(found, remote.as_str());
            prop_assert!(check_branch(branch).is_ok());
        }
    }
}
