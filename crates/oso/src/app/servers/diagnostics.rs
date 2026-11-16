//! The diagnostics each language server reports, taken into the buffers it follows (ADR 0138,
//! Diagnostics, step 6), on the UI thread: a set the server's thread placed and mapped to a
//! revision is mapped through the edits since by the buffer, O(entries), and swapped in. A
//! document kept from its server loses its set, which no longer follows what the server has.
//! Once the cursor stays 300 ms on a diagnostic, the echo area says it (Q14).
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a server.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::diagnostic::Diagnostics;
use oso_core::editor::Editor;
use oso_core::window::WindowId;
use oso_lsp::server::Diagnosed;

use super::super::{App, Freer};
use super::documents::Open;

/// Takes `diagnosed` into the buffers of `documents` whose document is open with the server.
/// The set a buffer had goes to `freer`: its strings can take 16 MiB, which the UI thread does
/// not free. Returns whether a buffer took a set, for a frame.
pub(super) fn take(
    editor: &mut Editor,
    documents: &HashMap<BufferId, Open>,
    diagnosed: Vec<Diagnosed>,
    freer: &Freer,
) -> bool {
    let mut taken = false;
    for Diagnosed::Open {
        id,
        revision,
        diagnostics,
    } in diagnosed
    {
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
            freer.free(old);
        }
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
