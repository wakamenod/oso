//! A terminal's snapshots in the app (ADR 0120): the next frame takes a ready one, a terminal
//! no window shows is looked at a few times a second, and each snapshot's rows, password
//! verdict and events go where they belong.
//!
//! Must not depend on: how a frame is drawn.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_term::Event;

use super::super::App;
use super::{HIDDEN_PACE, reads_a_password};
use crate::terminal::hosted;

impl App {
    /// A terminal's thread has a snapshot ready: the next frame takes it. A terminal no window
    /// shows asks for no frame of its own: it is looked at a few times a second instead,
    /// while its thread goes on parsing.
    pub(in crate::app) fn terminal_ready(&mut self, buffer: BufferId) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer) {
            hosted.ready = true;
            let shown = (self.editor.frame().windows().iter()).any(|(id, _)| {
                self.editor
                    .window_by_id(*id)
                    .is_some_and(|w| w.buffer() == buffer)
            });
            if shown {
                self.request_redraw();
            } else {
                // Its end, or its title, is taken within a moment though no frame comes.
                self.terminals
                    .hidden_due
                    .get_or_insert_with(|| Instant::now() + HIDDEN_PACE);
            }
        }
    }

    /// Before the event loop waits: once the moment for terminals no window shows has come,
    /// takes what they sent, at most a few times a second (ADR 0120).
    pub(in crate::app) fn take_hidden_snapshots(&mut self) {
        if self
            .terminals
            .hidden_due
            .is_some_and(|due| Instant::now() >= due)
        {
            self.terminals.hidden_due = None;
            if self.take_terminal_snapshots() {
                self.terminals.hidden_due = Some(Instant::now() + HIDDEN_PACE);
            }
            // A session that ended out of sight is remembered before the loop waits.
            self.save_sessions();
        }
    }

    /// When the event loop wakes for terminals no window shows, if it does.
    pub(in crate::app) fn hidden_terminals_deadline(&self) -> Option<Instant> {
        self.terminals.hidden_due
    }

    /// Before a frame: takes each ready terminal's snapshot, one per terminal, merges its rows
    /// into what the UI keeps and gives its buffers back. Returns whether it took any, as
    /// another may wait already.
    pub(in crate::app) fn take_terminal_snapshots(&mut self) -> bool {
        let mut ended = Vec::new();
        let mut said = Vec::new();
        let mut secret = Vec::new();
        let mut reported = Vec::new();
        let mut again = false;
        for (id, hosted) in self.terminals.map.iter_mut() {
            if !hosted.ready {
                continue;
            }
            let Some(terminal) = &hosted.terminal else {
                hosted.ready = false;
                continue;
            };
            let Some(mut snapshot) = terminal.take_snapshot() else {
                hosted.ready = false;
                continue;
            };
            // Another may wait already, as the end does behind one in flight: the next frame
            // looks again, so that this one takes one only.
            again = true;
            hosted.grid.apply(&snapshot);
            // The buffer's verdict is the one record of a password prompt. A program that
            // ended reads none, whatever its last snapshot saw.
            let ends = snapshot
                .events
                .iter()
                .any(|event| matches!(event, Event::Ended(_)));
            let echo_off = snapshot.echo_off && !ends;
            reported.push((id, echo_off));
            if echo_off != self.editor.buffer(id).is_some_and(reads_a_password) {
                hosted.note_stale = true;
                secret.push((id, echo_off));
            }
            for event in snapshot.events.drain(..) {
                match event {
                    Event::Title(title) => {
                        hosted.title = title;
                        hosted.note_stale = true;
                    }
                    Event::Ended(end) => {
                        hosted.ended = Some(hosted::ended(&end));
                        hosted.note_stale = true;
                        ended.push((id, end));
                    }
                    Event::PasteRefused { lines } => said.push(format!(
                        "{lines} lines not pasted: the program turned bracketed paste off"
                    )),
                    Event::Progress(progress) => hosted.progress = Some(progress),
                    // Step 6 and after (ADR 0120).
                    Event::Bell
                    | Event::Directory(_)
                    | Event::Notification { .. }
                    | Event::Prompt(_)
                    | Event::ClipboardWrite(_) => {}
                }
            }
            if hosted.ended.is_some() {
                // Nothing follows the end; its handle goes.
                hosted.terminal = None;
            } else {
                terminal.recycle(snapshot);
            }
        }
        for (id, end) in ended {
            self.editor.set_running(id, false);
            // A hosted session's end, which its modeline says differently (ADR 0121).
            self.session_terminal_ended(id, &end);
        }
        // A password prompt is a secret from the snapshot on: secure input, the log and the
        // input method follow at once, between events (ADR 0120, Q17).
        for (id, echo_off) in &secret {
            self.editor.set_echo_off(*id, *echo_off);
        }
        for (id, echo_off) in reported {
            self.dispatcher
                .terminal_reported(&mut self.editor, id, echo_off);
        }
        if !secret.is_empty() {
            self.secret_changed();
        }
        for text in said {
            self.editor.message(text);
        }
        // The frame being drawn shows the notes; a program may have asked for every motion.
        self.note_terminals();
        self.want_pointer_motion();
        if again {
            self.feed_sessions();
        }
        again
    }
}
