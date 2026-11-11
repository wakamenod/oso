//! The tests of finding a file's project, on scratch directories.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::fs;

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

    /// A main repository at `main` and a worktree of it at `worktree`, linked both ways as
    /// `git worktree add` links them.
    fn worktree(&self, main: &str, worktree: &str) -> (PathBuf, PathBuf) {
        let main = self.dir(main);
        let gitdir = self.dir(&format!("{}/.git/worktrees/wt", main.display()));
        let worktree = self.dir(worktree);
        let git_file = worktree.join(".git");
        fs::write(&git_file, format!("gitdir: {}\n", gitdir.display())).unwrap();
        fs::write(gitdir.join("gitdir"), format!("{}\n", git_file.display())).unwrap();
        (main, worktree)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn root_of(location: &Path) -> Option<PathBuf> {
    detect(location).map(|found| found.root.path)
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
    // A new file whose directory does not exist yet is in the project above it.
    assert_eq!(root_of(&outer.join("new/dir/file.rs")), Some(outer));
}

#[test]
fn a_file_with_no_marker_above_it_is_in_no_project_of_its_own() {
    let scratch = Scratch::new("none");
    let file = scratch.file("loose/notes.md", "");
    // The scratch directory itself may sit under some project; none is inside it.
    assert!(root_of(&file).is_none_or(|root| !root.starts_with(&scratch.0)));
}

#[test]
fn a_file_or_an_empty_name_is_no_marker_but_a_git_file_is() {
    let scratch = Scratch::new("kinds");
    let plain = scratch.dir("plain");
    scratch.file("plain/.jj", "not a directory");
    scratch.file("plain/.hg", "");
    assert!(root_of(&plain.join("a")).is_none_or(|root| !root.starts_with(&scratch.0)));
    let submodule = scratch.dir("sub");
    scratch.file("sub/.git", "gitdir: ../.git/modules/sub\n");
    let found = detect(&submodule.join("a")).unwrap();
    assert_eq!(found.root.path, submodule);
    assert_eq!(found.trust_root, None);
}

#[cfg(unix)]
#[test]
fn the_root_carries_its_device_and_inode() {
    use std::os::unix::fs::MetadataExt;
    let scratch = Scratch::new("identity");
    let root = scratch.dir("p");
    scratch.dir("p/.git");
    let metadata = fs::metadata(&root).unwrap();
    let found = detect(&root.join("a")).unwrap();
    assert_eq!(found.root.identity, Some((metadata.dev(), metadata.ino())));
}

#[test]
fn a_worktree_linked_both_ways_takes_its_main_repository() {
    let scratch = Scratch::new("worktree");
    let (main, worktree) = scratch.worktree("main", "main/.claude/worktrees/x");
    let found = detect(&worktree.join("src/a.rs")).unwrap();
    assert_eq!(found.root.path, worktree);
    assert_eq!(found.trust_root.map(|root| root.path), Some(main.clone()));
    // The main repository's own files are its own.
    assert_eq!(detect(&main.join("a.rs")).unwrap().trust_root, None);
}

#[test]
fn a_worktree_somewhere_else_takes_its_main_repository_too() {
    let scratch = Scratch::new("worktree-apart");
    let (main, worktree) = scratch.worktree("main", "elsewhere/x");
    let found = detect(&worktree.join("a.rs")).unwrap();
    assert_eq!(found.trust_root.map(|root| root.path), Some(main));
}

#[test]
fn a_git_file_that_claims_a_repository_without_its_link_back_takes_nothing() {
    let scratch = Scratch::new("forged");
    let (_, worktree) = scratch.worktree("main", "real");
    // Another repository names the same worktree directory, but the link back names the real
    // worktree, as only the main repository's own `.git` can say.
    let forged = scratch.dir("forged");
    let gitdir = fs::read_to_string(worktree.join(".git")).unwrap();
    fs::write(forged.join(".git"), gitdir).unwrap();
    let found = detect(&forged.join("a.rs")).unwrap();
    assert_eq!(found.root.path, forged);
    assert_eq!(found.trust_root, None);
}

#[test]
fn a_git_file_naming_no_worktrees_directory_takes_nothing() {
    let scratch = Scratch::new("not-worktree");
    scratch.dir("main");
    let modules = scratch.dir("main/.git/modules/sub");
    let sub = scratch.dir("main/sub");
    let git_file = sub.join(".git");
    fs::write(&git_file, format!("gitdir: {}\n", modules.display())).unwrap();
    fs::write(modules.join("gitdir"), format!("{}\n", git_file.display())).unwrap();
    let found = detect(&sub.join("a.rs")).unwrap();
    assert_eq!(found.root.path, sub);
    assert_eq!(found.trust_root, None);
}

#[test]
fn an_oversized_git_file_is_not_read_as_a_link() {
    let scratch = Scratch::new("oversized");
    let (_, worktree) = scratch.worktree("main", "wt");
    let git_file = worktree.join(".git");
    let mut text = fs::read_to_string(&git_file).unwrap();
    text.push_str(&"#".repeat(2000));
    fs::write(&git_file, text).unwrap();
    assert_eq!(detect(&worktree.join("a")).unwrap().trust_root, None);
}

#[cfg(unix)]
#[test]
fn a_git_that_links_to_a_worktrees_git_takes_nothing() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("linked");
    let (_, worktree) = scratch.worktree("main", "real");
    let absolute = scratch.dir("absolute");
    symlink(worktree.join(".git"), absolute.join(".git")).unwrap();
    let relative = scratch.dir("deep/relative");
    symlink("../../real/.git", relative.join(".git")).unwrap();
    for linked in [absolute, relative] {
        let found = detect(&linked.join("a.rs")).unwrap();
        assert_eq!(found.root.path, linked);
        assert_eq!(found.trust_root, None, "{}", linked.display());
    }
    // The worktree itself still takes its main repository.
    assert!(detect(&worktree.join("a.rs")).unwrap().trust_root.is_some());
}

#[cfg(unix)]
#[test]
fn a_worktree_reached_through_a_linked_directory_takes_nothing() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("linked-dir");
    let (_, worktree) = scratch.worktree("main", "real");
    let alias = scratch.0.join("alias");
    symlink(&worktree, &alias).unwrap();
    // A location that is not its own real path names no worktree; the loader always gives one
    // that is.
    assert_eq!(detect(&alias.join("a.rs")).unwrap().trust_root, None);
}
