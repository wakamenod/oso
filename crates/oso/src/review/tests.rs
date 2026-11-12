//! Reviews of temporary repositories: each scope, the baseline, refreshing, and the limits.
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the review's reader and thread
//! directly, and run git only through the process module.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use oso_core::review::{Anchor, Author, Review, Side};
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::list::Status;
use super::work::Reader;
use super::{Content, Entry, Id, Reading, Report, Request, Reviewer, Scope, TrustedRoot};

mod guards;

/// A fresh directory for one test, with no links in its path.
pub(crate) fn scratch_dir(name: &str) -> PathBuf {
    let name = format!("oso-review-test-{}-{name}", std::process::id());
    let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
    let _ = fs::remove_dir_all(&path);
    fs::create_dir(&path).unwrap();
    path
}

/// A temporary repository, with a state directory beside it.
struct Repo {
    dir: PathBuf,
    root: PathBuf,
}

impl Repo {
    fn new(name: &str) -> Repo {
        let dir = scratch_dir(name);
        let root = dir.join("repo");
        fs::create_dir(&root).unwrap();
        let repo = Repo { dir, root };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }

    /// Runs git in the repository for the test's own setup, with a fixed identity and no
    /// configuration of the person's.
    fn git(&self, args: &[&str]) -> Vec<u8> {
        use std::io::Read;
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
        let home = self.dir.as_os_str();
        let mut child = spawn(&Spawn {
            executable: &git,
            args: &all,
            cwd: Some(&self.root),
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Piped,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[("HOME", home), ("GIT_CONFIG_NOSYSTEM", "1".as_ref())],
            terminal: None,
        })
        .unwrap();
        let mut out = Vec::new();
        child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        assert!(child.wait().unwrap().success(), "git {args:?}");
        out
    }

    fn write(&self, path: &str, text: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
    }

    fn reader(&self) -> Reader {
        Reader::new(&self.root, Some(&self.dir.join("state")))
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// What one request reported.
#[derive(Default)]
struct Answers {
    entries: Vec<Entry>,
    readings: Vec<Arc<Reading>>,
    /// Whether the `Read` that ended the request said it stopped.
    stopped: Option<bool>,
    other: Vec<Report>,
}

impl Answers {
    fn status(&self, path: &str) -> Option<Status> {
        self.entries
            .iter()
            .find(|entry| &*entry.path == path.as_bytes())
            .map(|entry| entry.status)
    }

    fn reading(&self, path: &str) -> &Reading {
        self.readings
            .iter()
            .find(|reading| &*reading.entry.path == path.as_bytes())
            .unwrap_or_else(|| panic!("no reading of {path}"))
    }

    fn paths(&self) -> Vec<String> {
        self.readings
            .iter()
            .map(|reading| String::from_utf8_lossy(&reading.entry.path).into_owned())
            .collect()
    }
}

fn ask(reader: &mut Reader, request: Request) -> Answers {
    let mut answers = Answers::default();
    reader.answer(request, &|| false, &mut |report| {
        match report {
            Report::Listed { entries, .. } => answers.entries = entries.to_vec(),
            Report::File { reading, .. } => answers.readings.push(reading),
            Report::Read { stopped, .. } => answers.stopped = Some(stopped),
            other => answers.other.push(other),
        }
        true
    });
    answers
}

fn open(reader: &mut Reader, scope: Scope) -> Answers {
    ask(reader, Request::Open(scope))
}

/// The new side's lines a text reading adds, as text.
fn added(reading: &Reading) -> Vec<String> {
    let Content::Text { shape, .. } = &reading.content else {
        panic!("{reading:?} is no text");
    };
    shape
        .hunks()
        .iter()
        .flat_map(|hunk| &hunk.added)
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect()
}

fn loose_objects(repo: &Repo) -> usize {
    let objects = repo.root.join(".git/objects");
    fs::read_dir(objects)
        .unwrap()
        .flatten()
        .filter(|dir| dir.file_name().len() == 2)
        .map(|dir| fs::read_dir(dir.path()).unwrap().count())
        .sum()
}

#[test]
fn uncommitted_lists_changes_and_untracked_files_and_leaves_the_index_alone() {
    let repo = Repo::new("uncommitted");
    repo.write("a.txt", b"one\ntwo\n");
    repo.write("gone.txt", b"x\n");
    repo.write(".gitignore", b"*.log\n");
    repo.commit("first");
    repo.write("a.txt", b"one\nTWO\n");
    fs::remove_file(repo.root.join("gone.txt")).unwrap();
    repo.write("dir/new.txt", b"fresh\n");
    repo.write("noise.log", b"ignored\n");
    let index = fs::read(repo.root.join(".git/index")).unwrap();
    let objects = loose_objects(&repo);
    let answers = open(&mut repo.reader(), Scope::Uncommitted);
    assert_eq!(answers.entries.len(), 3, "{:?}", answers.paths());
    assert_eq!(answers.status("a.txt"), Some(Status::Modified));
    assert_eq!(answers.status("gone.txt"), Some(Status::Deleted));
    assert_eq!(answers.status("dir/new.txt"), Some(Status::Added));
    assert_eq!(added(answers.reading("a.txt")), ["TWO\n"]);
    assert_eq!(answers.reading("a.txt").counts(), Some((1, 1)));
    let Content::Text { old, new, .. } = &answers.reading("a.txt").content else {
        panic!()
    };
    assert_eq!((&**old, &**new), (&b"one\ntwo\n"[..], &b"one\nTWO\n"[..]));
    assert_eq!(added(answers.reading("dir/new.txt")), ["fresh\n"]);
    assert!(added(answers.reading("gone.txt")).is_empty());
    assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), index);
    // Marking untracked files writes at most the empty blob.
    assert!(loose_objects(&repo) <= objects + 1);
    let copies = repo.dir.join("state/reviews/tmp");
    assert_eq!(
        fs::read_dir(copies).unwrap().count(),
        0,
        "copies are removed"
    );
}

