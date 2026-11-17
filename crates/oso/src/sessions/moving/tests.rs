//! Tests of moving a session started outside oso (ADR 0151): which registry entry is the
//! asker's, the repository it is placed by, the wait's steps, and the words.
//!
//! Must not depend on: `app`, or the system's own registry; these use a scratch directory of
//! their own.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{
    Asker, GIVE_UP, Held, LATE, Look, NOT_FOUND, POLL, Recorded, Step, called, details, gave_up,
    judge, look, question, running, spawn_wait, step,
};
use crate::sessions::identity::SessionId;
use crate::sessions::spaces::Member;

const ID: &str = "1234abcd-0000-4000-8000-00000000aaaa";
const PID: u32 = 4242;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-moving-{name}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn dir(&self, name: &str) -> PathBuf {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A main checkout at `name`: a directory with a `.git` directory.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.dir(name);
        fs::create_dir_all(repo.join(".git")).unwrap();
        repo
    }

    /// The registry, holding an entry of [`PID`] that ran in `cwd` from `started`.
    fn registry(&self, cwd: &str, started: SystemTime) -> PathBuf {
        self.registry_of(cwd, started, "interactive")
    }

    /// The registry, holding an entry of [`PID`] of `kind` that ran in `cwd` from `started`.
    fn registry_of(&self, cwd: &str, started: SystemTime, kind: &str) -> PathBuf {
        let dir = self.dir("registry");
        let ms = started.duration_since(UNIX_EPOCH).unwrap().as_millis();
        let entry = format!(
            r#"{{"pid":{PID},"sessionId":"{ID}","cwd":"{cwd}","startedAt":{ms},"name":"api-fix","kind":"{kind}","status":"busy"}}"#
        );
        fs::write(dir.join(format!("{PID}.json")), entry).unwrap();
        dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn started() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_794_512_815)
}

/// The look of [`PID`] started at [`started`], the home directory `home`, `trusted` trusted.
fn asked(home: &Path, trusted: &[PathBuf]) -> Look {
    Look {
        pid: PID,
        started: started(),
        program: Some("claude".into()),
        trusted: Arc::from(trusted),
        home: Some(home.to_path_buf()),
    }
}

/// The process table as it is when [`PID`] still runs from [`started`].
fn still(pid: u32) -> io::Result<Option<SystemTime>> {
    Ok((pid == PID).then(started))
}

#[test]
fn the_asker_is_the_bridges_parent_by_its_registry_entry() {
    let scratch = Scratch::new("found");
    let repo = scratch.repo("api");
    let cwd = scratch.dir("api/src");
    let registry = scratch.registry(&cwd.display().to_string(), started());
    let asker = look(Some(&registry), &asked(&scratch.0, &[]), still).unwrap();
    assert_eq!(asker.id.as_str(), ID);
    assert_eq!(asker.pid, PID);
    assert_eq!(asker.name.as_deref(), Some("api-fix"));
    assert_eq!(asker.directory, cwd);
    assert_eq!(asker.program.as_deref(), Some("claude"));
    assert_eq!(asker.main.root, repo);
    assert!(asker.main.identity.is_some());
    assert_eq!(asker.worktree, None);
}

