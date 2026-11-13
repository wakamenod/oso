//! Worktree sessions over temporary repositories: making a worktree, `.worktreeinclude`, the
//! five checks of a removal in order, merge detection by ancestry and by a fake `gh`, and the
//! removal itself, which keeps the branch.
//!
//! Must not depend on: `app`, `dispatch` or winit; these run the jobs' functions directly, and
//! git only through the process module.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use oso_platform::privacy::PrivateDir;
use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};

use super::checks::{self, Check, Depth, Held};
use super::create::{self, Order};
use super::git::{Gh, Repo};
use super::merged::{self, Merged};
use super::porcelain::Listed;

/// A temporary repository with a commit on `main`, and a private directory for git's hooks.
struct Scratch {
    dir: PathBuf,
    root: PathBuf,
    hooks: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!("oso-worktree-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let root = dir.join("repo");
        fs::create_dir(&root).unwrap();
        let hooks = PrivateDir::open(dir.join("state/hooks")).unwrap();
        let scratch = Scratch {
            hooks: hooks.path().to_owned(),
            dir,
            root,
        };
        scratch.git(&["init", "-q", "-b", "main"]);
        fs::write(scratch.root.join("a.txt"), "a\n").unwrap();
        fs::write(scratch.root.join(".gitignore"), ".env\ntarget/\n").unwrap();
        scratch.git(&["add", "."]);
        scratch.git(&["commit", "-q", "-m", "first"]);
        scratch
    }

    fn repo(&self) -> Repo {
        Repo::open(&self.root, &self.hooks).unwrap()
    }

