//! The tests of finding a file's project, on scratch directories.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;
use std::time::Duration;

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::*;

/// A scratch directory with no links in its path, removed at the end of the test.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let name = format!("oso-project-test-{}-{test}", std::process::id());
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

    /// Whether `detect` found no project inside the scratch directory, which may itself sit
    /// under some project.
    fn none_inside(&self, location: &Path) -> bool {
        detect(location, None).is_none_or(|found| !found.root.path.starts_with(&self.0))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root_of(location: &Path) -> Option<PathBuf> {
    detect(location, None).map(|found| found.root.path)
}

#[test]
fn the_nearest_marker_above_the_file_is_the_root() {
    let scratch = Scratch::new("nearest");
    let outer = scratch.dir("outer");
    scratch.dir("outer/.git");
    let inner = scratch.dir("outer/vendor/lib");
    scratch.dir("outer/vendor/lib/.hg");
    let jj = scratch.dir("outer/other");
    scratch.dir("outer/other/.jj");
    assert_eq!(root_of(&outer.join("src/main.rs")), Some(outer.clone()));
    assert_eq!(root_of(&inner.join("src/lib.rs")), Some(inner));
    assert_eq!(root_of(&jj.join("a.txt")), Some(jj));
    // A new file whose directories do not exist yet is in the project above them.
    assert_eq!(root_of(&outer.join("new/dir/file.rs")), Some(outer));
}

#[test]
fn a_file_with_no_marker_above_it_is_in_no_project_of_its_own() {
    let scratch = Scratch::new("none");
    let file = scratch.file("loose/notes.md", "");
    assert!(scratch.none_inside(&file));
}

#[test]
fn a_marker_that_is_a_file_or_a_link_is_no_marker() {
    let scratch = Scratch::new("kinds");
    let plain = scratch.dir("plain");
    scratch.file("plain/.jj", "not a directory");
    scratch.file("plain/.hg", "");
    assert!(scratch.none_inside(&plain.join("a")));
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let real = scratch.dir("real/.git");
        let linked = scratch.dir("linked");
        symlink(&real, linked.join(".git")).unwrap();
        symlink(&real, linked.join(".hg")).unwrap();
        symlink(&real, linked.join(".jj")).unwrap();
        assert!(scratch.none_inside(&linked.join("a")));
    }
}

#[cfg(unix)]
#[test]
fn a_linked_directory_on_the_way_up_ends_the_walk() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("linked-way");
    scratch.dir("p/.git");
    let elsewhere = scratch.dir("p/x");
    let alias = scratch.0.join("alias");
    symlink(&elsewhere, &alias).unwrap();
    // The loader always gives the real place; a path through a link is not followed.
    assert!(scratch.none_inside(&alias.join("a.rs")));
}

#[test]
fn the_home_directory_is_never_a_project_nor_what_is_above_it() {
    let scratch = Scratch::new("home");
    let home = scratch.dir("home");
    scratch.dir("home/.git");
    let project = scratch.dir("home/src/p");
    scratch.dir("home/src/p/.git");
    assert_eq!(detect(&home.join("notes.md"), Some(&home)), None);
    assert_eq!(detect(&home.join("src/loose/a.rs"), Some(&home)), None);
    assert_eq!(
        detect(&project.join("a.rs"), Some(&home)).map(|found| found.root.path),
        Some(project)
    );
}

#[cfg(unix)]
#[test]
fn the_root_carries_its_device_and_inode() {
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new("identity");
    let root = scratch.dir("p");
    scratch.dir("p/.git");
    let metadata = fs::metadata(&root).unwrap();
    let found = detect(&root.join("a"), None).unwrap();
    assert_eq!(found.root.identity, Some((metadata.dev(), metadata.ino())));
}

#[test]
fn a_worktrees_git_file_is_kept_by_its_words() {
    let scratch = Scratch::new("worktree");
    let worktree = scratch.dir("main/.claude/worktrees/x");
    let named = scratch.0.join("main/.git/worktrees/x");
    fs::write(
        worktree.join(".git"),
        format!("gitdir: {}\n", named.display()),
    )
    .unwrap();
    let found = detect(&worktree.join("src/a.rs"), None).unwrap();
    assert_eq!(found.root.path, worktree);
    assert_eq!(found.worktree_of, Some(scratch.0.join("main")));
}

#[test]
fn what_a_git_file_names_is_never_looked_up() {
    let scratch = Scratch::new("never-followed");
    let remote = scratch.dir("remote");
    scratch.file("remote/.git", "gitdir: /net/host/share/.git/worktrees/a\n");
    let found = detect(&remote.join("a"), None).unwrap();
    let claim = found.worktree_of.expect("a claim");
    // A rooted path keeps the root's drive on Windows, as joining it does.
    #[cfg(unix)]
    assert_eq!(claim, PathBuf::from("/net/host/share"));
    #[cfg(windows)]
    assert!(
        claim.is_absolute() && claim.ends_with("net/host/share"),
        "{claim:?}"
    );
    #[cfg(unix)]
    {
        // A link on the way stays in the words, as nothing resolves them.
        use std::os::unix::fs::symlink;
        scratch.dir("real/.git/worktrees/b");
        symlink(scratch.0.join("real"), scratch.0.join("alias")).unwrap();
        let worktree = scratch.dir("b");
        let named = scratch.0.join("alias/.git/worktrees/b");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", named.display()),
        )
        .unwrap();
        let found = detect(&worktree.join("a"), None).unwrap();
        assert_eq!(found.worktree_of, Some(scratch.0.join("alias")));
    }
}

#[test]
fn a_submodule_or_an_oversized_git_file_names_no_main_repository() {
    let scratch = Scratch::new("not-worktree");
    let sub = scratch.dir("main/sub");
    scratch.dir("main/.git");
    scratch.file("main/sub/.git", "gitdir: ../.git/modules/sub\n");
    let found = detect(&sub.join("a.rs"), None).unwrap();
    assert_eq!(found.root.path, sub);
    assert_eq!(found.worktree_of, None);
    let big = scratch.dir("big");
    let text = format!("gitdir: /w/m/.git/worktrees/a\n{}", "#".repeat(2000));
    fs::write(big.join(".git"), text).unwrap();
    assert_eq!(detect(&big.join("a"), None).unwrap().worktree_of, None);
}

#[test]
fn the_thread_answers_each_request_and_wakes_once_answers_wait() {
    let scratch = Scratch::new("thread");
    let root = scratch.dir("p");
    scratch.dir("p/.git");
    let (woken, wakes) = mpsc::channel();
    let mut detector = Detector::new(None, move || woken.send(()).is_ok());
    let mut editor = Editor::with_buffer(Buffer::new("x"));
    let id = editor.add_buffer(Buffer::new("y"));
    let request = ProjectRequest {
        buffer: id,
        location: root.join("a.rs"),
    };
    detector.find(request.clone());
    wakes.recv_timeout(Duration::from_secs(10)).unwrap();
    let answers = detector.take();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].0, request);
    assert_eq!(
        answers[0].1.as_ref().map(|found| &found.root.path),
        Some(&root)
    );
    assert!(detector.take().is_empty());
}
