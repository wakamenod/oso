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
    fn walked(&self, most: usize) -> (Vec<String>, bool) {
        let mut all = Vec::new();
        let mut cut = None;
        walk(&self.0, most, &|| false, &mut |paths, end| {
            all.extend(paths.into_iter().map(String::from));
            if let Some(end) = end {
                cut = Some(end.cut);
            }
        });
        all.sort();
        (all, cut.expect("the walk ended"))
    }

    fn listed(&self) -> Vec<String> {
        self.walked(MAX_WALKED).0
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
fn links_are_not_followed_but_a_link_to_a_file_is_listed() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("links");
    let outside = Scratch::new("links-outside");
    outside.file("secret/id_rsa", "");
    let target = outside.file("notes.md", "");
    scratch.file("a.rs", "");
    symlink(&target, scratch.0.join("notes.md")).unwrap();
    symlink(outside.0.join("secret"), scratch.0.join("secret")).unwrap();
    symlink(scratch.0.join("missing"), scratch.0.join("dangling")).unwrap();
    assert_eq!(scratch.listed(), ["a.rs", "notes.md"]);
}

#[test]
fn a_walk_lists_at_most_its_limit_and_says_it_was_cut() {
    let scratch = Scratch::new("limit");
    for index in 0..5 {
        scratch.file(&format!("f{index}.rs"), "");
    }
    let (listed, cut) = scratch.walked(3);
    assert_eq!((listed.len(), cut), (3, true));
    let (listed, cut) = scratch.walked(5);
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
    walk(&scratch.0, MAX_WALKED, &stop, &mut |_, _| batches += 1);
    assert_eq!(batches, 0);
    assert_eq!(looked.get(), 4, "stopped as soon as it looked");
}

#[test]
fn the_walker_thread_sends_batches_of_the_live_request_only() {
    let scratch = Scratch::new("thread");
    scratch.file("a.rs", "");
    let (sent, received) = mpsc::channel();
    let mut walker = Walker::new(move |walked| sent.send(walked).is_ok());
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
    // 46,234 files on the author's Mac, and 0.19 s on one thread; this walk took 0.20 s.
    let fastest = (0..3)
        .map(|_| {
            let started = std::time::Instant::now();
            let mut count = 0;
            walk(&tree, MAX_WALKED, &|| false, &mut |paths, _| {
                count += paths.len()
            });
            (started.elapsed(), count)
        })
        .min();
    let (took, count) = fastest.unwrap();
    assert!(took < Duration::from_secs(1), "{count} files in {took:?}");
}
