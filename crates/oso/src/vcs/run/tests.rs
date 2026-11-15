//! A repository's run thread over temporary repositories: a commit with its message and its
//! output, a hook that fails and the name the failure gives, `--no-verify`, the message an
//! amend starts from, a stop that ends a hook and leaves no lock, and a root it refuses.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::sync::mpsc;
use std::time::Duration;

use oso_core::editor::{CommitKind, CommitSwitches};

use super::{RunEnd, RunReport, RunRequest, Runner};
use crate::git::TrustedRoot;
use crate::review::tests::Repo;

/// A repository with a first commit, its own identity and no signing.
fn repo(name: &str) -> Repo {
    let repo = Repo::new(name);
    repo.git(&["config", "user.name", "t"]);
    repo.git(&["config", "user.email", "t@t"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.root.join("a.txt"), "one\n").unwrap();
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-q", "-m", "first"]);
    repo
}

fn runner(repo: &Repo) -> (Runner, mpsc::Receiver<RunReport>) {
    let (sent, got) = mpsc::channel();
    let runner = Runner::start(
        TrustedRoot::for_tests(repo.root.clone()),
        Some(repo.dir.join("state")),
        move |report| sent.send(report).is_ok(),
    )
    .unwrap();
    (runner, got)
}

/// The reports until a run ends, the end, and the output it wrote.
fn until_end(got: &mpsc::Receiver<RunReport>) -> (RunEnd, String, bool) {
    let mut output = String::new();
    let mut began = false;
    loop {
        match got.recv_timeout(Duration::from_secs(30)).expect("a report") {
            RunReport::Began { line } => {
                began = true;
                output.push_str(&line);
            }
            RunReport::Output(text) => output.push_str(&text),
            RunReport::Elapsed(_) | RunReport::Message(_) => {}
            RunReport::Ended(end) => return (end, output, began),
        }
    }
}

fn commit(message: &str, switches: CommitSwitches) -> RunRequest {
    RunRequest::Commit {
        kind: CommitKind::New,
        switches,
        message: Some(message.as_bytes().to_vec()),
        pass: Vec::new(),
    }
}

fn subject(repo: &Repo) -> String {
    String::from_utf8(repo.git(&["log", "-1", "--format=%s"]))
        .unwrap()
        .trim()
        .to_owned()
}

#[cfg(unix)]
fn hook(repo: &Repo, name: &str, script: &str) {
    let path = repo.root.join(".git/hooks").join(name);
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_commit_takes_its_message_and_leaves_out_comment_lines() {
    let repo = repo("run-commit");
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let (runner, got) = runner(&repo);
    assert!(runner.ask(commit(
        "Change a\n\n# not this\nBody\n",
        CommitSwitches::default()
    )));
    let (end, output, began) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(began);
    assert!(output.starts_with("$ git -c core.commentChar=# commit --cleanup=strip --file=-\n"));
    assert_eq!(subject(&repo), "Change a");
    let body = String::from_utf8(repo.git(&["log", "-1", "--format=%b"])).unwrap();
    assert_eq!(body.trim(), "Body");
}

#[cfg(unix)]
#[test]
fn a_failing_hook_is_named_and_no_verify_leaves_it_out() {
    let repo = repo("run-hook");
    hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho checks failed >&2\nexit 1\n",
    );
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let (runner, got) = runner(&repo);
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let (end, output, _) = until_end(&got);
    assert_eq!(
        end,
        RunEnd::Failed("commit failed (hook pre-commit): $ shows the output".into())
    );
    assert!(output.contains("checks failed\n"), "{output}");
    assert_eq!(subject(&repo), "first");
    let switches = CommitSwitches {
        no_verify: true,
        ..CommitSwitches::default()
    };
    runner.ask(commit("Change a\n", switches));
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert!(output.contains("--no-verify"));
    assert_eq!(subject(&repo), "Change a");
}

#[test]
fn an_amend_starts_from_the_message_of_head() {
    let repo = repo("run-message");
    let (runner, got) = runner(&repo);
    runner.ask(RunRequest::Message);
    let Ok(RunReport::Message(message)) = got.recv_timeout(Duration::from_secs(30)) else {
        panic!("a message");
    };
    assert_eq!(message.unwrap().trim(), "first");
    runner.ask(RunRequest::Commit {
        kind: CommitKind::Reword,
        switches: CommitSwitches::default(),
        message: Some(b"First, reworded\n".to_vec()),
        pass: Vec::new(),
    });
    let (end, output, _) = until_end(&got);
    assert_eq!(end, RunEnd::Done, "{output}");
    assert_eq!(subject(&repo), "First, reworded");
    let count = String::from_utf8(repo.git(&["rev-list", "--count", "HEAD"])).unwrap();
    assert_eq!(count.trim(), "1");
}

#[cfg(unix)]
#[test]
fn a_stop_ends_the_hook_and_leaves_no_lock() {
    let repo = repo("run-stop");
    let marker = repo.dir.join("hook-went-on");
    let script = format!("#!/bin/sh\nsleep 30\ntouch '{}'\n", marker.display());
    hook(&repo, "pre-commit", &script);
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let (runner, got) = runner(&repo);
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let Ok(RunReport::Began { .. }) = got.recv_timeout(Duration::from_secs(30)) else {
        panic!("the run begins");
    };
    std::thread::sleep(Duration::from_millis(300));
    runner.stop();
    let (end, _, _) = until_end(&got);
    assert_eq!(end, RunEnd::Stopped);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists());
    assert!(!repo.root.join(".git/index.lock").exists());
    assert_eq!(subject(&repo), "first");
}

