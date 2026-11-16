//! Tests of the directory each buffer stands in (ADR 0141).
//!
//! Must not depend on: threads, the file system or the host.

use std::path::{Path, PathBuf};

use crate::buffer::{Baseline, Buffer, Mode};
use crate::editor::Editor;
use crate::project::{Found, RootIdentity};

/// The root the tests' paths are under, absolute on this system.
#[cfg(unix)]
const W: &str = "/w";
#[cfg(windows)]
const W: &str = r"C:\w";

/// `name` under [`W`].
fn at(name: &str) -> PathBuf {
    Path::new(W).join(name)
}

/// A buffer visiting `path`, whose path was found to lead there.
fn visited(path: &Path) -> Buffer {
    let mut buffer = Buffer::visiting(path.to_path_buf());
    buffer.set_baseline(Baseline {
        location: path.to_path_buf(),
        file: None,
    });
    buffer
}

fn found(path: &Path) -> Found {
    Found {
        root: RootIdentity {
            path: path.to_path_buf(),
            identity: cfg!(unix).then_some((1, 7)),
        },
        worktree_of: None,
        checked_main: None,
    }
}

/// An editor that started in `W`, showing `*scratch*`.
fn editor() -> Editor {
    let mut editor = Editor::default();
    editor.set_start_directory(PathBuf::from(W));
    editor
}

#[test]
fn scratch_stands_in_the_directory_oso_started_in() {
    let editor = editor();
    let scratch = editor.window().buffer();
    assert_eq!(editor.default_directory(scratch), PathBuf::from(W));
}

#[test]
fn a_file_stands_in_its_directory_and_follows_a_new_name() {
    let mut editor = editor();
    let id = editor.add_buffer(visited(&at("oso/src/main.rs")));
    assert_eq!(editor.default_directory(id), at("oso/src"));
    editor.file_moved(&at("oso/src"), &at("oso/lib"));
    assert_eq!(editor.default_directory(id), at("oso/lib"));
}

#[test]
fn a_listing_stands_in_the_directory_it_lists() {
    let mut editor = editor();
    let id = editor.add_listing(at("notes"));
    assert_eq!(editor.default_directory(id), at("notes"));
}

#[test]
fn a_git_status_and_a_review_stand_in_their_projects_root() {
    let mut editor = editor();
    let file = editor.add_buffer(visited(&at("oso/src/main.rs")));
    editor.set_buffer_project(file, Some(found(&at("oso"))));
    let project = editor.buffer(file).unwrap().project().unwrap();
    let status = editor.add_git_status(project, "*git: oso*");
    assert_eq!(editor.default_directory(status), at("oso"));
    let (_, review) = editor.add_review(project, "*review: oso*".to_owned());
    assert_eq!(editor.default_directory(review), at("oso"));
}

#[test]
fn a_new_buffer_stands_where_the_one_shown_does() {
    let mut editor = editor();
    let file = editor.add_buffer(visited(&at("oso/src/main.rs")));
    editor.show(file);
    let made = editor.add_internal_buffer("*Help*", Mode::Fundamental);
    assert_eq!(editor.default_directory(made), at("oso/src"));
    // It keeps it, as Emacs's buffers keep theirs, whatever is shown after.
    let other = editor.add_listing(at("notes"));
    editor.show(other);
    assert_eq!(editor.default_directory(made), at("oso/src"));
}

#[test]
fn a_new_buffer_beside_scratch_follows_the_start_directory() {
    let mut editor = Editor::default();
    let made = editor.add_internal_buffer("*Help*", Mode::Fundamental);
    editor.set_start_directory(PathBuf::from(W));
    assert_eq!(editor.default_directory(made), PathBuf::from(W));
}

#[test]
fn the_host_gives_a_terminal_its_directory_which_a_move_follows() {
    let mut editor = editor();
    let terminal = editor.add_terminal_buffer("*terminal*");
    editor.set_buffer_directory(terminal, at("oso/crates"));
    assert_eq!(editor.default_directory(terminal), at("oso/crates"));
    editor.file_moved(&at("oso"), &at("osa"));
    assert_eq!(editor.default_directory(terminal), at("osa/crates"));
    editor.file_moved(&at("osa/crates"), &at("elsewhere"));
    assert_eq!(editor.default_directory(terminal), at("elsewhere"));
}

#[test]
fn what_a_terminals_program_says_moves_its_prompts_but_no_buffer_made_from_it() {
    let mut editor = editor();
    let terminal = editor.add_terminal_buffer("*terminal*");
    editor.set_buffer_directory(terminal, at("oso"));
    editor.set_reported_directory(terminal, at("elsewhere"));
    assert_eq!(editor.default_directory(terminal), at("elsewhere"));
    editor.show(terminal);
    // A buffer made from it stands where it started, which a process started from that takes.
    let made = editor.add_internal_buffer("*Help*", Mode::Fundamental);
    assert_eq!(editor.default_directory(made), at("oso"));
    // Only a terminal's program says one.
    let scratch = editor.add_internal_buffer("*notes*", Mode::Fundamental);
    editor.set_reported_directory(scratch, at("elsewhere"));
    assert_eq!(editor.default_directory(scratch), at("oso"));
}

#[test]
fn a_buffer_made_under_a_prompt_stands_where_the_window_beneath_does() {
    let mut editor = editor();
    let file = editor.add_buffer(visited(&at("oso/src/main.rs")));
    editor.show(file);
    editor
        .open_file_prompt("Find file:", "file", "~/", PathBuf::from(W))
        .unwrap();
    let made = editor.add_internal_buffer("*Help*", Mode::Fundamental);
    assert_eq!(editor.default_directory(made), at("oso/src"));
}
