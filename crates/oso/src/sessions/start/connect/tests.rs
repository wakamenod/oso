//! Tests of a start that Remote Control may connect, over temporary repositories: which remotes
//! keep it blocked, and where it must run to come back.
//!
//! Must not depend on: `app`, the network or the person's own configuration but as git reads it;
//! git runs only through the process module.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
use serde_json::json;

use super::{
    Back, CHANGED, Connect, Leave, MOVED, NO_ANSWER, NO_STATE, NO_TRUST, Place, Sent, Settled,
    TOO_MANY, address_of, judge, look, settle,
};
use crate::git::TrustedRoot;
use crate::sessions::native::wire::remote::RemoteId;
use crate::sessions::start::line_tests::native_with;
use crate::sessions::start::{Kind, Order, Runs};

struct Scratch {
    dir: PathBuf,
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Scratch {
    /// A directory of its own, `repo` in it.
    fn bare(name: &str) -> Scratch {
        let dir = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("oso-connect-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let root = dir.join("repo");
        fs::create_dir(&root).unwrap();
        fs::create_dir(dir.join("state")).unwrap();
        Scratch { dir, root }
    }

    /// One with a repository with a commit on `main`.
    fn new(name: &str) -> Scratch {
        let scratch = Self::bare(name);
        scratch.git(&["init", "-q", "-b", "main"]);
        scratch.git(&["commit", "-q", "--allow-empty", "-m", "first"]);
        scratch
    }

    fn git(&self, args: &[&str]) {
        let git = Executable::from_path("git", None).expect("git is on PATH");
        let mut all: Vec<OsString> = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ]
        .map(OsString::from)
        .to_vec();
        all.extend(args.iter().map(OsString::from));
        let mut child = spawn(&Spawn {
            executable: &git,
            args: &all,
            cwd: Some(&self.root),
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            own_session: false,
            set: &[
                ("HOME", self.dir.as_os_str()),
                ("GIT_CONFIG_NOSYSTEM", "1".as_ref()),
            ],
            terminal: None,
            pass_fd: None,
            args_logged: true,
        })
        .unwrap();
        assert!(child.wait().unwrap().success(), "git {args:?}");
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn look_at(&self, root: &Path) -> Result<(Sent, Option<Place>), String> {
        let trusted = TrustedRoot::for_tests(root.to_owned());
        look(&trusted, root, Some(&self.state()))
    }

    fn look(&self) -> Result<(Sent, Option<Place>), String> {
        self.look_at(&self.root)
    }

    fn refusal(&self) -> String {
        self.look().unwrap_err()
    }
}

const USER: &str = "a git remote's address holds a user name";
const PASSWORD: &str = "a git remote's address holds a password";
const UNREAD: &str = "a git remote's address cannot be read for a user name or a password";

#[test]
fn what_is_sent_names_each_address_and_the_branch() {
    let scratch = Scratch::new("sent");
    assert!(scratch.look().unwrap().0.addresses.is_empty());
    scratch.git(&["remote", "add", "origin", "https://example.invalid/o/r.git"]);
    scratch.git(&["remote", "add", "fork", "git@example.invalid:f/r.git"]);
    let (sent, place) = scratch.look().unwrap();
    assert_eq!(
        sent.addresses,
        [
            "git@example.invalid:f/r.git",
            "https://example.invalid/o/r.git"
        ]
    );
    assert_eq!(sent.branch.as_deref(), Some("main"));
    assert!(place.unwrap().repository.is_some());
    // Detached, no branch is shown.
    scratch.git(&["checkout", "-q", "--detach"]);
    assert_eq!(scratch.look().unwrap().0.branch, None);
    for n in 0..10 {
        let name = format!("r{n}");
        scratch.git(&["remote", "add", &name, &format!("https://h/{n}")]);
    }
    assert_eq!(scratch.look().unwrap().0.addresses.len(), 12);
    scratch.git(&["remote", "add", "more", "https://h/more"]);
    assert_eq!(scratch.refusal(), TOO_MANY);
    scratch.git(&["remote", "remove", "more"]);
    let long = format!("https://h/{}", "r".repeat(400));
    scratch.git(&["remote", "set-url", "r0", &long]);
    assert_eq!(scratch.refusal(), TOO_MANY);
}

#[test]
fn any_remotes_user_name_or_password_refuses() {
    let key = |key: &str, value: &str, why: &str| {
        let scratch = Scratch::new("remotes");
        scratch.git(&["remote", "add", "origin", "https://example.invalid/o/r.git"]);
        assert!(scratch.look().is_ok());
        scratch.git(&["config", "--add", key, value]);
        let said = scratch.refusal();
        assert_eq!(said, why, "{key}={value}");
        assert!(!said.contains(value));
    };
    key("remote.fork.url", "https://me:secret@h/r", PASSWORD);
    key("remote.fork.url", "https://token@h/r", USER);
    key("remote.origin.url", "https://token@h/r", USER);
    key("remote.origin.pushurl", "ssh://me@h/r", USER);
    key("remote.fork.url", "ext::sh -c x", UNREAD);
    key("remote.fork.url", "https://me:pa/ss@h/r", UNREAD);
    key("branch.main.remote", "https://me:secret@h/r", PASSWORD);
    key("branch.main.pushRemote", "https://me@h/r", USER);
    key("remote.pushDefault", "https://me@h/r", USER);
    // What a rewrite makes of an address.
    let rewrite = "url.https://me:secret@example.invalid/";
    let from = "https://example.invalid/";
    key(&format!("{rewrite}.insteadOf"), from, PASSWORD);
    key(&format!("{rewrite}.pushInsteadOf"), from, PASSWORD);
}

#[test]
fn an_included_configuration_is_read() {
    let scratch = Scratch::new("include");
    let more = scratch.dir.join("more");
    fs::write(&more, "[remote \"x\"]\n\turl = https://me:secret@h/r\n").unwrap();
    scratch.git(&["config", "include.path", more.to_str().unwrap()]);
    assert_eq!(scratch.refusal(), PASSWORD);
}

#[test]
fn a_git_that_does_not_answer_refuses() {
    let scratch = Scratch::new("broken");
    fs::write(scratch.root.join(".git/config"), "[remote\n").unwrap();
    assert_eq!(scratch.refusal(), NO_ANSWER);
    let scratch = Scratch::bare("no-repository");
    fs::write(scratch.root.join(".git"), "gitdir: /nowhere/at/all\n").unwrap();
    assert_eq!(scratch.refusal(), NO_ANSWER);
    // Remotes as git kept them before its configuration.
    let scratch = Scratch::new("old");
    let old = scratch.root.join(".git/remotes");
    fs::create_dir_all(&old).unwrap();
    assert!(scratch.look().is_ok());
    fs::write(old.join("x"), "URL: https://me:s@h/r\n").unwrap();
    assert_eq!(scratch.refusal(), NO_ANSWER);
}

#[test]
fn below_another_repository_it_refuses_and_with_none_nothing_is_sent() {
    let scratch = Scratch::bare("below");
    let (sent, place) = scratch.look().unwrap();
    assert_eq!(sent, Sent::default());
    assert_eq!(place.unwrap().repository, None);
    fs::create_dir(scratch.dir.join(".git")).unwrap();
    let why = "its directory is in another repository";
    assert_eq!(scratch.refusal(), why);
    // Or one with no `.git` of its own, above it or itself.
    for place in ["", "repo"] {
        let scratch = Scratch::bare("bare");
        for name in ["HEAD", "objects", "refs"] {
            fs::create_dir(scratch.dir.join(place).join(name)).unwrap();
        }
        assert_eq!(scratch.refusal(), why);
    }
}

#[test]
fn a_linked_worktree_reads_its_repositorys_remotes() {
    let scratch = Scratch::new("linked");
    let linked = scratch.dir.join("linked");
    let path = linked.to_str().unwrap();
    scratch.git(&["worktree", "add", "-q", "-b", "b", path]);
    scratch.git(&["remote", "add", "origin", "https://example.invalid/o/r.git"]);
    let (sent, place) = scratch.look_at(&linked).unwrap();
    assert_eq!(sent.addresses, ["https://example.invalid/o/r.git"]);
    assert_eq!(sent.branch.as_deref(), Some("b"));
    let main = scratch.look().unwrap().1.unwrap();
    let place = place.unwrap();
    assert_eq!(place.repository, main.repository);
    assert_ne!(place.directory, main.directory);
    scratch.git(&["remote", "set-url", "origin", "https://t@github.com/o/r"]);
    assert_eq!(scratch.look_at(&linked).unwrap_err(), USER);
}

#[test]
fn only_under_the_trust_of_the_directory_itself() {
    let scratch = Scratch::new("trust");
    let state = scratch.state();
    let root = TrustedRoot::for_tests(scratch.root.clone());
    let below = scratch.root.join("below");
    fs::create_dir(&below).unwrap();
    assert_eq!(look(&root, &below, Some(&state)).unwrap_err(), NO_TRUST);
    assert_eq!(look(&root, &scratch.root, None).unwrap_err(), NO_STATE);
    root.revoke();
    let said = look(&root, &scratch.root, Some(&state)).unwrap_err();
    assert_eq!(said, NO_TRUST);
}

fn remote_id() -> RemoteId {
    use crate::sessions::native::wire::{Event, parse};
    let line = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": "oso-1",
            "response": {"bridge_session_id": "cse_abc"},
        },
    });
    let Event::Answer(answer) = parse(line.to_string().as_bytes()) else {
        panic!("no answer");
    };
    answer.remote.unwrap().id.unwrap()
}