    /// git for the test's own setup, in `dir` relative to the repository, with a fixed
    /// identity and no configuration of the person's.
    fn git_in(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
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
            cwd: Some(dir),
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

    fn git(&self, args: &[&str]) -> Vec<u8> {
        self.git_in(&self.root, args)
    }

    /// A bare repository beside this one as `origin`, with `main` pushed.
    fn with_origin(&self) {
        let bare = self.dir.join("origin.git");
        self.git_in(&self.dir, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        self.git(&["remote", "add", "origin", bare.to_str().unwrap()]);
        self.git(&["push", "-q", "origin", "main"]);
    }

    /// Makes a worktree session's worktree on `branch` with a brief.
    fn create(&self, branch: &str) -> Result<create::Created, String> {
        let order = Order {
            branch: branch.to_owned(),
            base: None,
            brief: "Do the thing.".to_owned(),
            from: None,
        };
        create::create(&self.repo(), &order, &|_| {})
    }

    /// The listed worktree at `path`.
    fn listed(&self, path: &Path) -> Listed {
        self.repo()
            .worktrees()
            .unwrap()
            .into_iter()
            .find(|w| oso_core::project::same_place(&w.path, path))
            .unwrap()
    }

    /// The first check of a removal of `path` that refuses, with `held` from the UI.
    fn refusal(&self, path: &Path, held: &Held, gh: Option<&Gh>) -> Option<Check> {
        let repo = self.repo();
        let looked = checks::look(&repo, gh, self.listed(path), held, Depth::UntilRefused);
        checks::removable(&looked, held).err()
    }

    /// A fake `gh` that prints `answer` and exits with `status`.
    fn fake_gh(&self, answer: &str, status: u8) -> Gh {
        let n = fs::read_dir(&self.dir).unwrap().count();
        let answer_file = self.dir.join(format!("gh-answer-{n}.json"));
        fs::write(&answer_file, answer).unwrap();
        let script = self.dir.join(format!("gh-{n}"));
        fs::write(
            &script,
            format!(
                "#!/bin/sh\ncat '{}'\nexit {status}\n",
                answer_file.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Gh::at(Executable::absolute(script).unwrap())
    }
}

#[test]
fn a_new_branch_gets_a_worktree_under_claude_worktrees() {
    let s = Scratch::new("create");
    let made = s.create("feat/One").unwrap();
    assert_eq!(made.path, s.root.join(".claude/worktrees/feat-one"));
    assert!(made.path.join("a.txt").is_file());
    assert!(made.made_branch);
    assert_eq!(made.branch, "feat/One");
    assert!(
        made.prompt.starts_with(&format!(
            "You are in a git worktree of {} at {}, on branch feat/One from main.",
            s.root.display(),
            made.path.display()
        )),
        "{}",
        made.prompt
    );
    assert!(made.prompt.ends_with("Do the thing."));
    let listed = s.listed(&made.path);
    assert_eq!(listed.branch.as_deref(), Some("feat/One"));
}

#[test]
fn an_existing_branch_is_checked_out_and_one_in_use_is_refused() {
    let s = Scratch::new("existing");
    s.git(&["branch", "old"]);
    let made = s.create("old").unwrap();
    assert!(!made.made_branch);
    let again = s.create("old").unwrap_err();
    assert!(again.contains("checked out already"), "{again}");
    let main = s.create("main").unwrap_err();
    assert!(main.contains("checked out already"), "{main}");
}

#[test]
fn bad_names_and_bases_are_refused_before_anything_is_made() {
    let s = Scratch::new("refused");
    for name in ["-x", "a..b", "a\u{7}b", "", "a b"] {
        let refused = s.create(name).unwrap_err();
        assert!(
            refused.starts_with("Not a branch name"),
            "{name:?}: {refused}"
        );
    }
    let order = Order {
        branch: "new".to_owned(),
        base: Some("no-such-commit".to_owned()),
        brief: "x".to_owned(),
        from: None,
    };
    let refused = create::create(&s.repo(), &order, &|_| {}).unwrap_err();
    assert!(refused.starts_with("Not a commit"), "{refused}");
    let empty = Order {
        brief: "  ".to_owned(),
        base: None,
        ..order
    };
    assert!(create::create(&s.repo(), &empty, &|_| {}).is_err());
    assert!(!s.root.join(".claude/worktrees/new").exists());
}

#[test]
fn a_link_where_the_worktrees_go_is_refused() {
    let s = Scratch::new("link");
    let elsewhere = s.dir.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, s.root.join(".claude")).unwrap();
    let refused = s.create("x").unwrap_err();
    assert!(refused.contains("is a link"), "{refused}");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn the_repositorys_hooks_do_not_run() {
    let s = Scratch::new("hooks");
    let ran = s.dir.join("hook-ran");
    let hook = s.root.join(".git/hooks/post-checkout");
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    s.create("hooked").unwrap();
    assert!(!ran.exists(), "a repository's hook ran");
}

#[test]
fn worktreeinclude_copies_ignored_matching_files() {
    let s = Scratch::new("include");
    fs::write(s.root.join(".env"), "SECRET=1\n").unwrap();
    fs::write(s.root.join(".worktreeinclude"), ".env\na.txt\n").unwrap();
    s.git(&["add", ".worktreeinclude"]);
    s.git(&["commit", "-q", "-m", "include"]);
    let made = s.create("inc").unwrap();
    assert_eq!(
        fs::read_to_string(made.path.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    // Tracked, so it came with the checkout, not as a copy.
    assert_eq!(made.copied.files, 1);
}

#[test]
fn the_checks_refuse_in_order() {
    let s = Scratch::new("checks");
    s.with_origin();
    let made = s.create("work").unwrap();
    let path = made.path;
    // 1: the main worktree.
    let main = s.refusal(&s.root, &Held::default(), None);
    assert!(matches!(main, Some(Check::NotLinked(_))), "{main:?}");
    // 2 before 3: untracked, with an unsaved buffer too.
    fs::write(path.join("new.txt"), "n").unwrap();
    let held = Held {
        unsaved: 1,
        ..Held::default()
    };
    let dirty = s.refusal(&path, &held, None);
    assert!(
        matches!(dirty, Some(Check::Dirty(st)) if st.untracked == 1),
        "{dirty:?}"
    );
    fs::remove_file(path.join("new.txt")).unwrap();
    // 3, then 4.
    assert_eq!(s.refusal(&path, &held, None), Some(Check::Unsaved(1)));
    let unsent = Held {
        unsent: true,
        ..Held::default()
    };
    assert_eq!(s.refusal(&path, &unsent, None), Some(Check::Unsent));
    let busy = Held {
        busy: vec!["work".into()],
        ..Held::default()
    };
    assert_eq!(
        s.refusal(&path, &busy, None),
        Some(Check::Busy(vec!["work".into()]))
    );
    // 5: no commit made on the branch, so not merged, though its tip is in origin/main.
    assert!(matches!(
        s.refusal(&path, &Held::default(), None),
        Some(Check::NotMerged(_))
    ));
}

#[test]
fn a_merged_branch_is_removed_and_kept() {
    let s = Scratch::new("remove");
    s.with_origin();
    let made = s.create("done").unwrap();
    fs::write(made.path.join("b.txt"), "b\n").unwrap();
    s.git_in(&made.path, &["add", "b.txt"]);
    s.git_in(&made.path, &["commit", "-q", "-m", "work"]);
    // Not merged yet.
    let refused = s.refusal(&made.path, &Held::default(), None);
    assert!(matches!(refused, Some(Check::NotMerged(_))), "{refused:?}");
    s.git(&["merge", "-q", "done"]);
    s.git(&["push", "-q", "origin", "main"]);
    assert_eq!(s.refusal(&made.path, &Held::default(), None), None);
    let repo = s.repo();
    let tip = merged::tip(&repo, "done").unwrap();
    assert_eq!(
        merged::merged(&repo, None, "done", &tip),
        Merged::Ancestor("origin/main".into())
    );
    checks::remove(&repo, &made.path).unwrap();
    assert!(!made.path.exists());
    assert!(
        !s.git(&["branch", "--list", "done"]).is_empty(),
        "the branch is kept"
    );
}

#[test]
fn git_refuses_a_worktree_that_changed_after_its_checks() {
    let s = Scratch::new("late");
    let made = s.create("late").unwrap();
    fs::write(made.path.join("a.txt"), "changed\n").unwrap();
    assert!(checks::remove(&s.repo(), &made.path).is_err());
    assert!(made.path.join("a.txt").is_file(), "nothing was removed");
}

#[test]
fn a_squash_merge_is_found_by_its_pull_request() {
    let s = Scratch::new("squash");
    s.with_origin();
    s.git(&["remote", "set-url", "origin", "https://github.com/o/r.git"]);
    let made = s.create("sq").unwrap();
    fs::write(made.path.join("b.txt"), "b\n").unwrap();
    s.git_in(&made.path, &["add", "b.txt"]);
    s.git_in(&made.path, &["commit", "-q", "-m", "work"]);
    let repo = s.repo();
    let tip = merged::tip(&repo, "sq").unwrap();
    let answer = |head: &str, base: &str| {
        format!(r#"[{{"number":7,"headRefOid":"{head}","baseRefName":"{base}"}}]"#)
    };
    let gh = s.fake_gh(&answer(&tip, "main"), 0);
    assert_eq!(
        merged::merged(&repo, Some(&gh), "sq", &tip),
        Merged::PullRequest(7)
    );
    assert_eq!(s.refusal(&made.path, &Held::default(), Some(&gh)), None);
    for (gh, why) in [
        (
            s.fake_gh(&answer(&"0".repeat(40), "main"), 0),
            "no merged pull request",
        ),
        (
            s.fake_gh(&answer(&tip, "feature"), 0),
            "no merged pull request",
        ),
        (s.fake_gh("not json", 0), "unexpected answer"),
        (s.fake_gh("[]", 1), "gh failed"),
    ] {
        let found = merged::merged(&repo, Some(&gh), "sq", &tip);
        assert!(
            matches!(&found, Merged::No(said) if said.contains(why)),
            "{found:?}"
        );
    }
    let found = merged::merged(&repo, None, "sq", &tip);
    assert!(matches!(&found, Merged::No(said) if said.contains("gh is not found")));
}

#[test]
fn no_remote_is_not_merged() {
    let s = Scratch::new("no-remote");
    let repo = s.repo();
    let tip = merged::tip(&repo, "main").unwrap();
    let found = merged::merged(&repo, None, "main", &tip);
    assert!(
        matches!(&found, Merged::No(said) if said.contains("no remote")),
        "{found:?}"
    );
}

#[test]
fn a_directory_that_is_no_linked_worktree_is_refused() {
    let s = Scratch::new("not-linked");
    let made = s.create("real").unwrap();
    // A worktree's `.git` that names this repository, but which it does not record back.
    let fake = s.dir.join("fake");
    fs::create_dir(&fake).unwrap();
    let git_file = fs::read(made.path.join(".git")).unwrap();
    fs::write(fake.join(".git"), git_file).unwrap();
    assert!(s.repo().in_worktree(&fake).is_none());
    assert!(s.repo().in_worktree(&made.path).is_some());
}