#[test]
fn staged_and_unstaged_split_the_changes_at_the_index() {
    let repo = Repo::new("staged");
    repo.write("a.txt", b"1\n2\n3\n");
    repo.commit("first");
    repo.write("a.txt", b"1\nstaged\n3\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", b"1\nstaged\n3\nunstaged\n");
    repo.write("untracked.txt", b"u\n");
    let mut reader = repo.reader();
    let staged = open(&mut reader, Scope::Staged);
    assert_eq!(added(staged.reading("a.txt")), ["staged\n"]);
    assert_eq!(staged.entries.len(), 1);
    let unstaged = open(&mut reader, Scope::Unstaged);
    assert_eq!(added(unstaged.reading("a.txt")), ["unstaged\n"]);
    assert_eq!(unstaged.status("untracked.txt"), None, "only tracked files");
}

#[test]
fn a_commit_is_read_against_its_first_parent_and_a_root_against_nothing() {
    let repo = Repo::new("commits");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.write("a.txt", b"a\nb\n");
    repo.commit("second");
    repo.write("a.txt", b"a\nb\nc\n");
    repo.commit("third");
    let mut reader = repo.reader();
    let root = open(
        &mut reader,
        Scope::Commit {
            revision: "HEAD~2".into(),
        },
    );
    assert_eq!(root.status("a.txt"), Some(Status::Added));
    let second = open(
        &mut reader,
        Scope::Commit {
            revision: "HEAD~1".into(),
        },
    );
    assert_eq!(added(second.reading("a.txt")), ["b\n"]);
    let range = open(
        &mut reader,
        Scope::Range {
            from: "HEAD~1".into(),
            to: "HEAD".into(),
        },
    );
    assert_eq!(added(range.reading("a.txt")), ["b\n", "c\n"]);
}