#[test]
fn a_session_comes_back_only_where_it_was() {
    let scratch = Scratch::new("back");
    let state = scratch.state();
    let at = scratch.look().unwrap().1.unwrap();
    let root = TrustedRoot::for_tests(scratch.root.clone());
    let again = |place: Place| {
        let connect = Connect::again(Back {
            id: remote_id(),
            place,
        });
        judge(&root, &scratch.root, Some(&state), &connect)
    };
    assert_eq!(again(at), Ok(Some(at)));
    let elsewhere = (at.directory.0, at.directory.1 + 1);
    for place in [
        Place {
            directory: elsewhere,
            ..at
        },
        Place {
            repository: None,
            ..at
        },
        Place {
            repository: Some(elsewhere),
            ..at
        },
    ] {
        assert_eq!(again(place), Err(MOVED.to_owned()));
    }
    // The person's own needs no place it was in, and what it was said to send.
    let sent = scratch.look().unwrap().0;
    let asked = Connect::asked(sent);
    let judged = || judge(&root, &scratch.root, Some(&state), &asked);
    assert_eq!(judged(), Ok(Some(at)));
    scratch.git(&["remote", "add", "origin", "https://example.invalid/other"]);
    assert_eq!(judged(), Err(CHANGED.to_owned()));
    scratch.git(&["remote", "remove", "origin"]);
    assert_eq!(judged(), Ok(Some(at)));
    scratch.git(&["checkout", "-q", "-b", "other"]);
    assert_eq!(judged(), Err(CHANGED.to_owned()));
}

