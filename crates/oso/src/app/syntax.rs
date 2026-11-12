//! Syntax highlighting's glue (ADR 0111): what the UI tells the worker after each command, and
//! taking in what it reports. The worker's threads are in `pool`; nothing here parses.
//!
//! Must not depend on: key presses or how a frame is drawn; it reads buffers and windows only.
//!
//! - A buffer gets a job only while `Buffer::can_highlight` holds: a language mode, not
//!   internal, at most 2 MiB, and not one that may hold secrets, which includes one not judged
//!   yet. Once it no longer does, or it is killed, the worker drops it at once.
//! - A job carries the splices since the last one, or none to start from nothing, an O(1)
//!   snapshot of the text, and the ranges windows show. While a file loads, only its first
//!   snapshot is sent, and then the loaded text.
//! - A buffer whose highlighting failed stays plain until its mode changes, and the message
//!   names it, quoting nothing of its text or its path.

use std::collections::HashMap;
use std::ops::Range;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::Editor;
use oso_core::window::{Scroll, ViewSize, WindowId};
use oso_syntax::{Failure, Job, Language, Outcome, Report};

use super::App;

mod pool;

use pool::Pool;

/// What the UI keeps of highlighting.
#[derive(Default)]
pub(super) struct Syntax {
    pool: Option<Pool>,
    /// The threads could not start: no buffer is highlighted this session.
    broken: bool,
    /// What each buffer the worker has was sent last.
    sent: HashMap<BufferId, Sent>,
    /// Buffers whose highlighting failed, in the mode it failed in.
    stopped: HashMap<BufferId, Mode>,
    /// What windows show, kept while nothing that decides it moved.
    shown: Vec<(BufferId, Range<usize>)>,
    /// What `shown` was built from: each window's buffer, that buffer's revision, the
    /// window's scroll and size, and whether it wraps.
    views: Vec<View>,
}

/// What decides what a window shows, as far as the next job cares.
type View = (WindowId, BufferId, u64, Scroll, ViewSize, bool);

/// The last job of a buffer's.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    revision: u64,
    mode: Mode,
    shown: Vec<Range<usize>>,
    loading: bool,
}

/// What to tell the worker: buffers to drop first, then jobs.
#[derive(Debug, Default)]
pub(super) struct Orders {
    pub(super) drops: Vec<BufferId>,
    pub(super) jobs: Vec<Job<BufferId>>,
}

impl Orders {
    fn is_empty(&self) -> bool {
        self.drops.is_empty() && self.jobs.is_empty()
    }
}

impl Syntax {
    /// The buffers the worker must drop now: killed, or no longer allowed, as a buffer that
    /// turned secret is. Cheap: it builds no screen.
    pub(super) fn drops(&mut self, editor: &Editor) -> Vec<BufferId> {
        let mut drops = Vec::new();
        self.sent.retain(|id, sent| {
            let keep = editor
                .buffer(*id)
                .is_some_and(|buffer| buffer.mode() == sent.mode && allowed(buffer.mode(), buffer));
            if !keep {
                drops.push(*id);
            }
            keep
        });
        self.stopped.retain(|id, _| editor.buffer(*id).is_some());
        drops
    }

    /// Everything to tell the worker after a command: the drops, and a job for each buffer
    /// whose text or shown ranges moved.
    pub(super) fn orders(&mut self, editor: &Editor) -> Orders {
        let mut orders = Orders {
            drops: self.drops(editor),
            jobs: Vec::new(),
        };
        self.refresh_shown(editor);
        for (id, buffer) in editor.buffers() {
            let mode = buffer.mode();
            match self.stopped.get(&id) {
                Some(stopped) if *stopped == mode => continue,
                Some(_) => {
                    // A new mode gives it another try (ADR 0111).
                    self.stopped.remove(&id);
                }
                None => {}
            }
            let Some(language) = Language::of(mode).filter(|_| allowed(mode, buffer)) else {
                continue;
            };
            let shown: Vec<Range<usize>> = self
                .shown
                .iter()
                .filter(|(shown, _)| *shown == id)
                .map(|(_, range)| range.clone())
                .collect();
            let loading = buffer.loading().is_some();
            let last = self.sent.get(&id);
            let unchanged = last.is_some_and(|last| {
                // While a file loads, the snapshots after the first are skipped.
                (loading && last.loading)
                    || (last.revision == buffer.revision() && last.shown == shown)
            });
            if unchanged {
                continue;
            }
            let edits = last.and_then(|last| buffer.splices_since(last.revision));
            orders.jobs.push(Job {
                buffer: id,
                language,
                revision: buffer.revision(),
                text: buffer.text().clone(),
                edits,
                shown: shown.clone(),
            });
            let sent = Sent {
                revision: buffer.revision(),
                mode,
                shown,
                loading,
            };
            self.sent.insert(id, sent);
        }
        orders
    }