#[test]
fn a_branch_is_read_from_where_it_left_its_base() {
    let repo = Repo::new("branch");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("a.txt", b"a\ntopic\n");
    repo.commit("on topic");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("main.txt", b"m\n");
    repo.commit("on main");
    repo.git(&["checkout", "-q", "topic"]);
    repo.write("wip.txt", b"w\n");
    let answers = open(
        &mut repo.reader(),
        Scope::Branch {
            base: "main".into(),
        },
    );
    assert_eq!(added(answers.reading("a.txt")), ["topic\n"]);
    assert_eq!(answers.status("wip.txt"), Some(Status::Added));
    assert_eq!(
        answers.status("main.txt"),
        None,
        "main's own commits are not the branch's"
    );
}

#[test]
fn a_session_is_read_against_its_baseline_untracked_files_included() {
    let repo = Repo::new("session");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.write("notes.txt", b"before\n");
    let mut reader = repo.reader();
    let taken = ask(&mut reader, Request::Baseline);
    let [Report::Baseline { tree, left_out: 0 }] = &taken.other[..] else {
        panic!("{:?}", taken.other);
    };
    repo.write("notes.txt", b"before\nafter\n");
    repo.write("later.txt", b"l\n");
    let answers = open(
        &mut reader,
        Scope::Session {
            baseline: tree.clone(),
        },
    );
    assert_eq!(answers.status("notes.txt"), Some(Status::Modified));
    assert_eq!(added(answers.reading("notes.txt")), ["after\n"]);
    assert_eq!(answers.status("later.txt"), Some(Status::Added));
    assert_eq!(answers.status("a.txt"), None);
    let gone = Id::parse(&[b'1'; 40]).unwrap();
    let answers = open(&mut reader, Scope::Session { baseline: gone });
    assert!(matches!(answers.other[..], [Report::BaselineGone]));
}

#[test]
fn a_refresh_reads_only_what_changed_and_shown_files_first() {
    let repo = Repo::new("refresh");
    for name in ["a", "b", "c"] {
        repo.write(&format!("{name}.txt"), b"x\n");
    }
    repo.commit("root");
    for name in ["a", "b", "c"] {
        repo.write(&format!("{name}.txt"), b"y\n");
    }
    let mut reader = repo.reader();
    assert_eq!(open(&mut reader, Scope::Uncommitted).readings.len(), 3);
    let again = ask(&mut reader, Request::Refresh { first: Vec::new() });
    assert_eq!(again.entries.len(), 3);
    assert!(again.readings.is_empty(), "{:?}", again.paths());
    repo.write("b.txt", b"z\nz\n");
    repo.write("c.txt", b"w\nw\n");
    let first = vec![Box::from(&b"c.txt"[..])];
    let changed = ask(&mut reader, Request::Refresh { first });
    assert_eq!(changed.paths(), ["c.txt", "b.txt"]);
}

#[test]
fn secrets_binaries_large_files_and_links_are_not_shown_as_text() {
    let repo = Repo::new("kinds");
    repo.write("keep.txt", b"k\n");
    repo.commit("root");
    repo.write(".env", b"TOKEN=1\n");
    repo.write("blob.bin", b"a\0b\n");
    repo.write("huge.txt", &vec![b'x'; 9 * 1024 * 1024]);
    let answers = open(&mut repo.reader(), Scope::Uncommitted);
    assert!(matches!(answers.reading(".env").content, Content::Withheld));
    assert!(matches!(
        answers.reading("blob.bin").content,
        Content::Binary { old: 0, new: 4 }
    ));
    assert!(matches!(
        answers.reading("huge.txt").content,
        Content::TooLarge {
            old: 0,
            new: 9_437_184
        }
    ));
}

#[cfg(unix)]
#[test]
fn a_link_is_read_as_its_target_and_never_followed() {
    let repo = Repo::new("link");
    repo.write("keep.txt", b"k\n");
    repo.commit("root");
    let outside = repo.dir.join("outside.txt");
    fs::write(&outside, b"not for the review\n").unwrap();
    std::os::unix::fs::symlink(&outside, repo.root.join("link")).unwrap();
    let answers = open(&mut repo.reader(), Scope::Uncommitted);
    assert_eq!(
        added(answers.reading("link")),
        [outside.to_string_lossy().into_owned()]
    );
}

