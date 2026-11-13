//! What the app keeps of hosted agent sessions for ADR 0121 step 3, and the moments it acts
//! on: the screen feed to the sessions worker.
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread.

use std::time::Instant;

use super::feed::{self, Feed, Now};
use crate::app::App;
use crate::sessions::screen::Evidence;

/// Step 3's state of hosted sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Watch {
    feed: Feed,
}

impl App {
    /// Sends each hosted session's screen that changed to the worker, as the feed paces them
    /// (ADR 0121, Q6). Runs after snapshots are merged, and when a change that waited is due.
    pub(in crate::app) fn feed_sessions(&mut self) {
        let Some(worker) = &self.sessions else {
            return;
        };
        let now = Instant::now();
        for (id, terminal) in self.terminals.map.iter() {
            let Some(session) = self.hosting.table().by_buffer(id) else {
                continue;
            };
            let progress = terminal.progress.and_then(feed::progress);
            let screen = Now {
                generation: terminal.grid.generation(),
                title: &terminal.title,
                progress,
                scrolled: terminal.grid.scrolled(),
            };
            if self.watch.feed.look(session.key, screen, now) {
                let rows = terminal.grid.row_texts();
                let evidence =
                    Evidence::new(rows.iter().map(String::as_str), &terminal.title, progress);
                worker.send_screen(session.key, evidence);
            }
        }
    }

    /// When a session's moment comes next: a screen change that waited for the feed's pace.
    pub(in crate::app) fn sessions_deadline(&self) -> Option<Instant> {
        self.watch.feed.due()
    }

    /// Before the event loop waits: does what is due for sessions.
    pub(in crate::app) fn sessions_due(&mut self) {
        if self
            .watch
            .feed
            .due()
            .is_some_and(|due| Instant::now() >= due)
        {
            self.feed_sessions();
        }
    }
}
