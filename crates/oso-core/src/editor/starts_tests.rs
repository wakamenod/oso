//! Tests of what an agent's open or save starts and what waits for the person (ADR 0118).
//!
//! Must not depend on: threads, processes or the host.

use std::path::PathBuf;

use super::Editor;
use crate::buffer::{Acted, Baseline, Buffer, BufferId, Start, Starts};
use crate::display_map::TabWidth;
use crate::load::decode_all;
use crate::project::{Found, RootIdentity, TrustList};

const SERVER: Starts = Starts::of(Start::ServerStart);
const SAVED: Starts = Starts::of(Start::ServerSave);

/// A buffer visiting `path`, whose path was found to lead there.
fn visited(path: &str) -> Buffer {
    let mut buffer = Buffer::visiting(PathBuf::from(path));
    buffer.set_baseline(Baseline {
        location: PathBuf::from(path),
        file: None,
    });
    buffer
}

fn found(path: &str, inode: u64) -> Found {
    Found {
        root: RootIdentity {
            path: PathBuf::from(path),
            identity: Some((1, inode)),
        },
        worktree_of: None,
        checked_main: None,
    }
}

/// An editor showing `/w/oso/main.rs`, in the project `/w/oso`, trusted if `trusted`.
fn in_oso(trusted: bool) -> (Editor, BufferId) {
    let mut editor = Editor::with_buffer(visited("/w/oso/main.rs"));
    let id = editor.tiled().0.buffer();
    editor.set_buffer_project(id, Some(found("/w/oso", 7)));
    let record = if trusted { "/w/oso\tdev=1 ino=7\n" } else { "" };
    editor.set_trust_list(Some(TrustList::parse(record).0));
    (editor, id)
}

#[test]
fn the_persons_open_runs_what_would_start() {
    let (mut editor, id) = in_oso(true);
    assert_eq!(editor.starts_on_open(id, SERVER, Acted::Person), SERVER);
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn an_agents_open_runs_no_program_and_marks_the_buffer() {
    let (mut editor, id) = in_oso(true);
    assert_eq!(
        editor.starts_on_open(id, SERVER, Acted::Agent),
        Starts::NONE
    );
    assert_eq!(editor.buffer(id).unwrap().open_waits(), SERVER);
    assert_eq!(editor.buffer(id).unwrap().waits().shown(), ["server"]);
}

#[test]
fn a_restricted_project_holds_nothing() {
    let (mut editor, id) = in_oso(false);
    for by in [Acted::Agent, Acted::Person] {
        assert_eq!(editor.starts_on_open(id, SERVER, by), Starts::NONE);
        assert_eq!(editor.starts_on_save(id, SAVED, by), Starts::NONE);
    }
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn a_buffer_in_no_project_holds_nothing() {
    let mut editor = Editor::with_buffer(visited("/w/loose.rs"));
    let id = editor.tiled().0.buffer();
    assert_eq!(
        editor.starts_on_open(id, SERVER, Acted::Agent),
        Starts::NONE
    );
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn the_persons_command_in_the_buffer_lets_an_open_go() {
    let (mut editor, id) = in_oso(true);
    editor.starts_on_open(id, SERVER, Acted::Agent);
    editor.note_persons_command();
    assert_eq!(editor.take_released_starts(), [(id, SERVER)]);
    assert!(editor.take_released_starts().is_empty(), "taken once");
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn a_command_the_agent_runs_lets_nothing_go() {
    let (mut editor, id) = in_oso(true);
    editor.starts_on_open(id, SERVER, Acted::Agent);
    editor.for_agent(Editor::note_persons_command);
    assert!(editor.take_released_starts().is_empty());
    assert_eq!(editor.buffer(id).unwrap().open_waits(), SERVER);
}

#[test]
fn a_command_in_another_buffer_lets_nothing_go() {
    let (mut editor, id) = in_oso(true);
    editor.starts_on_open(id, SERVER, Acted::Agent);
    let other = editor.add_buffer(Buffer::new("*scratch*"));
    editor.show(other);
    editor.note_persons_command();
    assert!(editor.take_released_starts().is_empty());
    editor.show(id);
    editor.note_persons_command();
    assert_eq!(
        editor.take_released_starts(),
        [(id, SERVER)],
        "switching to it counts"
    );
}

#[test]
fn an_agents_save_waits_for_the_persons_next_save_and_not_for_a_command() {
    let (mut editor, id) = in_oso(true);
    assert_eq!(editor.starts_on_save(id, SAVED, Acted::Agent), Starts::NONE);
    assert_eq!(editor.buffer(id).unwrap().save_waits(), SAVED);
    editor.note_persons_command();
    assert!(editor.take_released_starts().is_empty());
    assert_eq!(editor.buffer(id).unwrap().save_waits(), SAVED);
    assert_eq!(editor.starts_on_save(id, SAVED, Acted::Person), SAVED);
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn what_runs_on_release_is_what_would_start_then() {
    let (mut editor, id) = in_oso(true);
    editor.starts_on_save(id, SAVED, Acted::Agent);
    // The settings changed since: the person's save starts nothing, and the mark goes.
    assert_eq!(
        editor.starts_on_save(id, Starts::NONE, Acted::Person),
        Starts::NONE
    );
    assert!(editor.buffer(id).unwrap().waits().is_empty());
}

#[test]
fn untrusting_the_project_clears_its_marks() {
    let (mut editor, id) = in_oso(true);
    editor.starts_on_open(id, SERVER, Acted::Agent);
    editor.starts_on_save(id, SAVED, Acted::Agent);
    let project = editor.buffer(id).unwrap().project().unwrap();
    editor.untrust_project(project).unwrap();
    assert!(editor.buffer(id).unwrap().waits().is_empty());
    editor.note_persons_command();
    assert!(editor.take_released_starts().is_empty());
}

#[test]
fn trusting_a_project_runs_nothing_an_earlier_agents_open_would_have() {
    let (mut editor, id) = in_oso(false);
    editor.starts_on_open(id, SERVER, Acted::Agent);
    editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=7\n").0));
    editor.note_persons_command();
    assert!(editor.take_released_starts().is_empty());
}

#[test]
fn an_unmodified_buffer_saves_while_a_save_waits_there() {
    let mut buffer = Buffer::visiting(PathBuf::from("/w/oso/main.rs"));
    buffer.apply_load(decode_all(b"fn main() {}\n", TabWidth::default()));
    buffer.set_baseline(Baseline {
        location: PathBuf::from("/w/oso/main.rs"),
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.tiled().0.buffer();
    editor.set_buffer_project(id, Some(found("/w/oso", 7)));
    editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=7\n").0));
    assert_eq!(
        editor.start_save(),
        Err(super::SaveRefused::Unmodified),
        "nothing waits yet"
    );
    editor.starts_on_save(id, SAVED, Acted::Agent);
    assert_eq!(editor.start_save(), Ok(false));
    let job = editor.take_save_jobs().pop().unwrap();
    assert!(
        job.only_if_gone,
        "its text is the file's, so nothing need be written"
    );
}