#[test]
fn revisions_that_read_as_options_or_name_nothing_are_refused() {
    let repo = Repo::new("revisions");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    let mut reader = repo.reader();
    for revision in ["--output=x", "no-such-branch"] {
        let answers = open(
            &mut reader,
            Scope::Commit {
                revision: revision.into(),
            },
        );
        assert!(
            matches!(answers.other[..], [Report::Failed { .. }]),
            "{revision}"
        );
        assert!(answers.entries.is_empty());
    }
    assert!(!repo.root.join("x").exists());
}

#[test]
fn an_unborn_branch_is_read_against_the_empty_tree() {
    let repo = Repo::new("unborn");
    repo.write("a.txt", b"a\n");
    let answers = open(&mut repo.reader(), Scope::Uncommitted);
    assert_eq!(answers.status("a.txt"), Some(Status::Added));
}

#[test]
fn comments_are_carried_across_a_moved_old_side() {
    let repo = Repo::new("carry");
    repo.write("a.txt", b"1\n2\n3\n");
    repo.commit("root");
    repo.write("a.txt", b"1\n2\nnew\n3\n");
    let mut reader = repo.reader();
    let first = open(&mut reader, Scope::Uncommitted);
    let Content::Text { shape, .. } = &first.reading("a.txt").content else {
        panic!()
    };
    let mut review = Review::new();
    review.apply(
        review
            .carry(b"a.txt", Arc::clone(shape))
            .run(|_, _| None, |_, _| Vec::new()),
    );
    let line = Anchor::Line {
        side: Side::New,
        number: 3,
    };
    let id = review
        .add_comment(Author::Person, b"a.txt", line, "why?".into())
        .unwrap();
    // A line goes in above in a new commit, and the comment's line moves down by one.
    repo.write("a.txt", b"0\n1\n2\n3\n");
    repo.commit("above");
    repo.write("a.txt", b"0\n1\n2\nnew\n3\n");
    let second = open(&mut reader, Scope::Uncommitted);
    let Content::Text { shape, .. } = &second.reading("a.txt").content else {
        panic!()
    };
    let carried = ask(
        &mut reader,
        Request::Carry(review.carry(b"a.txt", Arc::clone(shape))),
    );
    let Some(Report::Carried { carried, .. }) = carried.other.into_iter().next() else {
        panic!("not carried");
    };
    assert_eq!(review.apply(carried).placed, 1);
    let comment = review.comment(id).unwrap();
    assert_eq!(
        comment.anchor(),
        Anchor::Line {
            side: Side::New,
            number: 4
        }
    );
}

#[test]
fn the_thread_answers_in_turn_and_ends_with_its_reviewer() {
    let repo = Repo::new("thread");
    repo.write("a.txt", b"a\n");
    repo.commit("root");
    repo.write("a.txt", b"b\n");
    let (sent, got) = mpsc::channel();
    let root = TrustedRoot::for_tests(repo.root.clone());
    let reviewer = Reviewer::start(root, Some(repo.dir.join("state")), move |report| {
        sent.send(format!("{report:?}")).is_ok()
    })
    .unwrap();
    assert!(reviewer.ask(Request::Open(Scope::Uncommitted)));
    let wait = Duration::from_secs(20);
    assert!(got.recv_timeout(wait).unwrap().starts_with("Listed"));
    let file = got.recv_timeout(wait).unwrap();
    assert!(
        file.contains("a.txt") && file.contains("text, 1 hunks"),
        "{file}"
    );
    let read = got.recv_timeout(wait).unwrap();
    assert!(
        read.starts_with("Read") && read.contains("stopped: false"),
        "{read}"
    );
    drop(reviewer);
    assert!(got.recv_timeout(wait).is_err(), "the thread ends");
}

#[test]
fn a_root_with_no_repository_says_so() {
    let dir = scratch_dir("no-repository");
    let mut reader = Reader::new(&dir, None);
    let answers = open(&mut reader, Scope::Uncommitted);
    assert!(matches!(answers.other[..], [Report::Failed { .. }]));
    fs::remove_dir_all(dir).unwrap();
}
