//! A repository's run thread with a remote, a bare repository beside it: where a push goes
//! with and without a target, setting the upstream, a forced push whose lease git refuses once
//! the remote moved, fetching and pulling fast-forward only, fixup and squash commits, and a
//! failure for want of a key.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::sync::mpsc;
use std::time::Duration;

use oso_core::editor::{CommitKind, CommitSwitches, FetchFrom, PushSwitches};

use super::super::{Planned, RemoteRun, RunEnd, RunReport, RunRequest, Want};
use super::{repo, runner, subject, until_end};
use crate::review::tests::Repo;

/// A repository with a first commit and an empty bare remote, `origin`, beside it.
fn with_remote(name: &str) -> (Repo, String) {
    let repo = repo(name);
    // Without Windows' verbatim prefix, which git cannot take.
    let bare = crate::git::for_git(&repo.dir.join("remote.git"));
    let bare = bare.to_str().unwrap().to_owned();
    repo.git(&["init", "-q", "--bare", "-b", "main", &bare]);
    repo.git(&["remote", "add", "origin", &bare]);
    (repo, bare)
}

/// What the next plan says.
fn planned(got: &mpsc::Receiver<RunReport>) -> Result<Planned, String> {
    loop {
        if let RunReport::Planned(planned) = got.recv_timeout(Duration::from_secs(30)).unwrap() {
            return planned;
        }
    }
}

fn plan(
    runner: &super::super::Runner,
    got: &mpsc::Receiver<RunReport>,
    want: Want,
    switches: PushSwitches,
) -> Result<Planned, String> {
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Plan { want, switches })));
    planned(got)
}

fn text(repo: &Repo, args: &[&str]) -> String {
    String::from_utf8(repo.git(args)).unwrap().trim().to_owned()
}

#[test]
fn a_branch_with_no_upstream_asks_for_a_remote_and_u_sets_it() {
    let (repo, _) = with_remote("run-push-upstream");
    let (runner, got) = runner(&repo);
    let none = PushSwitches::default();
    assert_eq!(
        plan(&runner, &got, Want::Upstream, none),
        Ok(Planned::Remote {
            remotes: vec!["origin".into()],
            set_upstream: true,
        })
    );
    let want = Want::Remote {
        remote: "origin".into(),
        set_upstream: true,
    };
    let Ok(Planned::Ready(push)) = plan(&runner, &got, want, none) else {
        panic!("no plan");
    };
    assert!(push.set_upstream);
    assert_eq!(push.destination(), "origin/main");
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Push {
        push,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(output.contains("--set-upstream -- origin refs/heads/main:refs/heads/main"));
    assert_eq!(
        text(&repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/main"
    );
    // Now `p` follows `@{push}`, which `push.default` makes the upstream.
    let Ok(Planned::Ready(push)) = plan(&runner, &got, Want::PushRemote, none) else {
        panic!("no push target");
    };
    assert_eq!(push.destination(), "origin/main");
    assert!(!push.set_upstream);
    // `e` lists the branch's own name on each remote first.
    let Ok(Planned::Branch { targets }) = plan(&runner, &got, Want::Elsewhere, none) else {
        panic!("no targets");
    };
    assert_eq!(targets.first().map(String::as_str), Some("origin/main"));
    // A remote that is not one, and a branch git would refuse, are refused.
    let want = Want::Remote {
        remote: "nowhere".into(),
        set_upstream: false,
    };
    assert!(plan(&runner, &got, want, none).is_err());
    assert!(plan(&runner, &got, Want::Branch("origin/-f".into()), none).is_err());
}

#[test]
fn a_forced_push_names_what_it_drops_and_git_refuses_it_once_the_remote_moved() {
    let (repo, bare) = with_remote("run-push-lease");
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    // Another clone moves the remote on.
    let other = crate::git::for_git(&repo.dir.join("other"));
    let other = other.to_str().unwrap();
    repo.git(&["clone", "-q", &bare, other]);
    std::fs::write(repo.dir.join("other/b.txt"), "b\n").unwrap();
    repo.git(&["-C", other, "add", "b.txt"]);
    repo.git(&["-C", other, "commit", "-q", "-m", "theirs"]);
    repo.git(&["-C", other, "push", "-q", "origin", "main"]);
    // Here, the last commit is made again, and the remote's move is not fetched.
    repo.git(&["commit", "-q", "--amend", "-m", "first, again"]);
    let (runner, got) = runner(&repo);
    let force = PushSwitches {
        force_with_lease: true,
        no_verify: false,
    };
    let Ok(Planned::Ready(push)) = plan(&runner, &got, Want::Upstream, force) else {
        panic!("no plan");
    };
    let (named, count) = push.dropped.clone().unwrap();
    assert_eq!(count, 1, "{named:?}");
    assert!(named[0].ends_with(" first"), "{named:?}");
    // A commit made after the question: the push it asked about is refused.
    let mut moved = push.clone();
    moved.head = "0".repeat(40);
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Push {
        push: moved,
        pass: Vec::new(),
    })));
    let (end, _, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Refused(why) if why.contains("moved since the question")),
        "{end:?}"
    );
    let lease = push.lease.clone().unwrap();
    assert_eq!(lease, text(&repo, &["rev-parse", "origin/main"]));
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Push {
        push,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Failed { said, .. } if said.starts_with("git push --force-with-lease origin/main failed")),
        "{end:?} {output}"
    );
    assert_eq!(
        String::from_utf8(repo.git(&["--git-dir", &bare, "log", "-1", "--format=%s"]))
            .unwrap()
            .trim(),
        "theirs"
    );
    // Fetched and pulled fast-forward only, it would go apart: git refuses that too.
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Fetch {
        from: FetchFrom::Upstream,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Pull { pass: Vec::new() })));
    let (end, _, _) = until_end(&got);
    assert!(matches!(end, RunEnd::Failed { .. }), "{end:?}");
    assert_eq!(subject(&repo), "first, again");
}