    /// Builds what windows show again only if a window, its buffer's text, its scroll or its
    /// size moved, so a turn of the loop that changed none of them builds no screen. A wrap
    /// that settles rows meanwhile is seen at the next of those: the ranges only order work.
    fn refresh_shown(&mut self, editor: &Editor) {
        let views: Vec<View> = editor
            .windows()
            .map(|window| {
                let revision = editor.shown(window).revision();
                let wraps = window.wrap().is_some();
                (
                    window.id(),
                    window.buffer(),
                    revision,
                    window.scroll(),
                    window.size(),
                    wraps,
                )
            })
            .collect();
        if views == self.views {
            return;
        }
        self.views = views;
        editor.shown_ranges(&mut self.shown);
    }

    /// Takes a report into its buffer, and returns what to say if its highlighting failed. A
    /// report for a buffer the worker was told to drop is ignored.
    pub(super) fn take(&mut self, editor: &mut Editor, report: Report<BufferId>) -> Option<String> {
        self.sent.get(&report.buffer)?;
        let buffer = editor.buffer_mut(report.buffer)?;
        match report.outcome {
            Outcome::Highlights(highlights) => {
                // False for a stale or unwanted report, which changes nothing.
                buffer.take_highlights(report.revision, highlights);
                None
            }
            Outcome::Failed(failure) => {
                self.sent.remove(&report.buffer);
                self.stopped.insert(report.buffer, buffer.mode());
                buffer.drop_highlights();
                Some(stopped(buffer.name(), failure))
            }
        }
    }
}

/// Whether the worker may have `buffer`, in `mode`.
fn allowed(mode: Mode, buffer: &oso_core::buffer::Buffer) -> bool {
    Language::of(mode).is_some() && buffer.can_highlight()
}

/// What the echo area says when a buffer's highlighting stops: its name and why, nothing of
/// its text or its path.
fn stopped(name: &str, failure: Failure) -> String {
    let why = match failure {
        Failure::TooLarge => "it is too large",
        Failure::TooSlow => "it took too long",
        Failure::Incompatible => "this build cannot parse its language",
        Failure::Lost => "the highlighter failed",
    };
    format!("Highlighting stopped in {name}: {why}")
}

impl App {
    /// Tells the worker what changed, starting its threads at the first job. Each report comes
    /// back as a [`super::UserEvent::Syntax`].
    pub(super) fn start_syntax(&mut self) {
        if self.syntax.broken {
            return;
        }
        let orders = self.syntax.orders(&self.editor);
        self.send_syntax(orders);
    }

    /// Drops at once what the worker may no longer have, after any event: a buffer that turned
    /// secret has no slice started on it after this.
    pub(super) fn drop_syntax(&mut self) {
        let drops = self.syntax.drops(&self.editor);
        self.send_syntax(Orders {
            drops,
            jobs: Vec::new(),
        });
    }

    fn send_syntax(&mut self, orders: Orders) {
        if orders.is_empty() {
            return;
        }
        if self.syntax.pool.is_none() {
            if orders.jobs.is_empty() {
                return;
            }
            let waker = self.waker.clone();
            let report = move |report| {
                // False only once the event loop is gone.
                let _ = waker.send(super::UserEvent::Syntax(report));
            };
            match Pool::start(report) {
                Ok(pool) => self.syntax.pool = Some(pool),
                Err(error) => {
                    log::warn!("the syntax threads could not start: {error}");
                    self.editor
                        .message("Highlighting is off: its threads could not start");
                    self.syntax = Syntax {
                        broken: true,
                        ..Syntax::default()
                    };
                    return;
                }
            }
        }
        if let Some(pool) = &self.syntax.pool {
            pool.send(orders);
        }
    }

    /// The worker highlighted, or stopped, a buffer.
    pub(super) fn syntax_reported(&mut self, report: Report<BufferId>) {
        if let Some(message) = self.syntax.take(&mut self.editor, report) {
            self.editor.message(message);
        }
    }
}

#[cfg(test)]
mod tests;
