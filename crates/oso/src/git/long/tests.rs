//! Long runs of git over a temporary repository: output from both streams in the order git
//! wrote it, the exit status, a stop that ends what git started too, and the end from
//! elsewhere. git runs shell aliases here, as a hook would run a script.
//!
//! Must not depend on: `app`, `dispatch` or winit.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
use super::GitError;
use super::Next;
use crate::git::Git;

/// A scratch directory with an empty repository, `repo`, made by the runner itself.
struct Repo {
    dir: PathBuf,
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Repo {
        let name = format!("oso-git-long-{}-{name}", std::process::id());
        let dir = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let repo = Repo { dir, root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    fn git(&self, words: &[&str]) {
        let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
        git(self).run(&words, &[], 4096).unwrap();
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn git(repo: &Repo) -> Git {
    let hooks = repo.dir.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    Git::new(&repo.root, &hooks).unwrap()
}

/// Everything the run wrote, and how it ended.
fn all_of(long: &mut super::Long) -> (String, std::process::ExitStatus) {
    let mut out = Vec::new();
    loop {
        match long.next(Duration::from_millis(50)).unwrap() {
            Next::Out(piece) => out.extend_from_slice(&piece),
            Next::Quiet => {}
            Next::Ended(status) => return (String::from_utf8_lossy(&out).into_owned(), status),
        }
    }
}

fn alias(name: &str, script: &str) -> Vec<String> {
    vec!["-c".into(), format!("alias.{name}=!{script}"), name.into()]
}

fn os(words: &[String]) -> Vec<&OsStr> {
    words.iter().map(OsStr::new).collect()
}

#[test]
fn a_long_run_gives_both_streams_and_its_status() {
    let repo = Repo::new("long-out");
    let words = alias("say", "echo out; echo err >&2; exit 3");
    let mut long = git(&repo).start_long(&os(&words), &[], None).unwrap();
    let (out, status) = all_of(&mut long);
    assert!(out.contains("out\n") && out.contains("err\n"), "{out:?}");
    assert_eq!(status.code(), Some(3));
}

#[test]
fn a_long_run_reads_what_it_is_given() {
    let repo = Repo::new("long-in");
    let words = alias("cat", "cat");
    let input = Some(b"from oso\n".to_vec());
    let mut long = git(&repo).start_long(&os(&words), &[], input).unwrap();
    let (out, status) = all_of(&mut long);
    assert_eq!(out, "from oso\n");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn a_stop_ends_git_and_what_it_started() {
    let repo = Repo::new("long-stop");
    let marker = repo.dir.join("still-here");
    let script = format!("sleep 30; touch '{}'", marker.display());
    let words = alias("slow", &script);
    let mut long = git(&repo).start_long(&os(&words), &[], None).unwrap();
    assert!(matches!(
        long.next(Duration::from_millis(100)).unwrap(),
        Next::Quiet
    ));
    let asked = Instant::now();
    long.stop();
    let (_, status) = all_of(&mut long);
    assert!(!status.success());
    assert!(asked.elapsed() < super::GRACE, "SIGTERM was enough");
    assert!(long.stopped());
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists());
}

#[cfg(unix)]
#[test]
fn a_stop_flag_ends_a_write_and_says_so() {
    let repo = Repo::new("long-flag");
    let words = alias("slow", "sleep 30");
    let stop = AtomicBool::new(true);
    let started = Instant::now();
    let ran = git(&repo).run_long(&os(&words), &[], None, &stop);
    assert!(matches!(ran, Err(GitError::Stopped)), "{:?}", ran.err());
    assert!(started.elapsed() < super::GRACE);
}

#[test]
fn a_write_keeps_the_start_of_what_git_said() {
    let repo = Repo::new("long-said");
    let words = alias("fail", "echo no such thing >&2; exit 1");
    let stop = AtomicBool::new(false);
    let out = git(&repo).run_long(&os(&words), &[], None, &stop).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(out.stderr, "no such thing\n");
}

#[cfg(unix)]
#[test]
fn the_ender_ends_the_group_from_elsewhere() {
    let repo = Repo::new("long-end");
    let words = alias("slow", "sleep 30");
    let mut long = git(&repo).start_long(&os(&words), &[], None).unwrap();
    long.ender().end();
    let started = Instant::now();
    let (_, status) = all_of(&mut long);
    assert!(!status.success());
    assert!(started.elapsed() < super::GRACE);
}

#[test]
fn hooks_run_only_in_a_run_that_lets_them() {
    let repo = Repo::new("long-hooks");
    repo.git(&["config", "user.name", "t"]);
    repo.git(&["config", "user.email", "t@t"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    let hook = repo.root.join(".git/hooks/pre-commit");
    std::fs::write(&hook, "#!/bin/sh\necho the hook ran\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let words = ["commit", "--allow-empty", "-m", "x"].map(String::from);
    let off = git(&repo);
    let mut long = off.start_long(&os(&words), &[], None).unwrap();
    let (out, status) = all_of(&mut long);
    assert!(status.success(), "{out}");
    assert!(!out.contains("the hook ran"), "{out}");
    let mut long = off
        .with_own_hooks(&[])
        .start_long(&os(&words), &[], None)
        .unwrap();
    let (out, status) = all_of(&mut long);
    assert!(!status.success());
    assert!(out.contains("the hook ran"), "{out}");
}

#[cfg(unix)]
#[test]
fn a_process_left_holding_the_pipes_holds_up_the_end_only_a_moment() {
    let repo = Repo::new("long-left");
    // It writes on after git has gone, every 10 ms, for 30 s.
    let words = alias(
        "leave",
        "(i=0; while [ $i -lt 3000 ]; do echo x; i=$((i+1)); sleep 0.01; done) & echo gone",
    );
    let mut long = git(&repo).start_long(&os(&words), &[], None).unwrap();
    let started = Instant::now();
    let (out, status) = all_of(&mut long);
    assert!(status.success());
    assert!(out.starts_with("gone\n"), "{out}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    long.stop();
}

/// A run of `commit` in `repo` whose pre-commit hook ignores `SIGTERM`, as a hook may, and
/// marks `marker` after a while; git itself dies of `SIGTERM` and leaves the hook behind.
#[cfg(unix)]
fn deaf_commit(repo: &Repo, marker: &std::path::Path) -> super::Long {
    use std::os::unix::fs::PermissionsExt;
    repo.git(&["config", "user.name", "t"]);
    repo.git(&["config", "user.email", "t@t"]);
    repo.git(&["config", "commit.gpgsign", "false"]);
    let hook = repo.root.join(".git/hooks/pre-commit");
    let script = format!(
        "#!/bin/sh\ntrap '' TERM\nsleep 2\ntouch '{}'\n",
        marker.display()
    );
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let words = ["commit", "--allow-empty", "-m", "x"].map(String::from);
    let long = git(repo)
        .with_own_hooks(&[])
        .start_long(&os(&words), &[], None)
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    long
}

#[cfg(unix)]
#[test]
fn a_stop_ends_a_hook_that_ignores_sigterm_once_git_has_gone() {
    let repo = Repo::new("long-deaf");
    let marker = repo.dir.join("went-on");
    let mut long = deaf_commit(&repo, &marker);
    let asked = Instant::now();
    long.stop();
    let (_, status) = all_of(&mut long);
    assert!(!status.success());
    // git died of the SIGTERM; its group got SIGKILL before it was reaped, not after GRACE.
    assert!(asked.elapsed() < super::GRACE, "{:?}", asked.elapsed());
    std::thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "the hook went on");
}

#[cfg(unix)]
#[test]
fn quitting_ends_a_hook_that_ignores_sigterm_at_once() {
    let repo = Repo::new("long-deaf-end");
    let marker = repo.dir.join("went-on");
    let mut long = deaf_commit(&repo, &marker);
    long.ender().end();
    let (_, status) = all_of(&mut long);
    assert!(!status.success());
    std::thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "the hook went on");
}

#[cfg(unix)]
#[test]
fn a_dropped_run_ends_a_hook_that_ignores_sigterm() {
    let repo = Repo::new("long-deaf-drop");
    let marker = repo.dir.join("went-on");
    let long = deaf_commit(&repo, &marker);
    drop(long);
    std::thread::sleep(Duration::from_secs(3));
    assert!(!marker.exists(), "the hook went on");
}