#[test]
fn a_fetch_and_a_fast_forward_pull_take_the_remote_s_commits() {
    let (repo, bare) = with_remote("run-pull");
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    let other = crate::git::for_git(&repo.dir.join("other"));
    let other = other.to_str().unwrap();
    repo.git(&["clone", "-q", &bare, other]);
    std::fs::write(repo.dir.join("other/b.txt"), "b\n").unwrap();
    repo.git(&["-C", other, "add", "b.txt"]);
    repo.git(&["-C", other, "commit", "-q", "-m", "theirs"]);
    repo.git(&["-C", other, "push", "-q", "origin", "main"]);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Fetch {
        from: FetchFrom::All,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(
        output.contains("$ git fetch --prune --progress --all"),
        "{output}"
    );
    assert_eq!(subject(&repo), "first");
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Pull { pass: Vec::new() })));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(subject(&repo), "theirs");
}

#[test]
fn a_fixup_and_a_squash_are_made_for_an_earlier_commit() {
    let repo = repo("run-fixup");
    let first = text(&repo, &["rev-parse", "HEAD"]);
    let (runner, got) = runner(&repo);
    for (kind, message, file, want) in [
        (CommitKind::Fixup, None, "b.txt", "fixup! first"),
        (
            CommitKind::Squash,
            Some(b"More of it\n".to_vec()),
            "c.txt",
            "squash! first",
        ),
    ] {
        std::fs::write(repo.root.join(file), "x\n").unwrap();
        repo.git(&["add", file]);
        assert!(runner.ask(RunRequest::Commit {
            kind,
            switches: CommitSwitches::default(),
            message,
            pass: Vec::new(),
            target: Some(first.clone()),
        }));
        let (end, output, _) = until_end(&got);
        assert_eq!(end, RunEnd::Done, "{output}");
        assert_eq!(subject(&repo), want);
    }
    let body = text(&repo, &["log", "-1", "--format=%b"]);
    assert_eq!(body, "More of it");
}

#[cfg(unix)]
#[test]
fn a_remote_that_wants_a_key_says_how_to_get_past_it() {
    let (repo, _) = with_remote("run-push-key");
    let ssh = repo.dir.join("fake-ssh");
    let session = repo.dir.join("session");
    // It says the state of its group's leader, git: `s` marks a session's leader.
    let script = format!(
        "#!/bin/sh\nps -o stat= -p $(ps -o pgid= -p $$) > '{}'\n\
         echo 'git@example: Permission denied (publickey).' >&2\nexit 255\n",
        session.display()
    );
    std::fs::write(&ssh, script).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "core.sshCommand", ssh.to_str().unwrap()]);
    repo.git(&[
        "remote",
        "set-url",
        "origin",
        "ssh://git@example.invalid/x.git",
    ]);
    let (runner, got) = runner(&repo);
    let want = Want::Remote {
        remote: "origin".into(),
        set_upstream: true,
    };
    let Ok(Planned::Ready(push)) = plan(&runner, &got, want, PushSwitches::default()) else {
        panic!("no plan");
    };
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Push {
        push,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert!(
        matches!(&end, RunEnd::Failed { said, .. } if said.contains("credential helper")),
        "{end:?} {output}"
    );
    // No terminal: git leads a session of its own, not only a group.
    let leader = std::fs::read_to_string(&session).unwrap();
    assert!(leader.trim().contains('s'), "{leader:?}");
}

#[test]
fn a_remote_named_like_an_option_is_still_the_remote() {
    let (repo, bare) = with_remote("run-push-dash");
    repo.git(&["config", "remote.-x.url", &bare]);
    let (runner, got) = runner(&repo);
    let want = Want::Remote {
        remote: "-x".into(),
        set_upstream: false,
    };
    let Ok(Planned::Ready(push)) = plan(&runner, &got, want, PushSwitches::default()) else {
        panic!("no plan");
    };
    assert!(runner.ask(RunRequest::Remote(RemoteRun::Push {
        push,
        pass: Vec::new(),
    })));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(
        output.contains("-- -x refs/heads/main:refs/heads/main"),
        "{output}"
    );
    let pushed = String::from_utf8(repo.git(&["--git-dir", &bare, "log", "-1", "--format=%s"]));
    assert_eq!(pushed.unwrap().trim(), "first");
}
