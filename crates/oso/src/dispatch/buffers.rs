//! What `C-x b` and `C-x k` complete over: the buffers, most recent first, each with what
//! marginalia says of a buffer (ADR 0088, ADR 0171). `C-x b` lists after them the recent files no buffer
//! visits, as consult-buffer does, and `C-x C-r` lists the recent files (ADR 0148).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::command::BufferPurpose;
use oso_core::complete::Candidate;
use oso_core::editor::Editor;
use oso_core::window::Window;

/// The buffers a prompt read for `purpose` lists, in its order: for showing, as consult-buffer
/// has them (ADR 0171), those no window shows, the most recent first, then those other windows
/// show, then the current buffer, so that `RET` goes back to the one before; for killing, the
/// current first, then those the window showed before, the most recent first, then the rest.
pub(super) fn candidates(editor: &Editor, purpose: BufferPurpose) -> Arc<[Candidate]> {
    let order = match purpose {
        BufferPurpose::Switch => by_visibility(editor),
        BufferPurpose::Kill => by_window(editor),
    };
    // The user's by id, so that a thousand buffers are not each looked for through the list;
    // the few internal ones listed are looked for so.
    let users: HashMap<BufferId, &Buffer> = editor.buffers().collect();
    let buffers = order
        .into_iter()
        .filter_map(|id| users.get(&id).copied().or_else(|| editor.buffer(id)))
        .map(|buffer| Candidate::new(buffer.name(), "", annotation(buffer, editor.home())));
    if purpose == BufferPurpose::Kill {
        return buffers.collect();
    }
    let visited = visited(editor);
    let files = (editor.recent_files().iter())
        .filter(|path| !visited.contains(path.as_path()))
        .map(|path| Candidate::new(editor.recent_file_row(path), "", FILE));
    buffers.chain(files).collect()
}

/// The buffers by recency, as `consult--buffer-sort-visibility` sorts `buffer-list`: those no
/// window shows, then those another window shows, then the current one (ADR 0171).
fn by_visibility(editor: &Editor) -> Vec<BufferId> {
    let current = editor.tiled().0.buffer();
    let shown: HashSet<BufferId> = editor.windows().map(Window::buffer).collect();
    let order = editor.buffers_by_recency();
    let has_current = order.contains(&current);
    let (visible, mut hidden): (Vec<BufferId>, Vec<BufferId>) = order
        .into_iter()
        .filter(|id| *id != current)
        .partition(|id| shown.contains(id));
    hidden.extend(visible);
    hidden.extend(has_current.then_some(current));
    hidden
}

/// The buffers `C-x k` lists: the current one, those its window showed before, the most recent
/// first, then the rest by recency (ADR 0088). The current one is listed whenever `kill-buffer`
/// may kill it, as a review's buffer, which closes the review (ADR 0122), though no buffer list
/// holds it: else `RET` would kill the buffer shown before it.
fn by_window(editor: &Editor) -> Vec<BufferId> {
    let window = editor.tiled().0;
    let listed = editor.buffers_by_recency();
    let mut unseen: HashSet<BufferId> = listed.iter().copied().collect();
    if editor.review_of(window.buffer()).is_some() {
        unseen.insert(window.buffer());
    }
    let shown = std::iter::once(window.buffer()).chain(window.places().map(|(id, _)| id));
    shown
        .chain(listed.iter().copied())
        .filter(|id| unseen.remove(id))
        .collect()
}

/// What a recent file's row in `C-x b` says, in the annotation's face (ADR 0148).
const FILE: &str = "file";

/// What a recent file's row in `C-x C-r` says when a buffer visits it.
const OPEN: &str = "open";

/// The files the buffers visit, but those the agent channel is still opening, which a visit
/// does not take either (`Editor::request_visit`).
fn visited(editor: &Editor) -> HashSet<&Path> {
    (editor.buffers())
        .filter(|(_, buffer)| !buffer.agent_pending())
        .filter_map(|(_, buffer)| buffer.path())
        .collect()
}

/// The recent files `C-x C-r` lists, the most recent first and the current buffer's last, so
/// that `RET` opens the one before; `open` when a buffer visits one (ADR 0148).
pub(super) fn recent_candidates(editor: &Editor) -> Arc<[Candidate]> {
    let current = editor.current().1.path();
    let (others, last): (Vec<&PathBuf>, Vec<&PathBuf>) =
        (editor.recent_files().iter()).partition(|path| Some(path.as_path()) != current);
    let visited = visited(editor);
    others
        .into_iter()
        .chain(last)
        .map(|path| {
            let annotation = if visited.contains(path.as_path()) {
                OPEN
            } else {
                ""
            };
            Candidate::new(editor.recent_file_row(path), "", annotation)
        })
        .collect()
}

