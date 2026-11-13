//! The tests of walking a project, on scratch directories.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

/// A scratch directory with no links in its path, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-walk-test-{}-{test}", std::process::id());
        let path = fs::canonicalize(std::env::temp_dir()).unwrap().join(name);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    /// Everything a walk of it lists, sorted, and whether it was cut.
    fn walked(&self, limits: Limits) -> (Vec<String>, bool) {
        let mut all = Vec::new();
        let mut cut = None;
        walk(&self.0, None, limits, &|| false, &mut |paths, end| {
            all.extend(paths.into_iter().map(String::from));
            if let Some(end) = end {
                cut = Some(end.cut);
            }
        });
        all.sort();
        (all, cut.expect("the walk ended"))
    }

    fn listed(&self) -> Vec<String> {
        self.walked(Limits::PROJECT).0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hidden_files_are_listed_and_the_projects_markers_never_entered() {
    let scratch = Scratch::new("hidden");
    scratch.dir(".git/objects");
    scratch.file(".git/HEAD", "ref: refs/heads/main\n");
    scratch.file(".jj/repo/store", "");
    scratch.file(".hg/store", "");
    scratch.file(".github/workflows/ci.yml", "");
    scratch.file(".env", "");
    scratch.file("src/main.rs", "");
    assert_eq!(
        scratch.listed(),
        [".env", ".github/workflows/ci.yml", "src/main.rs"]
    );
}

#[test]
fn a_directory_holding_a_marker_is_another_project_and_not_entered() {
    let scratch = Scratch::new("nested");
    scratch.dir(".git");
    scratch.file("a.rs", "");
    scratch.dir("vendor/lib/.git");
    scratch.file("vendor/lib/b.rs", "");
    scratch.file("sub/.git", "gitdir: ../.git/modules/sub\n");
    scratch.file("sub/c.rs", "");
    scratch.dir("other/.hg");
    scratch.file("other/d.rs", "");
    scratch.file("vendor/e.rs", "");
    assert_eq!(scratch.listed(), ["a.rs", "vendor/e.rs"]);
}

#[test]
fn ignore_files_apply_as_they_do_for_rg() {
    let scratch = Scratch::new("ignores");
    scratch.dir(".git/info");
    scratch.file(".git/info/exclude", "excluded.txt\n");
    scratch.file(".gitignore", "target/\n*.log\n");
    scratch.file(".ignore", "ignored.rs\n");
    scratch.file(".rgignore", "rgignored.rs\n");
    scratch.file("docs/.gitignore", "draft.md\n");
    for name in [
        "excluded.txt",
        "target/debug/oso",
        "run.log",
        "ignored.rs",
        "rgignored.rs",
        "docs/draft.md",
        "docs/ui.md",
        "src/main.rs",
    ] {
        scratch.file(name, "");
    }
    assert_eq!(
        scratch.listed(),
        [
            ".gitignore",
            ".ignore",
            ".rgignore",
            "docs/.gitignore",
            "docs/ui.md",
            "src/main.rs"
        ]
    );
}

#[test]
fn a_gitignore_outside_a_git_repository_does_not_apply() {
    let scratch = Scratch::new("no-git");
    scratch.file(".gitignore", "*.log\n");
    scratch.file("run.log", "");
    assert_eq!(scratch.listed(), [".gitignore", "run.log"]);
}

#[cfg(unix)]
#[test]
fn links_are_not_followed_and_one_is_looked_at_only_while_it_stays_under_the_root() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("links");
    let outside = Scratch::new("links-outside");
    outside.file("secret/id_rsa", "");
    scratch.file("a.rs", "");
    scratch.file("sub/b.rs", "");
    // Under the root: a file is listed, a directory and nowhere are not, and no link is entered.
    symlink(scratch.0.join("a.rs"), scratch.0.join("to-a.rs")).unwrap();
    symlink("sub/b.rs", scratch.0.join("to-b.rs")).unwrap();
    symlink("sub", scratch.0.join("to-sub")).unwrap();
    symlink("to-sub/b.rs", scratch.0.join("through-a-link.rs")).unwrap();
    symlink("missing", scratch.0.join("dangling")).unwrap();
    symlink("loop-b", scratch.0.join("loop-a")).unwrap();
    symlink("loop-a", scratch.0.join("loop-b")).unwrap();
    // Out of the root, at any hop, it is listed without a look: one leading nowhere is too.
    symlink(outside.0.join("secret"), scratch.0.join("secret")).unwrap();
    symlink("../nowhere-outside", scratch.0.join("up")).unwrap();
    symlink("to-sub/../../x", scratch.0.join("up-through-a-link")).unwrap();
    assert_eq!(
        scratch.listed(),
        [
            "a.rs",
            "secret",
            "sub/b.rs",
            "through-a-link.rs",
            "to-a.rs",
            "to-b.rs",
            "up",
            "up-through-a-link"
        ]
    );
}

