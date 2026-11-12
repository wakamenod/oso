//! Tests of a walk deeper than the directories it holds open: those let go are opened again,
//! only as the directory they were.
//!
//! Must not depend on: `app`, winit or the GPU.

#[cfg(unix)]
use std::fs;
use std::sync::Arc;

use super::super::{Limits, Spent, tree};
use super::Scratch;

/// What the walk spends, from nothing.
fn nothing_spent() -> Spent {
    Spent {
        limits: Limits::PROJECT,
        entries: 0,
        names: 0,
        ignore_bytes: 0,
    }
}

/// The stack of a walk gone `depth` directories down `scratch`'s `a0/a1/…`, each opened from
/// the one before.
fn deep_stack(scratch: &Scratch, depth: usize, spent: &mut Spent) -> tree::Stack {
    let root = Arc::new(oso_platform::file_meta::Directory::open(&scratch.0).unwrap());
    let project = tree::Project::at(&scratch.0, &root, spent);
    let mut stack = tree::Stack::new();
    let listed = tree::Listed::open(root, scratch.0.clone(), &project, spent, &|| false);
    stack.push(listed.unwrap().unwrap());
    let mut path = scratch.0.clone();
    for level in 0..depth {
        let name = std::ffi::OsString::from(format!("a{level}"));
        path.push(&name);
        let opened = stack.open_top(spent).unwrap().unwrap();
        let below = Arc::new(opened.open_directory(&name).unwrap().unwrap());
        let listed =
            tree::Listed::below(below, path.clone(), name, &project, spent, &|| false).unwrap();
        stack.push(listed.unwrap());
    }
    stack
}

#[test]
fn a_deep_walk_holds_few_directories_open_and_opens_one_let_go_again() {
    let scratch = Scratch::new("deep");
    let depth = tree::MAX_HELD + 8;
    let mut path = String::new();
    for level in 0..depth {
        path.push_str(&format!("a{level}/"));
        scratch.file(&format!("{path}f{level}"), "");
    }
    let mut spent = nothing_spent();
    let mut stack = deep_stack(&scratch, depth, &mut spent);
    assert_eq!(stack.held(), tree::MAX_HELD, "the root and the deepest");
    // Back up to one let go: it opens again, as the directory it was, each name an entry.
    for _ in 0..depth - 3 {
        stack.pop();
    }
    let before = spent.entries;
    let again = stack
        .open_top(&mut spent)
        .unwrap()
        .expect("the same directory");
    assert_eq!(spent.entries - before, 3, "from the root, three names down");
    let names: Vec<_> = again.entries().unwrap().map(|e| e.unwrap().name).collect();
    assert!(names.contains(&std::ffi::OsString::from("f2")));
    assert!(stack.held() <= tree::MAX_HELD);
    // The whole walk lists every level's file.
    let listed = scratch.listed();
    assert_eq!(listed.len(), depth);
    assert!(
        listed
            .iter()
            .any(|path| path.ends_with(&format!("f{}", depth - 1)))
    );
}

#[cfg(unix)]
#[test]
fn a_directory_let_go_and_replaced_since_is_not_walked_again() {
    use std::os::unix::fs::symlink;
    let scratch = Scratch::new("deep-replaced");
    let outside = Scratch::new("deep-replaced-outside");
    outside.file("elsewhere", "");
    let depth = tree::MAX_HELD + 8;
    let mut path = String::new();
    for level in 0..depth {
        path.push_str(&format!("a{level}/"));
        scratch.file(&format!("{path}f{level}"), "");
    }
    let mut spent = nothing_spent();
    let mut stack = deep_stack(&scratch, depth, &mut spent);
    for _ in 0..depth - 3 {
        stack.pop();
    }
    // `a0/a1/a2` moves away and another directory of its name comes.
    let a2 = scratch.0.join("a0/a1/a2");
    fs::rename(&a2, scratch.0.join("a0/a1/moved")).unwrap();
    scratch.file("a0/a1/a2/f2", "");
    assert!(
        stack.open_top(&mut spent).unwrap().is_none(),
        "another inode"
    );
    assert!(
        stack.last_mut().unwrap().entries.is_empty(),
        "its entries are left"
    );
    // A link to a directory outside, under the same name, is not followed either.
    fs::remove_dir_all(&a2).unwrap();
    fs::rename(scratch.0.join("a0/a1/moved"), &a2).unwrap();
    let mut stack = deep_stack(&scratch, depth, &mut spent);
    for _ in 0..depth - 3 {
        stack.pop();
    }
    fs::rename(&a2, scratch.0.join("a0/a1/moved")).unwrap();
    symlink(&outside.0, &a2).unwrap();
    assert!(stack.open_top(&mut spent).unwrap().is_none(), "a link");
}

// Windows makes no path this long without its long paths turned on.
#[cfg(unix)]
#[test]
fn a_walk_deeper_than_its_bound_stops_and_says_it_was_cut() {
    let scratch = Scratch::new("too-deep");
    let deep = "d/".repeat(super::super::MAX_DEPTH + 2);
    scratch.file(&format!("{deep}f"), "");
    scratch.file("top", "");
    let (listed, cut) = scratch.walked(Limits::PROJECT);
    assert!(cut);
    assert!(!listed.iter().any(|path| path.ends_with("/f")));
}
