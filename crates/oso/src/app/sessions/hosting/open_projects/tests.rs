//! What the lookups of open projects count, over a real editor: files, terminals and native
//! sessions' transcripts.
//!
//! Must not depend on: `App`; the transcripts' buffers are named as the app's table does.

use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer, BufferId};
use oso_core::editor::Editor;
use oso_core::project::{Found, ProjectId, RootIdentity, TrustList};

use super::{at_root, of_all, of_files_and_terminals, trust_at};
use crate::git::TrustedRoot;

/// An absolute path on this system, spelled as it spells one: `path` as written on Unix, and on
/// drive `C:` on Windows, where a path with no drive is not absolute.
pub(in crate::app::sessions::hosting) fn abs(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:{path}").replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

/// The root at `path`, with an identity where the system records one.
fn root(path: &str) -> RootIdentity {
    RootIdentity {
        path: abs(path),
        identity: cfg!(unix).then_some((1, 7)),
    }
}

pub(in crate::app::sessions::hosting) fn record(path: &str) -> TrustList {
    let identity = if cfg!(unix) { "\tdev=1 ino=7" } else { "" };
    let line = format!("{}{identity}\n", abs(path).display());
    let (list, skipped) = TrustList::parse(&line);
    assert_eq!(skipped, 0, "{line:?}");
    list
}

fn found_in(editor: &mut Editor, id: BufferId, path: &str) -> ProjectId {
    let request = editor.take_project_requests().pop().unwrap();
    assert_eq!(request.buffer, id);
    let found = Found {
        root: root(path),
        worktree_of: None,
        checked_main: None,
    };
    editor.project_found(&request, Some(found));
    editor.buffer(id).unwrap().project().unwrap()
}

/// A native session's transcript whose session runs at `path`, its project's root.
pub(in crate::app::sessions::hosting) fn transcript(editor: &mut Editor, path: &str) -> BufferId {
    let id = editor.add_native_session_buffer("*claude: s*");
    editor.set_hosts_session(id, true);
    editor.request_directory_project(id, &abs(path));
    found_in(editor, id, path);
    id
}

pub(in crate::app::sessions::hosting) fn file(editor: &mut Editor, path: &str) -> ProjectId {
    let place = abs(path).join("a.rs");
    let mut buffer = Buffer::visiting(place.clone());
    buffer.set_baseline(Baseline {
        location: place,
        file: None,
    });
    let id = editor.add_buffer(buffer);
    editor.request_project(id);
    found_in(editor, id, path)
}

fn open(editor: &Editor, transcripts: &[BufferId]) -> Vec<ProjectId> {
    of_all(editor, transcripts.iter().copied()).collect()
}

fn trust(editor: &Editor, transcripts: &[BufferId], path: &str) -> Option<Option<TrustedRoot>> {
    trust_at(editor, open(editor, transcripts).into_iter(), &abs(path))
}

#[test]
fn a_transcripts_project_counts_and_the_editors_buffers_leave_it_out() {
    let mut editor = Editor::default();
    let before = editor.buffers().count();
    let id = transcript(&mut editor, "/w/own");
    assert_eq!(editor.buffers().count(), before);
    assert_eq!(of_files_and_terminals(&editor).count(), 0);
    let project = editor.buffer(id).unwrap().project();
    assert_eq!(open(&editor, &[id]), Vec::from_iter(project));
    assert_eq!(
        at_root(&editor, open(&editor, &[id]).into_iter(), &abs("/w/own")),
        project
    );
    assert_eq!(
        at_root(&editor, open(&editor, &[id]).into_iter(), &abs("/w")),
        None
    );
}

#[test]
fn the_files_come_first() {
    let mut editor = Editor::default();
    let own = transcript(&mut editor, "/w/own");
    let of_file = file(&mut editor, "/w/file");
    let open = open(&editor, &[own]);
    assert_eq!(open.first(), Some(&of_file));
    assert_eq!(open.len(), 2);
}

#[test]
fn a_killed_transcript_and_one_of_no_session_count_no_more() {
    let mut editor = Editor::default();
    let (killed, forgotten) = (
        transcript(&mut editor, "/w/a"),
        transcript(&mut editor, "/w/b"),
    );
    assert_eq!(open(&editor, &[killed, forgotten]).len(), 2);
    editor.kill_buffer(killed);
    editor.set_hosts_session(forgotten, false);
    assert!(open(&editor, &[killed, forgotten]).is_empty());
}

#[test]
fn a_transcript_says_its_projects_trust_as_it_is_now() {
    let mut editor = Editor::default();
    editor.set_trust_list(Some(record("/w/own")));
    let id = transcript(&mut editor, "/w/own");
    assert!(trust(&editor, &[], "/w/own").is_none(), "not a judge's");
    assert!(trust(&editor, &[id], "/w/own").unwrap().is_some());
    editor.set_trust_list(Some(TrustList::default()));
    assert!(trust(&editor, &[id], "/w/own").unwrap().is_none());
}
