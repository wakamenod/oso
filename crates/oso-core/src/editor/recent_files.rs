//! The files a person visited, the most recent first, as Emacs's recentf keeps them, for
//! `C-x C-r` and the files `C-x b` lists after the buffers (ADR 0148).
//!
//! Must not depend on: `command` or `keymap`, or the host's threads, clock or I/O; the host
//! reads the list from disk, stamps what moved and writes it.
//!
//! - A file goes first when the person's command opens it or shows its buffer in the selected
//!   window, and when the person's load or save of it ends. A command the agent channel ran
//!   moves nothing: an agent's open goes first on the person's first command in its buffer,
//!   when ADR 0118 lets it go.
//! - Only a file judged [`Verdict::Ordinary`] that is on disk is kept, by its absolute path as
//!   visited, in UTF-8, with no control character. The list read back from disk is judged by
//!   its names again, so that a file the patterns came to match goes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::Editor;
pub use super::visits::AskedVisit;
use crate::buffer::{Buffer, BufferId, Verdict};

/// The most files kept (ADR 0148).
pub const MAX_RECENT_FILES: usize = 100;

/// The longest path kept, in bytes.
pub const MAX_RECENT_PATH: usize = 4096;

/// The most changes kept for the host to take, so that a host that never takes them holds
/// a bounded list.
const MAX_CHANGES: usize = 2 * MAX_RECENT_FILES;

/// What changed in the list since the host last took it, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentChanges {
    /// Files that went first, in the order they did.
    pub shown: Vec<PathBuf>,
    /// Files dropped, as gone from disk.
    pub forgotten: Vec<PathBuf>,
}

impl RecentChanges {
    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty() && self.forgotten.is_empty()
    }
}

/// The list and what changed in it.
#[derive(Debug, Default)]
pub(super) struct RecentList {
    /// The most recent first, at most [`MAX_RECENT_FILES`], each once.
    files: Vec<PathBuf>,
    changes: RecentChanges,
}

impl RecentList {
    /// Puts `path` first, unless it is first already.
    fn show(&mut self, path: &Path) {
        if self.files.first().is_some_and(|first| first == path) {
            return;
        }
        self.files.retain(|file| file != path);
        self.files.insert(0, path.to_path_buf());
        self.files.truncate(MAX_RECENT_FILES);
        push_bounded(&mut self.changes.shown, path);
    }

    /// Drops `path`.
    fn forget(&mut self, path: &Path) {
        let before = self.files.len();
        self.files.retain(|file| file != path);
        if self.files.len() != before {
            push_bounded(&mut self.changes.forgotten, path);
        }
    }
}

/// Adds `path` to `list`, dropping its oldest once it is full.
fn push_bounded(list: &mut Vec<PathBuf>, path: &Path) {
    if list.len() >= MAX_CHANGES {
        list.remove(0);
    }
    list.push(path.to_path_buf());
}

/// Whether `path` may be kept: absolute, UTF-8, not too long, no control character, and no
/// name on it that may hold secrets.
pub(super) fn may_be_kept(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    path.is_absolute()
        && text.len() <= MAX_RECENT_PATH
        && !text.chars().any(char::is_control)
        && crate::secret::judge_name(path) == Verdict::Ordinary
}

/// The file `buffer` visits, if it may go in the list: loaded, judged ordinary, on disk, the
/// person's to see, and neither internal nor a terminal.
fn recordable(buffer: &Buffer) -> Option<&Path> {
    let path = buffer.path()?;
    let on_disk = buffer.baseline().is_some_and(|b| b.file.is_some());
    let ready = buffer.loading().is_none() && !buffer.agent_pending();
    let kind = !buffer.is_internal() && !buffer.is_terminal();
    let judged = buffer.verdict() == Verdict::Ordinary;
    (on_disk && ready && kind && judged && may_be_kept(path)).then_some(path)
}

impl Editor {
    /// The recent files, the most recent first (ADR 0148).
    pub fn recent_files(&self) -> &[PathBuf] {
        &self.recent_files.files
    }