#[test]
fn an_entry_that_is_not_the_parents_as_it_connected_is_refused() {
    let scratch = Scratch::new("other");
    scratch.repo("api");
    let cwd = scratch.0.join("api").display().to_string();
    let home = scratch.0.clone();
    // No registry, or none with the parent's entry.
    assert_eq!(
        look(None, &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    let empty = scratch.dir("empty");
    assert_eq!(
        look(Some(&empty), &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    // An entry from another process that had the id before.
    let registry = scratch.registry(&cwd, started() - Duration::from_secs(3600));
    assert_eq!(
        look(Some(&registry), &asked(&home, &[]), still).unwrap_err(),
        NOT_FOUND
    );
    // A run with `-p` does not move.
    let printing = scratch.registry_of(&cwd, started(), "print");
    let why = look(Some(&printing), &asked(&home, &[]), still).unwrap_err();
    assert!(why.contains("interactive"), "{why}");
    // The parent ended, and another process has its id now.
    let registry = scratch.registry(&cwd, started());
    let another = |_| Ok(Some(started() + Duration::from_secs(60)));
    let why = look(Some(&registry), &asked(&home, &[]), another).unwrap_err();
    assert!(why.contains("not the one that connected"), "{why}");
    let gone = |_| Ok(None);
    assert!(look(Some(&registry), &asked(&home, &[]), gone).is_err());
}

#[test]
fn a_directory_oso_cannot_place_is_refused() {
    let scratch = Scratch::new("places");
    let repo = scratch.repo("api");
    let home = scratch.0.clone();
    let refused = |cwd: &str| {
        let registry = scratch.registry(cwd, started());
        look(Some(&registry), &asked(&home, &[]), still).unwrap_err()
    };
    // The registry's reader keeps no directory it could not place a session by.
    assert_eq!(refused("api"), NOT_FOUND);
    let dotted = format!("{}/../api", repo.display());
    assert_eq!(refused(&dotted), NOT_FOUND);
    assert!(refused(&format!("{}/gone", repo.display())).contains("gone"));
    let plain = scratch.dir("notes");
    assert!(refused(&plain.display().to_string()).contains("no repository"));
    #[cfg(unix)]
    {
        let link = scratch.0.join("link");
        std::os::unix::fs::symlink(&repo, &link).unwrap();
        assert!(refused(&link.display().to_string()).contains("link"));
    }
}

/// A worktree at `worktree` of the main repository at `main`, as `git worktree add` leaves
/// them.
fn worktree_pair(scratch: &Scratch) -> (PathBuf, PathBuf) {
    let main = scratch.repo("main");
    let worktree = scratch.dir("wt");
    let entry = main.join(".git/worktrees/wt");
    fs::create_dir_all(&entry).unwrap();
    fs::write(
        entry.join("gitdir"),
        format!("{}\n", worktree.join(".git").display()),
    )
    .unwrap();
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", entry.display()),
    )
    .unwrap();
    (main, worktree)
}

#[test]
fn a_worktree_counts_only_with_its_main_repository_trusted() {
    let scratch = Scratch::new("worktree");
    let (main, worktree) = worktree_pair(&scratch);
    let registry = scratch.registry(&worktree.display().to_string(), started());
    let home = scratch.0.clone();
    let why = look(Some(&registry), &asked(&home, &[]), still).unwrap_err();
    assert!(why.contains("does not trust"), "{why}");
    let trusted = std::slice::from_ref(&main);
    let asker = look(Some(&registry), &asked(&home, trusted), still).unwrap();
    assert_eq!(asker.main.root, main);
    assert_eq!(asker.worktree, Some(worktree));
}

#[test]
fn the_wait_stops_once_the_process_ended_and_gives_up_after_five_minutes() {
    let soon = Duration::from_secs(1);
    assert_eq!(step(soon, true, false), Step::Wait);
    assert_eq!(step(soon, false, false), Step::Stopped);
    assert_eq!(step(LATE, true, false), Step::Late);
    assert_eq!(step(LATE, true, true), Step::Wait);
    assert_eq!(step(LATE, false, true), Step::Stopped);
    assert_eq!(step(GIVE_UP, true, true), Step::GiveUp);
    assert_eq!(step(GIVE_UP, false, true), Step::Stopped);
    // The process with the id must be the one that started then.
    assert!(running(PID, started(), still));
    assert!(!running(PID, started(), |_| Ok(None)));
    let other = |_| Ok(Some(started() + Duration::from_secs(30)));
    assert!(!running(PID, started(), other));
    // Linux counts a start from boot, which moves when the clock is set: within the
    // registry's slack it is the same process, or a resume could fork the conversation.
    let stepped = |_| Ok(Some(started() + Duration::from_secs(1)));
    assert!(running(PID, started(), stepped));
    let back = |_| Ok(Some(started() - Duration::from_secs(1)));
    assert!(running(PID, started(), back));
    // A system that cannot tell keeps the wait going: a resume could fork the conversation.
    assert!(running(PID, started(), |_| Err(io::Error::other("no"))));
}

fn asker(directory: &str) -> Asker {
    Asker {
        pid: PID,
        started: started(),
        id: SessionId::parse(ID).unwrap(),
        program: Some("claude".into()),
        name: Some("api-fix".into()),
        directory: PathBuf::from(directory),
        main: Member {
            root: PathBuf::from("/w/api"),
            identity: None,
        },
        worktree: None,
    }
}

#[test]
fn the_question_names_the_session_its_directory_and_what_is_added() {
    let asking = asker("/w/api/src");
    let id = SessionId::parse(ID).unwrap();
    assert_eq!(called(Some("api-fix"), &id), "api-fix");
    assert_eq!(called(Some("  "), &id), "1234abcd");
    assert_eq!(called(None, &id), "1234abcd");
    let label = question("api-fix", &asking, "fix-login", None);
    assert_eq!(
        label,
        "move session \"api-fix\" (\"claude\", pid 4242, in \"/w/api/src\", outside oso) into space \
         \"fix-login\"? It stops there first, then resumes here with your settings and may \
         start sessions as yours do."
    );
    let adding = question("api-fix", &asking, "fix-login", Some(Path::new("/w/api")));
    assert!(
        adding.contains("into space \"fix-login\", adding repository \"/w/api\" to it?"),
        "{adding}"
    );
    let shown = details(&label, &asking, "fix-login", None);
    assert!(shown.contains(ID), "{shown}");
    assert!(shown.contains("/exit"), "{shown}");
    // The question names the process as the system does, not as the registry says.
    let mut shell = asker("/w/api/src");
    shell.program = Some("sh".into());
    let label = question("api-fix", &shell, "fix-login", None);
    assert!(label.contains("(\"sh\", pid 4242"), "{label}");
    shell.program = None;
    let label = question("api-fix", &shell, "fix-login", None);
    assert!(label.contains("(a process with no name, pid"), "{label}");
    // `?` shows a long path whole, and says which space a worktree leaves.
    let long = format!("/w/{}/api", "d".repeat(200));
    let mut deep = asker(&long);
    deep.worktree = Some(PathBuf::from(&long));
    let shown = details(&label, &deep, "fix-login", Some("other"));
    assert!(shown.contains(&long), "{shown}");
    assert!(shown.contains("which leaves space \"other\""), "{shown}");
}

#[test]
fn registry_text_is_escaped_in_every_word() {
    // A name and a directory from the registry, which any process of the person's may write.
    let asking = asker("/w/a\u{202e}pi\n");
    let label = question("x\"\u{1b}[2J", &asking, "s", None);
    assert!(!label.contains('\u{202e}') && !label.contains('\n') && !label.contains('\u{1b}'));
    assert!(label.contains("\\u{202e}"), "{label}");
    let said = gave_up("\u{202e}name");
    assert!(!said.contains('\u{202e}'), "{said}");
}

#[test]
fn a_move_is_judged_by_the_record_the_trust_and_the_space() {
    let asking = asker("/w/api/src");
    let held = |record, trusted, member, members| Held {
        record,
        trusted,
        member,
        members,
    };
    // A new repository joins the space; one it has does not.
    assert_eq!(judge(&asking, "s", &held(None, true, false, 1)), Ok(true));
    assert_eq!(judge(&asking, "s", &held(None, true, true, 16)), Ok(false));
    // A full space, an untrusted repository.
    let full = judge(&asking, "s", &held(None, true, false, 16)).unwrap_err();
    assert!(full.contains("16 repositories"), "{full}");
    let untrusted = judge(&asking, "s", &held(None, false, true, 1)).unwrap_err();
    assert!(untrusted.contains("trust-project"), "{untrusted}");
    // oso's record: a running one forks, another directory or an orchestrator does not move,
    // and a stopped one in the same directory does.
    let recorded = |runs, directory: &'static str, orchestrates| Recorded {
        name: "api-fix",
        runs,
        directory: Path::new(directory),
        orchestrates,
    };
    let running = held(Some(recorded(true, "/w/api/src", false)), true, true, 1);
    assert!(
        judge(&asking, "s", &running)
            .unwrap_err()
            .contains("runs this")
    );
    let elsewhere = held(Some(recorded(false, "/w/other", false)), true, true, 1);
    assert!(
        judge(&asking, "s", &elsewhere)
            .unwrap_err()
            .contains("another directory")
    );
    let orchestrator = held(Some(recorded(false, "/w/api/src", true)), true, true, 1);
    assert!(
        judge(&asking, "s", &orchestrator)
            .unwrap_err()
            .contains("orchestrator")
    );
    let stopped = held(Some(recorded(false, "/w/api/src", false)), true, false, 1);
    assert_eq!(judge(&asking, "s", &stopped), Ok(true));
}

#[test]
fn the_wait_ends_when_told_without_a_step() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    // This test's own process runs all along, so only `stop` ends the wait.
    let pid = std::process::id();
    let then = oso_platform::process::start_time(pid).unwrap().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (sent, steps) = mpsc::channel();
    let (ended, done) = mpsc::channel::<()>();
    spawn_wait(pid, then, Arc::clone(&stop), move |step| {
        let _ = &ended;
        sent.send(step).is_ok()
    })
    .unwrap();
    stop.store(true, Ordering::Release);
    // The thread drops `send`, and with it `ended`, within a poll.
    let gone = done.recv_timeout(POLL * 4);
    assert_eq!(gone, Err(mpsc::RecvTimeoutError::Disconnected));
    assert!(steps.try_recv().is_err());
}

#[test]
fn a_space_name_is_as_long_as_the_channel_takes() {
    assert_eq!(
        crate::channel::protocol::sessions::MAX_SPACE_NAME,
        crate::sessions::spaces::stored::MAX_NAME
    );
}