fn order(scratch: &Scratch, remote: Leave, trusted: Option<TrustedRoot>) -> Order {
    Order {
        token: 1,
        kind: Kind::New,
        name: "s".to_owned(),
        directory: scratch.root.clone(),
        project: Some(scratch.root.clone()),
        trusted,
        baseline: false,
        state: Some(scratch.state()),
        claude: None,
        runs: native_with(remote),
        brief: None,
        steer_worktrees: false,
        check_shared: false,
        space: None,
    }
}

#[test]
fn a_start_is_blocked_unless_every_check_passes() {
    let scratch = Scratch::new("settle");
    let trusted = || Some(TrustedRoot::for_tests(scratch.root.clone()));
    let connect = || {
        let sent = Sent {
            branch: Some("main".into()),
            ..Sent::default()
        };
        Leave::Connect(Connect::asked(sent))
    };
    let blocked = |order: &Order| match &order.runs {
        Runs::Native(native) => native.remote.blocked(),
        Runs::Terminal(_) => panic!("not native"),
    };
    // With no leave.
    let mut plain = order(&scratch, Leave::Blocked, trusted());
    assert_eq!(settle(&mut plain), Settled::Blocked);
    assert!(blocked(&plain));
    // With it.
    let mut leave = order(&scratch, connect(), trusted());
    assert!(matches!(settle(&mut leave), Settled::Connects(Some(_))));
    assert!(!blocked(&leave));
    // Under a space's gate there is no trust.
    let mut gated = order(&scratch, connect(), None);
    assert_eq!(settle(&mut gated), Settled::Refused(NO_TRUST.to_owned()));
    assert!(blocked(&gated));
    // A remote's password.
    scratch.git(&["remote", "add", "origin", "https://a:b@h/r"]);
    let mut remote = order(&scratch, connect(), trusted());
    assert_eq!(settle(&mut remote), Settled::Refused(PASSWORD.to_owned()));
    assert!(blocked(&remote));
}

#[test]
fn a_line_of_gits_list_gives_its_address_alone() {
    let of = |line: &'static str| address_of(line.as_bytes());
    assert_eq!(of("o\thttps://h/r (fetch)"), Some(&b"https://h/r"[..]));
    let partial = of("o\thttps://h/r (push) [blob:none]");
    assert_eq!(partial, Some(&b"https://h/r"[..]));
    assert_eq!(of("o\t/a b (fetch)"), None);
    assert_eq!(of("o https://h/r (fetch)"), None);
    assert_eq!(of("o\thttps://h/r"), None);
}