    /// The person's load or save of `id` ended, or their command left it in the selected
    /// window: its file goes first, if it may be kept. Not for what the agent channel runs.
    pub fn note_recent_visit(&mut self, id: BufferId) {
        if self.runs_for_agent() {
            return;
        }
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        // The usual case while typing: nothing moves, and nothing is judged or copied.
        let first = self.recent_files.files.first().map(PathBuf::as_path);
        if first.is_some_and(|first| buffer.path() == Some(first)) {
            return;
        }
        let Some(path) = recordable(buffer).map(Path::to_path_buf) else {
            return;
        };
        self.recent_files.show(&path);
    }

    /// The files read back from disk, the most recent first: those not in the list yet go
    /// after it, each judged again, up to [`MAX_RECENT_FILES`] in all. Nothing goes first, so
    /// nothing is written for them.
    pub fn remember_recent_files(&mut self, remembered: impl IntoIterator<Item = PathBuf>) {
        let files = &mut self.recent_files.files;
        let mut listed: HashSet<PathBuf> = files.iter().cloned().collect();
        for path in remembered {
            if files.len() >= MAX_RECENT_FILES {
                break;
            }
            if may_be_kept(&path) && listed.insert(path.clone()) {
                files.push(path);
            }
        }
    }

    /// What changed in the list since the last call, for the host to stamp and write.
    pub fn take_recent_changes(&mut self) -> RecentChanges {
        std::mem::take(&mut self.recent_files.changes)
    }

    /// The recent file whose row reads `text`, as [`Editor::recent_file_row`] writes it. It
    /// makes no row to compare, as a preview asks this every frame.
    pub fn recent_file_named(&self, text: &str) -> Option<&Path> {
        let from_home = text.strip_prefix("~/").zip(self.home());
        let wanted = |path: &Path| match from_home {
            Some((rest, home)) => path.strip_prefix(home).is_ok_and(|p| p == Path::new(rest)),
            None => path == Path::new(text) && !self.home().is_some_and(|h| path.starts_with(h)),
        };
        self.recent_files
            .files
            .iter()
            .find(|path| wanted(path))
            .map(PathBuf::as_path)
    }

    /// How a recent file's row reads: its path, from the home directory as `~/`.
    pub fn recent_file_row(&self, path: &Path) -> String {
        crate::complete::path::typed_form(path, false, self.home())
    }