#[test]
fn a_root_no_longer_trusted_runs_nothing() {
    let repo = repo("run-revoked");
    let root = TrustedRoot::for_tests(repo.root.clone());
    let (sent, got) = mpsc::channel();
    let runner = Runner::start(root.clone(), Some(repo.dir.join("state")), move |report| {
        sent.send(report).is_ok()
    })
    .unwrap();
    root.revoke();
    runner.ask(commit("Nothing\n", CommitSwitches::default()));
    let (end, output, began) = until_end(&got);
    assert!(matches!(end, RunEnd::Refused(_)), "{end:?}");
    assert!(!began && output.is_empty());
    assert_eq!(subject(&repo), "first");
}

#[test]
fn a_flood_is_sent_as_its_end_with_what_was_left_out() {
    let mut waiting = super::Waiting::default();
    assert_eq!(waiting.take(), None);
    let line = format!("{}\n", "y".repeat(99));
    for _ in 0..(3 * super::MAX_FLUSH / line.len()) {
        waiting.push(&line);
    }
    let sent = waiting.take().unwrap();
    let (said, rest) = sent.split_once('\n').unwrap();
    assert!(
        said.starts_with("[… ") && said.ends_with(" bytes left out]"),
        "{said}"
    );
    assert!(rest.len() <= super::MAX_FLUSH);
    assert!(rest.ends_with(&line) && rest.starts_with('y'));
    waiting.push("last\n");
    assert_eq!(waiting.take().as_deref(), Some("last\n"));
}

#[cfg(unix)]
#[test]
fn a_root_revoked_while_a_hook_runs_stops_it() {
    let repo = repo("run-revoke-mid");
    let marker = repo.dir.join("hook-went-on");
    let script = format!("#!/bin/sh\nsleep 30\ntouch '{}'\n", marker.display());
    hook(&repo, "pre-commit", &script);
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let root = TrustedRoot::for_tests(repo.root.clone());
    let (sent, got) = mpsc::channel();
    let runner = Runner::start(root.clone(), Some(repo.dir.join("state")), move |report| {
        sent.send(report).is_ok()
    })
    .unwrap();
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let Ok(RunReport::Began { .. }) = got.recv_timeout(Duration::from_secs(30)) else {
        panic!("the run begins");
    };
    std::thread::sleep(Duration::from_millis(300));
    let revoked = std::time::Instant::now();
    root.revoke();
    let (end, _, _) = until_end(&got);
    assert_eq!(end, RunEnd::Stopped);
    assert!(revoked.elapsed() < crate::git::GRACE);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists());
    assert_eq!(subject(&repo), "first");
}

#[cfg(unix)]
#[test]
fn the_runner_ends_its_run_as_oso_ends() {
    let repo = repo("run-end");
    let marker = repo.dir.join("hook-went-on");
    let script = format!("#!/bin/sh\nsleep 30\ntouch '{}'\n", marker.display());
    hook(&repo, "pre-commit", &script);
    std::fs::write(repo.root.join("a.txt"), "two\n").unwrap();
    repo.git(&["add", "a.txt"]);
    let (runner, got) = runner(&repo);
    runner.ask(commit("Change a\n", CommitSwitches::default()));
    let Ok(RunReport::Began { .. }) = got.recv_timeout(Duration::from_secs(30)) else {
        panic!("the run begins");
    };
    runner.end();
    let (end, _, _) = until_end(&got);
    assert!(matches!(end, RunEnd::Failed(_)), "{end:?}");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists());
}
