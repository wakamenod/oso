//! The diagnostics each language server reports, taken into the buffers it follows (ADR 0138,
//! Diagnostics, step 6), on the UI thread: a set the server's thread placed and mapped to a
//! revision is mapped through the edits since by the buffer, O(entries), and swapped in. A
//! document kept from its server loses its set, which no longer follows what the server has.
//! Once the cursor stays 300 ms on a diagnostic, the echo area says it (Q14). The reports of
//! files no buffer holds are kept by path, shared with the server's thread, for `M-g d`, whose
//! rows are built on a thread of their own from a snapshot (step 6b, `rows`).
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.

mod rows;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::diagnostic::Diagnostics;
use oso_core::editor::{DiagnosticRows, Editor};
use oso_core::window::WindowId;
use oso_lsp::position::Encoding;
use oso_lsp::protocol::Published;
use oso_lsp::server::Diagnosed;

use super::super::{App, Freer, UserEvent};
use super::documents::Open;

/// The reports of a server's files that no document open with it holds, by path, each with the
/// encoding of its positions: the reports its thread keeps, shared, so they cost no more here,
/// and the paths shared too, so a snapshot copies none.
pub(super) type Files = HashMap<Arc<Path>, (Arc<Published>, Encoding)>;

/// Takes `diagnosed` into the buffers of `documents` whose document is open with the server,
/// and the reports of other files into `files`. What a buffer or `files` had goes to `freer`:
/// its strings can take 16 MiB, which the UI thread does not free. Returns whether a buffer
/// took a set, for a frame.
pub(super) fn take(
    editor: &mut Editor,
    (documents, files): (&HashMap<BufferId, Open>, &mut Files),
    diagnosed: Vec<Diagnosed>,
    freer: &Freer,
) -> bool {
    let mut taken = false;
    // What goes, freed in one go off this thread.
    let mut old_files = Vec::new();
    let mut old_sets = Vec::new();
    for diagnosed in diagnosed {
        let (id, revision, diagnostics) = match diagnosed {
            Diagnosed::Open {
                id,
                revision,
                diagnostics,
            } => (id, revision, diagnostics),
            Diagnosed::File {
                path,
                report,
                encoding,
            } => {
                let path: Arc<Path> = Arc::from(path);
                let old = if report.diagnostics.is_empty() {
                    files.remove(&path)
                } else {
                    files.insert(path, (report, encoding))
                };
                old_files.extend(old);
                continue;
            }
            Diagnosed::Forgotten => {
                old_files.extend(std::mem::take(files).into_values());
                continue;
            }
        };
        let Some((&buffer, _)) = documents
            .iter()
            .find(|(buffer, open)| buffer.get() == id && open.closed.is_none())
        else {
            continue;
        };
        if let Some(buffer) = editor.buffer_mut(buffer) {
            // A clone shares the strings, so the old set's last owner is the freeing thread's.
            let old = buffer.diagnostics().cloned();
            taken |= buffer.take_diagnostics(revision, diagnostics);
            old_sets.extend(old);
        }
    }
    if !old_files.is_empty() {
        freer.free(old_files);
    }
    if !old_sets.is_empty() {
        freer.free(old_sets);
    }
    taken
}

/// Empties the set of `id`, whose document was kept from its server: what it had no longer
/// follows the text the server has.
pub(super) fn clear(editor: &mut Editor, id: BufferId) -> bool {
    let Some(buffer) = editor.buffer_mut(id) else {
        return false;
    };
    let Some(server) = buffer.diagnostics().map(|set| set.server().to_owned()) else {
        return false;
    };
    let empty = Diagnostics::new(&server, [], buffer.text().len());
    let revision = buffer.revision();
    buffer.take_diagnostics(revision, empty)
}

impl App {
    /// Starts building the rows `M-g d`'s list asked for, if it asked, on a thread of its own,
    /// from the open buffers' sets and the files' reports of the servers as they are now. The
    /// rows come back as [`UserEvent::DiagnosticsListed`]. If the thread cannot start, an
    /// empty list comes at once.
    pub(in crate::app) fn start_diagnostics_list(&mut self) {
        let Some(request) = self.editor.take_diagnostics_request() else {
            return;
        };
        let open = (self.editor.buffers())
            .filter_map(|(buffer, held)| {
                let set = held.diagnostics()?;
                Some(rows::OpenSet {
                    buffer,
                    path: held.path()?.to_path_buf(),
                    text: held.text().clone(),
                    revision: held.revision(),
                    set: set.clone(),
                })
            })
            .collect();
        // Only the servers of this project, or of one inside it or around it; the paths are
        // judged on the thread, against the root as a server's URI reads back.
        let root = &request.root;
        let files = (self.servers.running.iter())
            .filter(|running| running.root.starts_with(root) || root.starts_with(&running.root))
            .flat_map(|running| running.files.iter())
            .map(|(path, (report, encoding))| (Arc::clone(path), Arc::clone(report), *encoding))
            .collect();
        let snapshot = rows::Snapshot {
            generation: request.generation,
            root: request.root,
            current: request.buffer,
            cursor: request.cursor,
            open,
            files,
        };
        // A newer list stops the build of an older one.
        let live = Arc::clone(&self.servers.listing);
        live.store(request.generation, Ordering::Relaxed);
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("oso-diagnostics".into())
            .spawn(move || {
                let built = rows::build(&snapshot, &|| {
                    live.load(Ordering::Relaxed) == snapshot.generation
                });
                // The snapshot's sets and reports are freed here too.
                drop(snapshot);
                if let Some(built) = built {
                    waker.send(UserEvent::DiagnosticsListed(built));
                }
            });
        if spawned.is_err() {
            log::warn!("the thread that lists diagnostics could not start");
            let empty = DiagnosticRows::new(request.generation, Vec::new(), None, false);
            self.editor.diagnostics_listed(empty);
        }
    }