    /// Drops `path`, as a load of it, asked for from the list, found nothing there.
    pub fn forget_recent_file(&mut self, path: &Path) {
        self.recent_files.forget(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Baseline, FileStamp};

    /// An absolute path on this system for `name`.
    fn at(name: &str) -> PathBuf {
        let root = if cfg!(windows) { r"C:\w" } else { "/w" };
        Path::new(root).join(name)
    }

    /// A buffer visiting `path`, loaded, on disk, and judged `verdict`: `None` leaves it
    /// unjudged.
    fn visiting(path: &Path, verdict: Option<Verdict>) -> Buffer {
        let mut buffer = Buffer::visiting(path.to_path_buf());
        buffer.finish_empty();
        match verdict {
            Some(Verdict::Unresolved) => buffer.judge_unresolved(&[]),
            Some(_) => buffer.judge_secret(Some(&[path.to_path_buf()])),
            None => {}
        }
        buffer.set_baseline(Baseline {
            location: path.to_path_buf(),
            file: Some(FileStamp {
                modified: std::time::SystemTime::UNIX_EPOCH,
                len: 0,
                mode: 0,
                identity: None,
            }),
        });
        buffer
    }

    fn ordinary(path: &Path) -> Buffer {
        visiting(path, Some(Verdict::Ordinary))
    }

    #[test]
    fn a_visit_goes_first_once_and_the_list_keeps_a_hundred() {
        let mut editor = Editor::default();
        let a = editor.add_buffer(ordinary(&at("a.md")));
        let b = editor.add_buffer(ordinary(&at("b.md")));
        editor.note_recent_visit(a);
        editor.note_recent_visit(b);
        editor.note_recent_visit(a);
        editor.note_recent_visit(a);
        assert_eq!(editor.recent_files(), [at("a.md"), at("b.md")]);
        let changes = editor.take_recent_changes();
        assert_eq!(changes.shown.len(), 3, "a repeat of the first is no change");
        assert!(editor.take_recent_changes().is_empty());
        for n in 0..150 {
            let id = editor.add_buffer(ordinary(&at(&n.to_string())));
            editor.note_recent_visit(id);
        }
        assert_eq!(editor.recent_files().len(), MAX_RECENT_FILES);
        assert_eq!(editor.recent_files()[0], at("149"));
    }

    #[test]
    fn only_an_ordinary_file_on_disk_goes_in() {
        let mut editor = Editor::default();
        let secret = editor.add_buffer(ordinary(&at(".env")));
        editor.note_recent_visit(secret);
        let unresolved = editor.add_buffer(visiting(&at("u"), Some(Verdict::Unresolved)));
        editor.note_recent_visit(unresolved);
        let unjudged = editor.add_buffer(visiting(&at("j"), None));
        editor.note_recent_visit(unjudged);
        let mut new = ordinary(&at("new"));
        new.set_baseline(Baseline {
            location: at("new"),
            file: None,
        });
        let new = editor.add_buffer(new);
        editor.note_recent_visit(new);
        let none = editor.add_buffer(Buffer::new("notes"));
        editor.note_recent_visit(none);
        let relative = editor.add_buffer(ordinary(Path::new("w/a")));
        editor.note_recent_visit(relative);
        let control = editor.add_buffer(ordinary(&at("a\nb")));
        editor.note_recent_visit(control);
        let mut loading = ordinary(&at("l"));
        loading.set_agent_pending(true);
        let loading = editor.add_buffer(loading);
        editor.note_recent_visit(loading);
        assert!(editor.recent_files().is_empty());
    }

    #[test]
    fn the_agent_moves_nothing() {
        let mut editor = Editor::default();
        let a = editor.add_buffer(ordinary(&at("a.md")));
        editor.for_agent(|editor| editor.note_recent_visit(a));
        assert!(editor.recent_files().is_empty());
    }

    #[test]
    fn what_is_read_back_goes_after_and_is_judged_again() {
        let mut editor = Editor::default();
        let a = editor.add_buffer(ordinary(&at("a.md")));
        editor.note_recent_visit(a);
        let _ = editor.take_recent_changes();
        editor.remember_recent_files([
            at("b.md"),
            at("a.md"),
            at(".ssh").join("config"),
            PathBuf::from("relative"),
            at(".env"),
        ]);
        assert_eq!(editor.recent_files(), [at("a.md"), at("b.md")]);
        assert!(editor.take_recent_changes().is_empty(), "nothing to write");
        editor.remember_recent_files((0..200).map(|n| at(&n.to_string())));
        assert_eq!(editor.recent_files().len(), MAX_RECENT_FILES);
    }

    #[test]
    fn a_file_found_gone_is_dropped() {
        let mut editor = Editor::default();
        editor.remember_recent_files([at("a.md"), at("b.md")]);
        editor.forget_recent_file(&at("a.md"));
        editor.forget_recent_file(&at("c.md"));
        assert_eq!(editor.recent_files(), [at("b.md")]);
        assert_eq!(editor.take_recent_changes().forgotten, [at("a.md")]);
    }

    #[test]
    fn a_row_reads_from_home_and_names_its_file_back() {
        let mut editor = Editor::default();
        editor.set_home(Some(at("me")));
        editor.remember_recent_files([at("me").join("a.md"), at("b")]);
        assert_eq!(editor.recent_file_row(&at("me").join("a.md")), "~/a.md");
        assert_eq!(
            editor.recent_file_named("~/a.md"),
            Some(at("me").join("a.md").as_path())
        );
        let b = editor.recent_file_row(&at("b"));
        assert_eq!(editor.recent_file_named(&b), Some(at("b").as_path()));
        assert_eq!(editor.recent_file_named("~/c"), None);
    }
}