#[cfg(unix)]
#[test]
fn an_ignore_file_that_is_a_link_is_not_read() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("ignore-link");
    let outside = Scratch::new("ignore-link-outside");
    let all = outside.file("all", "*\n");
    scratch.dir(".git/info");
    for name in [".rgignore", ".ignore", ".gitignore", ".git/info/exclude"] {
        symlink(&all, scratch.0.join(name)).unwrap();
    }
    scratch.file("src/main.rs", "");
    assert_eq!(
        scratch.listed(),
        [".gitignore", ".ignore", ".rgignore", "src/main.rs"]
    );
}

#[test]
fn an_ignore_file_past_its_bound_is_not_read_and_a_directory_of_its_name_is_none() {
    let scratch = Scratch::new("ignore-large");
    let large = format!(
        "*\n{}",
        "#".repeat(usize::try_from(MAX_IGNORE_FILE).unwrap())
    );
    scratch.file(".ignore", &large);
    scratch.dir(".rgignore");
    scratch.file(".rgignore/inside.rs", "");
    scratch.file("src/main.rs", "");
    assert_eq!(
        scratch.listed(),
        [".ignore", ".rgignore/inside.rs", "src/main.rs"]
    );
}

#[test]
fn no_ignore_file_above_the_root_is_read() {
    let scratch = Scratch::new("above");
    scratch.file(".ignore", "*\n");
    scratch.file(".gitignore", "*\n");
    scratch.dir(".git");
    scratch.dir("project/.git");
    scratch.file("project/src/main.rs", "");
    let project = Scratch(scratch.0.join("project"));
    assert_eq!(project.listed(), ["src/main.rs"]);
    std::mem::forget(project);
}

#[test]
fn a_deeper_ignore_file_and_a_negation_decide_and_rgignore_comes_first() {
    let scratch = Scratch::new("precedence");
    scratch.dir(".git");
    scratch.file(".gitignore", "*.log\n");
    scratch.file("keep/.gitignore", "!kept.log\n");
    scratch.file("keep/kept.log", "");
    scratch.file("keep/other.log", "");
    scratch.file(".ignore", "*.tmp\n");
    scratch.file(".rgignore", "!wanted.tmp\n");
    scratch.file("wanted.tmp", "");
    scratch.file("unwanted.tmp", "");
    assert_eq!(
        scratch.listed(),
        [
            ".gitignore",
            ".ignore",
            ".rgignore",
            "keep/.gitignore",
            "keep/kept.log",
            "wanted.tmp"
        ]
    );
}