    /// The rows of `M-g d`'s list came: the editor shows them, if they are for its list.
    pub(in crate::app) fn diagnostics_listed(&mut self, rows: DiagnosticRows) {
        self.editor.diagnostics_listed(rows);
        self.request_redraw();
    }
}

/// How long the cursor stays on a diagnostic before the echo area says it (ADR 0138, Q14), as
/// ElDoc's idle delay.
const AT_POINT_DELAY: Duration = Duration::from_millis(300);

/// Where the selected window's primary cursor is, as the message at point follows it: the
/// window, the buffer, its revision and the offset.
type Place = (WindowId, BufferId, u64, usize);

/// When the echo area says the diagnostic at point (Q14). Told the time, so it is tested with no
/// clock.
#[derive(Debug, Default)]
pub(super) struct AtPoint {
    /// The place last seen on a diagnostic, and when its message is due there.
    place: Option<Place>,
    due: Option<Instant>,
    /// What it last put in the echo area, which its next message may replace.
    said: String,
}

impl AtPoint {
    /// The cursor is at `place`, on a diagnostic or not (`None`), at `now`: a new place on one
    /// starts the time again.
    fn seen(&mut self, place: Option<Place>, now: Instant) {
        if place != self.place {
            self.place = place;
            self.due = place.map(|_| now + AT_POINT_DELAY);
        }
    }

    /// Whether the message is due at `now`, which it then forgets.
    fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if now >= due => {
                self.due = None;
                true
            }
            _ => false,
        }
    }

    /// When the loop must wake for it.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.due
    }
}

/// Where the cursor of `editor` is, if a diagnostic covers it.
fn place(editor: &Editor) -> Option<Place> {
    let (window, buffer) = editor.current();
    let offset = window.selections.primary().cursor;
    let id = window.buffer();
    buffer
        .diagnostics()?
        .at(offset)
        .next()
        .map(|_| (window.id(), id, buffer.revision(), offset))
}

impl App {
    /// Before the loop waits: once the cursor has stayed 300 ms on a diagnostic, the echo area
    /// says it, unless it says something else (Q14). Never logged, as ElDoc's are not (step 6's
    /// Q7).
    pub(in crate::app) fn diagnostic_at_point_if_due(&mut self) {
        let now = Instant::now();
        let at_point = &mut self.servers.at_point;
        at_point.seen(place(&self.editor), now);
        if !at_point.take_due(now) || self.editor.macro_replaying() {
            return;
        }
        let Some(text) = self.editor.diagnostic_at_point() else {
            return;
        };
        let before = std::mem::replace(&mut at_point.said, text.clone());
        self.editor.echo_unless_news(&before, text);
        self.request_redraw();
    }

    /// When the message at point is due, for the loop's wake.
    pub(in crate::app) fn diagnostic_at_point_deadline(&self) -> Option<Instant> {
        self.servers.at_point.next_wake()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Option<Place> = None;

    #[test]
    fn the_message_is_due_once_the_cursor_stays_on_a_place() {
        let start = Instant::now();
        let mut at_point = AtPoint::default();
        let here = || {
            let editor = Editor::default();
            (editor.window().id(), editor.window().buffer(), 0, 4)
        };
        at_point.seen(Some(here()), start);
        assert_eq!(at_point.next_wake(), Some(start + AT_POINT_DELAY));
        assert!(!at_point.take_due(start + AT_POINT_DELAY / 2));
        // The same place later keeps the time.
        at_point.seen(Some(here()), start + AT_POINT_DELAY / 2);
        assert!(at_point.take_due(start + AT_POINT_DELAY));
        assert!(!at_point.take_due(start + AT_POINT_DELAY * 2), "once");
        // Off a diagnostic, nothing is due; back on, the time starts again.
        at_point.seen(WINDOW, start + AT_POINT_DELAY * 3);
        assert_eq!(at_point.next_wake(), None);
        let (window, buffer, revision, _) = here();
        let later = start + AT_POINT_DELAY * 4;
        at_point.seen(Some((window, buffer, revision, 5)), later);
        assert_eq!(at_point.next_wake(), Some(later + AT_POINT_DELAY));
    }
}