/// `*` when modified, the size, the mode, and the file's path with the home directory as `~`.
fn annotation(buffer: &Buffer, home: Option<&Path>) -> String {
    let modified = if buffer.is_modified() { "*" } else { " " };
    let size = match buffer.loading() {
        Some(_) => "loading".to_owned(),
        None => size(buffer.text().len()),
    };
    let mode = buffer.mode().name();
    let path = buffer.path().map(|path| abbreviated(path, home));
    match path {
        Some(path) => format!("{modified} {size:>7}  {mode}  {path}"),
        None => format!("{modified} {size:>7}  {mode}"),
    }
}

/// A size as `ls -h` writes it: `812`, `1.2k`, `34M`.
pub(crate) fn size(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["k", "M", "G", "T"];
    if bytes < 1024 {
        return bytes.to_string();
    }
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 * 1024 && unit + 1 < UNITS.len() {
        value /= 1024;
        unit += 1;
    }
    // Tenths of the unit, shown only below ten.
    let tenths = value * 10 / 1024;
    if tenths < 100 {
        format!("{}.{}{}", tenths / 10, tenths % 10, UNITS[unit])
    } else {
        format!("{}{}", tenths / 10, UNITS[unit])
    }
}

/// `path` with the home directory written as `~`.
pub(super) fn abbreviated(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sizes_read_as_ls_h_writes_them() {
        assert_eq!(size(812), "812");
        assert_eq!(size(1024), "1.0k");
        assert_eq!(size(1300), "1.2k");
        assert_eq!(size(34 * 1024 * 1024), "34M");
        assert_eq!(size(10 * 1024), "10k");
    }

    #[test]
    fn the_home_directory_is_written_as_a_tilde() {
        let home = PathBuf::from("/home/me");
        let at = |path: &str| abbreviated(Path::new(path), Some(&home));
        assert_eq!(at("/home/me/notes/a.md"), "~/notes/a.md");
        assert_eq!(at("/home/me"), "~");
        assert_eq!(at("/home/meadow/a.md"), "/home/meadow/a.md");
        assert_eq!(abbreviated(Path::new("/a"), None), "/a");
    }

    #[test]
    fn showing_lists_the_current_buffer_last_and_killing_first() {
        let mut editor = Editor::default();
        let other = editor.add_buffer(Buffer::from_text("other", "x"));
        editor.show(other);
        let names = |purpose| -> Vec<String> {
            candidates(&editor, purpose)
                .iter()
                .map(|candidate| candidate.text().to_owned())
                .collect()
        };
        assert_eq!(
            names(BufferPurpose::Switch),
            ["*scratch*", "*Messages*", "other"]
        );
        assert_eq!(
            names(BufferPurpose::Kill),
            ["other", "*scratch*", "*Messages*"]
        );
    }

    #[test]
    fn showing_lists_the_hidden_first_then_those_other_windows_show_and_killing_the_window_s() {
        let mut editor = Editor::default();
        let [a, b, _] = ["a", "b", "c"].map(|name| editor.add_buffer(Buffer::from_text(name, "x")));
        editor.show(a);
        editor.show(b);
        let other = editor
            .split_window(oso_core::frame::Way::Down, false)
            .unwrap();
        editor.show_in(other, a);
        let names = |purpose| -> Vec<String> {
            candidates(&editor, purpose)
                .iter()
                .map(|candidate| candidate.text().to_owned())
                .collect()
        };
        // The order is b, a, `*scratch*`, `*Messages*`, c; a shows below, and b is current.
        assert_eq!(
            names(BufferPurpose::Switch),
            ["*scratch*", "*Messages*", "c", "a", "b"]
        );
        assert_eq!(
            names(BufferPurpose::Kill),
            ["b", "a", "*scratch*", "*Messages*", "c"]
        );
    }
    #[test]
    fn killing_lists_a_review_first_when_it_is_current_and_a_kept_buffer_not() {
        use oso_core::buffer::Mode;

        let (mut editor, project) = crate::commands::review::tests::in_project(true);
        let file = editor.tiled().0.buffer();
        let (_, review) = editor.add_review(project, "*review: uncommitted oso*".into());
        editor.show(review);
        let names = |editor: &Editor| -> Vec<String> {
            candidates(editor, BufferPurpose::Kill)
                .iter()
                .map(|candidate| candidate.text().to_owned())
                .collect()
        };
        // `kill-buffer` closes a review: `C-x k RET` in it is that, not a kill of the file
        // shown before it.
        assert_eq!(
            names(&editor),
            ["*review: uncommitted oso*", "a.rs", "*Messages*"]
        );
        // `C-x b` lists it no more than before.
        let shown: Vec<String> = candidates(&editor, BufferPurpose::Switch)
            .iter()
            .map(|candidate| candidate.text().to_owned())
            .collect();
        assert!(
            !shown.iter().any(|name| name.starts_with("*review")),
            "{shown:?}"
        );
        // An internal buffer `kill-buffer` keeps is listed as before.
        editor.show(file);
        let keys = editor.add_internal_buffer("*Review keys*", Mode::Fundamental);
        editor.show(keys);
        assert_eq!(names(&editor), ["a.rs", "*Messages*"]);
    }
}