#[cfg(unix)]
#[test]
fn an_ignore_file_that_is_a_pipe_does_not_hold_the_walk() {
    use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
    let scratch = Scratch::new("ignore-pipe");
    scratch.dir(".git/info");
    scratch.file("src/main.rs", "");
    let mkfifo = Executable::from_path("mkfifo", None).expect("mkfifo is on PATH");
    for name in [".gitignore", ".ignore", ".git/info/exclude"] {
        let status = spawn(&Spawn {
            executable: &mkfifo,
            args: &[scratch.0.join(name).into_os_string()],
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .and_then(|mut child| child.wait())
        .expect("cannot run mkfifo");
        assert!(status.success());
    }
    // A pipe is listed as `rg` lists one: not at all, as it is no file.
    assert_eq!(scratch.listed(), ["src/main.rs"]);
}

#[cfg(unix)]
#[test]
fn a_marker_counts_as_detection_counts_it() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("markers");
    let elsewhere = Scratch::new("markers-elsewhere");
    elsewhere.dir("repo/.git");
    // Each of these is another project, as detection would find it.
    scratch.dir("a/.git");
    scratch.file("a/x.rs", "");
    scratch.file("b/.git", "gitdir: ../.git/worktrees/b\n");
    scratch.file("b/x.rs", "");
    scratch.dir("c/.jj");
    scratch.file("c/x.rs", "");
    scratch.dir("d/.hg");
    scratch.file("d/x.rs", "");
    // And none of these is: a link, and a `.jj` or `.hg` that is a file.
    scratch.dir("e");
    symlink(elsewhere.0.join("repo/.git"), scratch.0.join("e/.git")).unwrap();
    scratch.file("e/x.rs", "");
    scratch.file("f/.hg", "");
    scratch.file("f/x.rs", "");
    scratch.file("g/.jj", "");
    scratch.file("g/x.rs", "");
    assert_eq!(scratch.listed(), ["e/x.rs", "f/x.rs", "g/x.rs"]);
}

#[test]
fn a_walk_stops_at_its_bytes_of_paths_and_says_it_was_cut() {
    let scratch = Scratch::new("bytes");
    for index in 0..5 {
        scratch.file(&format!("aaaa/bbbb/f{index}.rs"), "");
    }
    // Each path is 15 bytes; the names read are 33 in all.
    let (listed, cut) = scratch.walked(Limits {
        bytes: 40,
        ..Limits::PROJECT
    });
    assert_eq!((listed.len(), cut), (2, true));
    let (listed, cut) = scratch.walked(Limits {
        bytes: 75,
        ..Limits::PROJECT
    });
    assert_eq!((listed.len(), cut), (5, false));
}

#[test]
fn a_walk_lists_at_most_its_limit_and_says_it_was_cut() {
    let scratch = Scratch::new("limit");
    for index in 0..5 {
        scratch.file(&format!("f{index}.rs"), "");
    }
    let (listed, cut) = scratch.walked(Limits {
        files: 3,
        ..Limits::PROJECT
    });
    assert_eq!((listed.len(), cut), (3, true));
    let (listed, cut) = scratch.walked(Limits {
        files: 5,
        ..Limits::PROJECT
    });
    assert_eq!((listed.len(), cut), (5, false), "exactly the limit is all");
}

#[test]
fn a_stopped_walk_ends_at_its_next_entry_with_no_last_batch() {
    let scratch = Scratch::new("stop");
    for index in 0..50 {
        scratch.file(&format!("d{index}/f.rs"), "");
    }
    let looked = Cell::new(0);
    let stop = || {
        looked.set(looked.get() + 1);
        looked.get() > 3
    };
    let mut batches = 0;
    walk(&scratch.0, None, Limits::PROJECT, &stop, &mut |_, _| {
        batches += 1
    });
    assert_eq!(batches, 0);
    assert_eq!(looked.get(), 4, "stopped as soon as it looked");
}

#[test]
fn the_walker_thread_sends_batches_of_the_live_request_only() {
    let scratch = Scratch::new("thread");
    scratch.file("a.rs", "");
    let (sent, received) = mpsc::channel();
    let mut walker = Walker::new(None, move |walked| sent.send(walked).is_ok());
    let live = Arc::new(AtomicU64::new(2));
    // A request a newer listing has passed is never walked.
    walker.walk(WalkRequest {
        generation: 1,
        root: scratch.0.clone(),
        live: Arc::clone(&live),
    });
    walker.walk(WalkRequest {
        generation: 2,
        root: scratch.0.clone(),
        live,
    });
    let walked = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(walked.generation, 2);
    assert_eq!(walked.paths, vec![Box::from("a.rs")]);
    assert_eq!(walked.end, Some(WalkEnd { cut: false }));
    assert!(!format!("{walked:?}").contains("a.rs"), "no path in Debug");
}

#[test]
#[ignore = "a timing: run by hand in a release build, with OSO_WALK_TREE set to a tree"]
fn a_walk_of_a_large_tree_takes_about_as_long_as_rg() {
    let Some(tree) = std::env::var_os("OSO_WALK_TREE") else {
        return;
    };
    let tree = PathBuf::from(tree);
    // The fastest of three, warm, against a second: `rg --files --hidden` took 0.09 s for
    // 46,234 files on the author's Mac, and 0.19 s on one thread; this walk took 0.20 s. Listing
    // each directory from its parent (step 4), it took 0.139 s, against 0.114 s for step 3's on
    // the same day.
    let fastest = (0..3)
        .map(|_| {
            let started = std::time::Instant::now();
            let mut count = 0;
            walk(&tree, None, Limits::PROJECT, &|| false, &mut |paths, _| {
                count += paths.len()
            });
            (started.elapsed(), count)
        })
        .min();
    let (took, count) = fastest.unwrap();
    assert!(took < Duration::from_secs(1), "{count} files in {took:?}");
}

#[test]
fn a_walk_stops_at_its_entries_read_ignored_ones_too() {
    let scratch = Scratch::new("entries");
    scratch.file(".ignore", "*.o\n");
    for index in 0..20 {
        scratch.file(&format!("{index}.o"), "");
    }
    scratch.file("a.rs", "");
    let (_, cut) = scratch.walked(Limits {
        entries: 10,
        ..Limits::PROJECT
    });
    assert!(cut, "the ignored names count");
    let (listed, cut) = scratch.walked(Limits::PROJECT);
    assert_eq!(
        (listed, cut),
        (vec![".ignore".to_owned(), "a.rs".to_owned()], false)
    );
}

#[test]
fn a_walk_stops_at_its_bytes_of_ignore_files() {
    let scratch = Scratch::new("ignore-bytes");
    for depth in 1..=4 {
        let dir = "d/".repeat(depth);
        scratch.file(&format!("{dir}.ignore"), "*.o\n");
        scratch.file(&format!("{dir}x.rs"), "");
    }
    let (_, cut) = scratch.walked(Limits {
        ignore_bytes: 8,
        ..Limits::PROJECT
    });
    assert!(cut);
}

#[test]
fn the_walker_lists_nothing_of_a_root_that_holds_home_by_its_real_place() {
    let scratch = Scratch::new("home");
    scratch.file("me/notes.md", "");
    let (sent, received) = mpsc::channel();
    let mut walker = Walker::new(Some(scratch.0.join("me")), move |walked| {
        sent.send(walked).is_ok()
    });
    walker.walk(WalkRequest {
        generation: 1,
        root: scratch.0.clone(),
        live: Arc::new(AtomicU64::new(1)),
    });
    let walked = received.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(walked.paths.is_empty());
    assert_eq!(walked.end, Some(WalkEnd { cut: false }));
}

#[cfg(unix)]
#[test]
fn the_names_looked_at_to_resolve_links_count_as_entries_read() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("looks");
    scratch.file("sub/a.rs", "");
    // Each link takes a look at `sub`, at `sub/..`'s names and at `a.rs`.
    for index in 0..5 {
        symlink(
            "sub/../sub/../sub/a.rs",
            scratch.0.join(format!("l{index}")),
        )
        .unwrap();
    }
    let (_, cut) = scratch.walked(Limits {
        entries: 12,
        ..Limits::PROJECT
    });
    assert!(cut, "five links take more looks than twelve");
    let (listed, cut) = scratch.walked(Limits::PROJECT);
    assert_eq!((listed.len(), cut), (6, false));
}

