//! isearch's worker (ADR 0082): one thread that runs the newest job a search left, and
//! taking in what it found and counted. And the work that goes on a slice a turn beside it:
//! `!`'s replacing and a keyboard macro's replay (ADR 0113), and undoing or redoing a large
//! step (ADR 0119).
//!
//! Must not depend on: key presses or how a frame is drawn. The search and the count run on
//! the worker's thread over a snapshot, never here.
//!
//! - The thread starts with the first job and then waits for the next. A job sent while one
//!   runs supersedes it within a block of text, and jobs that queue meanwhile are dropped
//!   for the newest, on the worker, which also frees their snapshots.
//! - Nothing here logs what a worker reports: its offsets can say something of a buffer
//!   that may hold secrets.

use std::sync::mpsc::{self, Sender};
use std::thread;

use oso_core::editor::{SearchJob, SearchReport};

use super::shell::Loop;
use super::{App, UserEvent, Waker};

/// How long a keyboard macro's replay runs at each turn of the loop (ADR 0113).
const REPLAY_TURN: std::time::Duration = std::time::Duration::from_millis(4);

/// The worker's queue, once its thread has started.
#[derive(Default)]
pub(super) struct Searcher {
    jobs: Option<Sender<SearchJob>>,
}

impl Searcher {
    /// Sends `job` to the worker, starting its thread the first time. Fails, and drops the
    /// job, if the thread cannot start or has gone.
    fn send(&mut self, job: SearchJob, waker: &Waker) -> Result<(), ()> {
        if self.jobs.is_none() {
            let (jobs, queue) = mpsc::channel::<SearchJob>();
            let waker = waker.clone();
            let spawned = thread::Builder::new()
                .name("oso-search".into())
                .spawn(move || {
                    while let Ok(mut job) = queue.recv() {
                        // Only the newest is worth running.
                        while let Ok(newer) = queue.try_recv() {
                            job = newer;
                        }
                        // It fails only once the event loop is gone.
                        job.run(|report| {
                            let _ = waker.send(UserEvent::Search(report));
                        });
                    }
                });
            if spawned.is_err() {
                return Err(());
            }
            self.jobs = Some(jobs);
        }
        match &self.jobs {
            Some(jobs) => jobs.send(job).map_err(drop),
            None => Err(()),
        }
    }
}

impl App {
    /// Keeps isearch true to its window and its text, and sends the job that leaves, if
    /// any, to the worker. Each part the job ends comes back as a [`UserEvent::Search`].
    pub(super) fn start_searches(&mut self) {
        if self.editor.isearch_follow_text() {
            self.request_redraw();
        }
        let Some(job) = self.editor.take_isearch_job() else {
            return;
        };
        if self.searcher.send(job, &self.waker).is_err() {
            // The search waits for `C-g`, and the count stays unknown.
            self.searcher = Searcher::default();
            log::warn!("the search thread could not start");
        }
    }

    /// Replaces `!`'s next slice, if one is under way, and asks for another turn. Once it has
    /// ended, the input held meanwhile runs.
    pub(super) fn replace_slice(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.replace_works() {
            return;
        }
        self.editor.replace_continue();
        self.dispatcher.publish_for_emergency(&self.editor);
        self.start_searches();
        self.request_redraw();
        if !self.editor.replace_works() {
            let effects = self.dispatcher.run_held_during_all(&mut self.editor);
            self.apply(event_loop, effects);
            self.run_held_wheel();
        }
    }

    /// Undoes or redoes the next slice of a large step, if one is under way, and asks for
    /// another turn. Once it has ended, the input held meanwhile runs, and the agent whose
    /// command began it hears how it ended (ADR 0119).
    pub(super) fn undo_slice(&mut self, event_loop: &Loop<'_>) {
        if self.editor.crossing().is_none() {
            // `C-g` may have cancelled it since the last turn.
            self.agent_undo_turn(None);
            self.run_held_wheel();
            return;
        }
        let (effects, ended) = self.dispatcher.go_on_crossing(&mut self.editor);
        self.dispatcher.publish_for_emergency(&self.editor);
        self.apply(event_loop, effects);
        let over = ended.is_some();
        self.agent_undo_turn(ended);
        if over {
            self.run_held_wheel();
        }
    }

    /// Feeds a keyboard macro's replay for a few milliseconds, and asks for another turn while
    /// it runs (ADR 0113).
    pub(super) fn replay_macro(&mut self, event_loop: &Loop<'_>) {
        if !self.editor.macro_replaying() {
            return;
        }
        let until = std::time::Instant::now() + REPLAY_TURN;
        let effects = self.dispatcher.replay(&mut self.editor, until);
        self.apply(event_loop, effects);
        self.request_redraw();
    }

    /// The worker found or counted something.
    pub(super) fn isearch_reported(&mut self, report: SearchReport) {
        if self.editor.isearch_reported(report) {
            self.request_redraw();
        }
        self.start_searches();
    }
}
