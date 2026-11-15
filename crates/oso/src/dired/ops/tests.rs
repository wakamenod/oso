//! Tests of the directory editor's file operations on scratch directories.
//!
//! Must not depend on: `app` or winit.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::{Kind, Outcome, Plan, Progress, run};

struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Self {
        let path = std::env::temp_dir().join(format!("osoops-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn plan(kind: Kind, directory: &Path, names: &[&str], target: PathBuf) -> Plan {
    let identity = oso_platform::file_meta::Directory::open(directory)
        .and_then(|listed| listed.identity())
        .unwrap();
    Plan {
        kind,
        directory: directory.to_path_buf(),
        identity,
        names: names.iter().map(OsString::from).collect(),
        target,
        overwrite: None,
    }
}

fn ran(plan: &Plan) -> Outcome {
    let mut each = |_| {};
    run(
        plan,
        &mut Progress {
            stop: &|| false,
            each: &mut each,
        },
    )
}

#[test]
fn a_copy_goes_into_a_directory_and_asks_before_writing_over() {
    let scratch = Scratch::new("copy");
    scratch.file("from/a.txt", "one");
    scratch.file("from/sub/inner.txt", "deep");
    scratch.file("to/a.txt", "old");
    let from = scratch.0.join("from");
    let to = scratch.0.join("to");
    let mut asked = plan(Kind::Copy, &from, &["a.txt", "sub"], to.clone());
    let outcome = ran(&asked);
    assert_eq!(outcome.existing, [to.join("a.txt")]);
    assert_eq!(outcome.done, 0, "nothing written before the question");
    assert_eq!(fs::read_to_string(to.join("a.txt")).unwrap(), "old");
    asked.overwrite = Some(outcome.existing);
    let outcome = ran(&asked);
    assert_eq!(outcome.error, None);
    assert_eq!(fs::read_to_string(to.join("a.txt")).unwrap(), "one");
    assert_eq!(
        fs::read_to_string(to.join("sub/inner.txt")).unwrap(),
        "deep"
    );
    assert_eq!(outcome.touched, [to]);
    assert!(from.join("a.txt").exists(), "a copy leaves the source");
}

#[test]
fn one_name_goes_to_a_new_name() {
    let scratch = Scratch::new("new-name");
    scratch.file("a.txt", "one");
    let outcome = ran(&plan(
        Kind::Copy,
        &scratch.0,
        &["a.txt"],
        scratch.0.join("b.txt"),
    ));
    assert_eq!(outcome.error, None);
    assert_eq!(fs::read_to_string(scratch.0.join("b.txt")).unwrap(), "one");
    // Several names need a directory.
    let outcome = ran(&plan(
        Kind::Copy,
        &scratch.0,
        &["a.txt", "b.txt"],
        scratch.0.join("nowhere"),
    ));
    assert!(outcome.error.is_some());
}

#[test]
fn a_move_renames_and_says_where_each_went() {
    let scratch = Scratch::new("move");
    scratch.file("a.txt", "one");
    fs::create_dir(scratch.0.join("to")).unwrap();
    let outcome = ran(&plan(
        Kind::Rename,
        &scratch.0,
        &["a.txt"],
        scratch.0.join("to"),
    ));
    assert_eq!(outcome.error, None);
    assert!(!scratch.0.join("a.txt").exists());
    assert_eq!(
        outcome.moved,
        [(scratch.0.join("a.txt"), scratch.0.join("to").join("a.txt"))]
    );
}

#[test]
fn a_directory_is_never_written_over_a_file_nor_moved_over_a_directory() {
    let scratch = Scratch::new("kinds");
    fs::create_dir(scratch.0.join("thing")).unwrap();
    scratch.file("to/thing", "a file");
    scratch.file("other/thing/x", "");
    let to = scratch.0.join("to");
    let outcome = ran(&plan(Kind::Copy, &scratch.0, &["thing"], to.clone()));
    assert!(outcome.error.is_some());
    let outcome = ran(&plan(
        Kind::Rename,
        &scratch.0,
        &["thing"],
        scratch.0.join("other"),
    ));
    assert!(outcome.error.is_some());
    assert!(scratch.0.join("thing").exists());
}

#[test]
fn a_delete_takes_a_tree_and_leaves_what_its_links_lead_to() {
    let scratch = Scratch::new("delete");
    scratch.file("tree/a/b.txt", "");
    scratch.file("kept/c.txt", "");
    #[cfg(unix)]
    std::os::unix::fs::symlink(scratch.0.join("kept"), scratch.0.join("tree/link")).unwrap();
    let outcome = ran(&plan(Kind::Delete, &scratch.0, &["tree"], PathBuf::new()));
    assert_eq!(outcome.error, None);
    assert!(!scratch.0.join("tree").exists());
    assert!(scratch.0.join("kept/c.txt").exists());
}

#[test]
fn nothing_is_done_in_a_directory_that_changed_since_it_was_listed() {
    let scratch = Scratch::new("changed");
    let listed = scratch.0.join("listed");
    scratch.file("listed/a.txt", "");
    let asked = plan(Kind::Delete, &listed, &["a.txt"], PathBuf::new());
    fs::rename(&listed, scratch.0.join("gone")).unwrap();
    scratch.file("listed/a.txt", "another");
    let outcome = ran(&asked);
    assert!(outcome.error.unwrap().contains("changed"));
    assert!(listed.join("a.txt").exists());
}

#[test]
fn a_stop_ends_it_after_the_entry_in_hand() {
    let scratch = Scratch::new("stop");
    scratch.file("a", "");
    scratch.file("b", "");
    let mut each = |_| {};
    let outcome = run(
        &plan(Kind::Delete, &scratch.0, &["a", "b"], PathBuf::new()),
        &mut Progress {
            stop: &|| true,
            each: &mut each,
        },
    );
    assert!(outcome.error.is_some());
    assert!(scratch.0.join("a").exists() && scratch.0.join("b").exists());
}

#[test]
fn directories_are_made_with_their_parents() {
    let scratch = Scratch::new("mkdir");
    let target = scratch.0.join("x/y/z");
    let outcome = ran(&plan(Kind::Mkdir, &scratch.0, &[], target.clone()));
    assert_eq!(outcome.error, None);
    assert!(target.is_dir());
    assert_eq!(outcome.done, 3);
    let again = ran(&plan(Kind::Mkdir, &scratch.0, &[], target));
    assert!(again.error.unwrap().contains("there already"));
}