#[test]
fn the_names_a_directory_holds_count_against_the_bytes_of_paths() {
    let scratch = Scratch::new("names");
    scratch.file(".ignore", "*.o\n");
    for index in 0..10 {
        scratch.file(&format!("{index:020}.o"), "");
    }
    let (listed, cut) = scratch.walked(Limits {
        bytes: 100,
        ..Limits::PROJECT
    });
    assert!(listed.is_empty() && cut, "ignored names count too");
}

#[test]
fn an_ignore_file_longer_than_what_is_left_ends_the_walk_before_it_is_read() {
    let scratch = Scratch::new("ignore-left");
    scratch.file(".ignore", "*.o\n");
    scratch.file("d/.ignore", "*.a\n*.b\n");
    scratch.file("d/x.rs", "");
    let (_, cut) = scratch.walked(Limits {
        ignore_bytes: 6,
        ..Limits::PROJECT
    });
    assert!(cut);
    let (listed, cut) = scratch.walked(Limits {
        ignore_bytes: 12,
        ..Limits::PROJECT
    });
    assert_eq!(
        (listed.len(), cut),
        (3, false),
        "exactly what is left is read"
    );
}

mod deep;

#[cfg(unix)]
#[test]
fn a_root_that_is_home_by_its_device_and_inode_lists_nothing_however_it_is_spelled() {
    let scratch = Scratch::new("home-identity");
    scratch.file("home/me/notes.txt", "");
    std::os::unix::fs::symlink(scratch.0.join("home"), scratch.0.join("also-home")).unwrap();
    let home = scratch.0.join("home/me");
    for root in [
        scratch.0.join("also-home/me"),
        scratch.0.join("also-home"),
        home.clone(),
    ] {
        let mut listed = Vec::new();
        walk(
            &root,
            Some(&home),
            Limits::PROJECT,
            &|| false,
            &mut |paths, _| {
                listed.extend(paths);
            },
        );
        assert!(listed.is_empty(), "{}", root.display());
    }
    let mut listed = Vec::new();
    walk(
        &home,
        Some(&scratch.0.join("elsewhere")),
        Limits::PROJECT,
        &|| false,
        &mut |paths, _| {
            listed.extend(paths);
        },
    );
    assert_eq!(listed.len(), 1);
}
