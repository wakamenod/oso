//! Jumping to the source: which scopes open the work tree's file, the names of side buffers,
//! and the window the source goes to.
//!
//! Must not depend on: the app's threads, git or files; these work on hunks and an editor made
//! here.

use oso_core::buffer::Mode;
use oso_core::frame::Way;
use oso_core::review::Side;

use super::{
    SideBuffer, now_said, secret_buffer, select_source_window, side_mode, side_name, side_shown,
};
use crate::git::Id;
use crate::review::Scope;

#[test]
fn only_a_review_of_the_work_tree_or_the_index_opens_the_file() {
    let id = Id::parse(b"0123456789abcdef0123456789abcdef01234567").unwrap();
    for scope in [
        Scope::Session { baseline: id },
        Scope::Uncommitted,
        Scope::Staged,
        Scope::Unstaged,
        Scope::Branch {
            base: "main".into(),
        },
    ] {
        assert!(scope.new_side_on_disk(false), "{scope:?}");
    }
    let commit = Scope::Commit {
        revision: "HEAD".into(),
    };
    let range = Scope::Range {
        from: "a".into(),
        to: "b".into(),
    };
    assert!(!commit.new_side_on_disk(true));
    assert!(!range.new_side_on_disk(true));
    let pr = Scope::PullRequest { number: 7 };
    assert!(pr.new_side_on_disk(true), "checked out here");
    assert!(!pr.new_side_on_disk(false), "its head commit");
}

#[test]
fn a_side_s_name_is_one_line_and_says_its_side_and_scope() {
    let name = side_name(b"src/a\nb.rs", Side::Old, &Scope::Uncommitted);
    assert_eq!(name, "*old: src/a\\nb.rs \u{b7} uncommitted*");
    // A bidirectional mark is escaped, so the name cannot reorder what follows it.
    let name = side_name("a\u{202e}sr.b".as_bytes(), Side::New, &Scope::Uncommitted);
    assert!(!name.contains('\u{202e}'), "{name}");
    let long = vec![b'x'; 200];
    let name = side_name(&long, Side::New, &Scope::Staged);
    assert!(
        name.starts_with("*new: xxx") && name.chars().count() < 110,
        "{name}"
    );
}

#[test]
fn the_source_goes_to_another_window_than_the_review_s_or_one_split_off() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let file = editor.tiled().0.buffer();
    let (review, buffer) = editor.add_review(project, "*review: uncommitted oso*".into());
    let first = editor.tiled().0.id();
    editor.show_in(first, buffer);
    // The review alone: a window is split off below and selected, the review staying.
    select_source_window(&mut editor, review).unwrap();
    let second = editor.tiled().0.id();
    assert_ne!(second, first);
    editor.select_window(first);
    assert_eq!(editor.tiled().0.buffer(), buffer);
    // The new window shows the file now: it is taken, with no split.
    editor.show_in(second, file);
    select_source_window(&mut editor, review).unwrap();
    assert_eq!(editor.tiled().0.id(), second);
    // A window showing the same review is passed over.
    editor.select_window(second);
    editor.show_in(second, buffer);
    editor.select_window(first);
    let third = editor.split_window(Way::Down, false).unwrap();
    editor.select_window(third);
    editor.show_in(third, file);
    editor.select_window(first);
    select_source_window(&mut editor, review).unwrap();
    assert_eq!(editor.tiled().0.id(), third);
}

#[test]
fn a_side_is_found_again_by_its_path_and_side_not_its_cut_name() {
    let long = |end: &str| format!("{}/{end}", "d".repeat(100)).into_bytes();
    let (rust, toml) = (long("a.rs"), long("b.toml"));
    let name = |path: &[u8]| side_name(path, Side::New, &Scope::Staged);
    assert_eq!(name(&rust), name(&toml), "both names are cut");
    let mut editor = oso_core::editor::Editor::default();
    let id = editor.add_side_buffer(name(&rust), Mode::Rust);
    let sides = [SideBuffer {
        id,
        path: rust.clone().into(),
        side: Side::New,
    }];
    assert_eq!(side_shown(&sides, &rust, Side::New), Some(id));
    assert_eq!(side_shown(&sides, &rust, Side::Old), None);
    assert_eq!(side_shown(&sides, &toml, Side::New), None);
}

#[test]
fn a_side_takes_its_own_path_s_mode() {
    let mode = |old: Option<&[u8]>, side| side_mode(b"src/a.rs", old, side);
    assert_eq!(mode(None, Side::New), Mode::Rust);
    assert_eq!(mode(None, Side::Old), Mode::Rust);
    assert_eq!(mode(Some(b"src/a.toml"), Side::Old), Mode::Toml);
    assert_eq!(mode(Some(b"src/a.toml"), Side::New), Mode::Rust);
}

#[test]
fn what_ret_says_of_the_file_now() {
    assert_eq!(
        now_said(None),
        "The file as checked out now \u{b7} M-RET shows it as the review read it"
    );
    assert_eq!(
        now_said(Some(12)),
        "#12's head is not checked out here: this checkout's file \u{b7} M-RET shows #12's"
    );
}

#[test]
fn a_secret_open_buffer_of_either_name_of_a_file_is_found() {
    let mut editor = oso_core::editor::Editor::default();
    let mut open = |path: &str, judged: &str| {
        let mut buffer = oso_core::buffer::Buffer::visiting(path.into());
        buffer.judge_secret(Some(&[judged.into()]));
        editor.add_buffer(buffer);
    };
    open("/p/a.rs", "/p/a.rs");
    open("/p/old.rs", "/p/.ssh/id_rsa");
    let secret = |places: &[&str]| {
        let places: Vec<_> = places.iter().map(std::path::PathBuf::from).collect();
        secret_buffer(&editor, &places)
    };
    assert!(!secret(&["/p/a.rs"]));
    assert!(secret(&["/p/old.rs", "/p/a.rs"]));
    assert!(!secret(&[]));
}
