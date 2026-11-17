//! Syntax highlighting's glue (ADR 0111): what the UI tells the worker after each command, and
//! taking in what it reports. The worker's threads are in `pool`; nothing here parses.
//!
//! Must not depend on: key presses or how a frame is drawn; it reads buffers and windows only.
//!
//! - A buffer gets a job only while `Buffer::can_highlight` holds: a language mode, not
//!   internal but for a server's description at point while it shows (ADR 0138), at most
//!   2 MiB, and not one that may hold secrets, which includes one not judged yet. Once it no
//!   longer does, or it is killed, the worker drops it at once.
//! - A job carries the splices since the last one, or none to start from nothing, an O(1)
//!   snapshot of the text, and the ranges windows show. While a file loads, only its first
//!   snapshot is sent, and then the loaded text.
//! - A buffer whose highlighting failed stays plain until a mode is set in it, the same one too,
//!   and the message names it, quoting nothing of its text or its path.
//! - With `global-font-lock-mode` off, no buffer is allowed, so every one is dropped.
//! - A report asks for a frame only if a window shows runs it changed.
//! - The threads know a text by [`SyntaxKey`]: a buffer, or a review's side, whose report goes
//!   to the review's view thread untouched (ADR 0122, step 9).

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::editor::{Editor, Preview, ReviewId};
use oso_core::syntax::Highlights;
use oso_core::window::{Scroll, ViewSize, Window, WindowId};
use oso_syntax::{Failure, Job, Language, Outcome, Report};

use super::App;

mod pool;

pub(in crate::app) use pool::Documents;
use pool::Pool;

/// What the syntax threads know a text by: a buffer, or a side of a review that its view thread
/// opened under a number of its own (ADR 0122, step 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SyntaxKey {
    Buffer(BufferId),
    Side(ReviewId, u64),
}

/// What the UI keeps of highlighting.
#[derive(Default)]
pub(super) struct Syntax {
    pool: Option<Pool<SyntaxKey>>,
    /// The threads could not start: no buffer is highlighted this session.
    broken: bool,
    /// What each buffer the worker has was sent last.
    sent: HashMap<BufferId, Sent>,
    /// Buffers whose highlighting failed, with the mode revision it failed in and when:
    /// setting a mode, the same one too, moves it on and gives the buffer another try, at most
    /// once every [`RETRY_EVERY`].
    stopped: HashMap<BufferId, (u64, Instant)>,
    /// The last epoch given out: each new start of a buffer takes the next.
    epochs: u64,
    /// What windows show, kept while nothing that decides it moved.
    shown: Vec<(BufferId, Range<usize>)>,
    /// What windows show now, read at each report: a wrap that settled rows since `shown`
    /// was built moves it, and whether a report draws a frame depends on it.
    drawn: Vec<(BufferId, Range<usize>)>,
    /// What `shown` was built from: each window's buffer, that buffer's revision, the
    /// window's scroll and size, and whether it wraps.
    views: Vec<View>,
    /// Each buffer's mode and whether the worker may have it, reused between calls.
    present: HashMap<BufferId, (Mode, bool)>,
    /// The description shown at point when `shown` was built, and its revision.
    described: Option<(BufferId, u64)>,
}

/// The buffers the worker may have: every buffer of the user's, and a server's description
/// at point while it shows, the one internal buffer highlighted (ADR 0138).
fn highlightable(editor: &Editor) -> impl Iterator<Item = (BufferId, &Buffer)> {
    let described = editor
        .hover()
        .and_then(|shown| Some((shown.buffer(), editor.buffer(shown.buffer())?)));
    editor.buffers().chain(described)
}

/// The description shown at point and its revision, as `shown` follows it.
fn described(editor: &Editor) -> Option<(BufferId, u64)> {
    let id = editor.hover()?.buffer();
    Some((id, editor.buffer(id)?.revision()))
}

/// How long a buffer whose highlighting stopped waits before a mode set again tries again, so
/// that retries, whoever asks for them, cost the worker a bounded share of its time.
const RETRY_EVERY: Duration = Duration::from_secs(10);

/// What decides what a window shows, as far as the next job cares.
type View = (WindowId, BufferId, u64, Scroll, ViewSize, bool);

/// The last job of a buffer's, and the last report taken.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Sent {
    epoch: u64,
    revision: u64,
    mode: Mode,
    /// The buffer's mode revision: a mode set since, the same one too, starts from nothing.
    mode_revision: u64,
    shown: Vec<Range<usize>>,
    loading: bool,
    /// The stamp of the last report taken: an older one that comes later is dropped.
    stamp: u64,
}

/// What to tell the worker: buffers to drop first, then jobs.
#[derive(Debug)]
pub(super) struct Orders<Key = BufferId> {
    pub(super) drops: Vec<Key>,
    pub(super) jobs: Vec<Job<Key>>,
}

impl<Key> Default for Orders<Key> {
    fn default() -> Self {
        Self {
            drops: Vec::new(),
            jobs: Vec::new(),
        }
    }
}

/// What taking a report did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Taken {
    /// Whether a window shows runs it changed, so that a frame is due.
    pub(super) redraw: bool,
    /// What to say, if the buffer's highlighting failed.
    pub(super) message: Option<String>,
}

impl<Key> Orders<Key> {
    fn is_empty(&self) -> bool {
        self.drops.is_empty() && self.jobs.is_empty()
    }
}

impl Syntax {
    /// The buffers the worker must drop now: killed, or no longer allowed, as a buffer that
    /// turned secret is. One walk over the buffers, and none while the worker has nothing.
    pub(super) fn drops(&mut self, editor: &Editor) -> Vec<BufferId> {
        let mut drops = Vec::new();
        if self.sent.is_empty() && self.stopped.is_empty() {
            return drops;
        }
        self.present.clear();
        self.present.extend(
            highlightable(editor)
                .map(|(id, buffer)| (id, (buffer.mode(), allowed(editor, buffer)))),
        );
        let present = &self.present;
        self.sent.retain(|id, sent| {
            let keep = present
                .get(id)
                .is_some_and(|(mode, allowed)| *mode == sent.mode && *allowed);
            if !keep {
                drops.push(*id);
            }
            keep
        });
        self.stopped.retain(|id, _| present.contains_key(id));
        drops
    }

    /// Everything to tell the worker after a command: the drops, and a job for each buffer
    /// whose text or shown ranges moved. No screen is looked at while no buffer can be
    /// highlighted.
    pub(super) fn orders(&mut self, editor: &Editor) -> Orders {
        let mut orders = Orders {
            drops: self.drops(editor),
            ..Orders::default()
        };
        let mut wanted = highlightable(editor)
            .filter(|(id, buffer)| {
                let revision = buffer.mode_revision();
                let held = self.stopped.get(id).is_some_and(|(stopped, at)| {
                    *stopped == revision || at.elapsed() < RETRY_EVERY
                });
                allowed(editor, buffer) && !held
            })
            .peekable();
        if wanted.peek().is_none() {
            return orders;
        }
        let wanted: Vec<BufferId> = wanted.map(|(id, _)| id).collect();
        self.refresh_shown(editor);
        for id in wanted {
            if let Some(job) = self.job(editor, id) {
                orders.jobs.push(job);
            }
        }
        orders
    }

    /// The job for buffer `id`, if its text or what windows show of it moved.
    fn job(&mut self, editor: &Editor, id: BufferId) -> Option<Job<BufferId>> {
        let buffer = editor.buffer(id)?;
        let mode = buffer.mode();
        // A mode set again gives a buffer whose highlighting failed another try (ADR 0111).
        self.stopped.remove(&id);
        let language = Language::of(mode)?;
        let loading = buffer.loading().is_some();
        let last = self.sent.get(&id);
        let same_shown = |last: &Sent| {
            let shown = self.shown.iter().filter(|(shown, _)| *shown == id);
            shown.map(|(_, range)| range).eq(last.shown.iter())
        };
        // While a file loads, the snapshots after the first are skipped. A mode set since, even
        // twice in one command and back, is a change: it forgot the highlights.
        let mode_revision = buffer.mode_revision();
        let unchanged = last.is_some_and(|last| {
            last.mode_revision == mode_revision
                && ((loading && last.loading)
                    || (last.revision == buffer.revision() && same_shown(last)))
        });
        if unchanged {
            return None;
        }
        let edits = last
            .filter(|last| last.mode == mode && last.mode_revision == mode_revision)
            .and_then(|last| buffer.splices_since(last.revision));
        let (epoch, stamp) = match (&edits, last) {
            (Some(_), Some(last)) => (last.epoch, last.stamp),
            _ => {
                self.epochs += 1;
                (self.epochs, 0)
            }
        };
        let shown: Vec<Range<usize>> = self
            .shown
            .iter()
            .filter(|(shown, _)| *shown == id)
            .map(|(_, range)| range.clone())
            .collect();
        let sent = Sent {
            epoch,
            revision: buffer.revision(),
            mode,
            mode_revision,
            shown: shown.clone(),
            loading,
            stamp,
        };
        self.sent.insert(id, sent);
        Some(Job {
            buffer: id,
            language,
            epoch,
            revision: buffer.revision(),
            text: buffer.text().clone(),
            edits,
            shown,
        })
    }

    /// Builds what windows show again only if a window, its buffer's text, its scroll or its
    /// size moved. A wrap that settles rows meanwhile is seen at the next of those: for jobs,
    /// the ranges only order work. A report reads them afresh.
    fn refresh_shown(&mut self, editor: &Editor) {
        let key = |window: &Window| -> View {
            (
                window.id(),
                window.buffer(),
                editor.shown(window).revision(),
                window.scroll(),
                window.size(),
                window.wrap().is_some(),
            )
        };
        let same = self.described == described(editor)
            && editor.windows().len() == self.views.len()
            && editor
                .windows()
                .map(key)
                .zip(&self.views)
                .all(|(now, was)| now == *was);
        if same {
            return;
        }
        self.views.clear();
        self.views.extend(editor.windows().map(key));
        self.described = described(editor);
        editor.shown_ranges(&mut self.shown);
    }

    /// Takes a report into its buffer: whether a frame is due, and what to say if its
    /// highlighting failed. A report for a buffer the worker was told to drop, from before its
    /// last new start, or older than one already taken, is ignored.
    pub(super) fn take(&mut self, editor: &mut Editor, report: Report<BufferId>) -> Taken {
        let Some(sent) = self.sent.get_mut(&report.buffer) else {
            return Taken::default();
        };
        if report.epoch != sent.epoch || report.stamp <= sent.stamp {
            return Taken::default();
        }
        sent.stamp = report.stamp;
        // Two seeks a window, with no row laid out.
        editor.shown_ranges(&mut self.drawn);
        // So does a server's description at point (ADR 0138).
        let previewed = matches!(editor.preview(), Some(Preview::Buffer { id, .. }) if id == report.buffer)
            || editor
                .hover()
                .is_some_and(|shown| shown.buffer() == report.buffer);
        let Some(buffer) = editor.buffer_mut(report.buffer) else {
            return Taken::default();
        };
        // O(1): the runs are shared.
        let before = buffer.highlights().cloned();
        let why = match report.outcome {
            Outcome::Highlights(highlights) => {
                // False for a report the buffer cannot take, which changes nothing.
                buffer.take_highlights(report.revision, highlights);
                None
            }
            Outcome::Failed(failure) => Some(why(failure)),
            Outcome::Lost => Some("the highlighter failed"),
        };
        let message = why.map(|why| {
            self.sent.remove(&report.buffer);
            let stopped = (buffer.mode_revision(), Instant::now());
            self.stopped.insert(report.buffer, stopped);
            buffer.drop_highlights();
            format!("Highlighting stopped in {}: {why}", buffer.name())
        });
        let after = buffer.highlights();
        // The preview float shows a part of its buffer no window's range gives.
        let everywhere = previewed.then_some(0..usize::MAX);
        let redraw = (self.drawn.iter())
            .filter(|(id, _)| *id == report.buffer)
            .map(|(_, range)| range)
            .chain(everywhere.as_ref())
            .any(|range| changed(before.as_ref(), after, range));
        Taken { redraw, message }
    }
}

/// The most runs a report's comparison steps over in one shown range. A window that shows long
/// lines whole, unwrapped, may show far more runs than it draws: past this, the report is taken
/// as changed, and costs a frame instead.
const MOST_RUNS_COMPARED: usize = 4096;

/// Whether `before` and `after` differ in `range`, where no highlights are plain text. O(log n)
/// and a step per run in the range, at most [`MOST_RUNS_COMPARED`].
fn changed(before: Option<&Highlights>, after: Option<&Highlights>, range: &Range<usize>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => {
            let (mut was, mut is) = (before.runs(range.clone()), after.runs(range.clone()));
            for _ in 0..MOST_RUNS_COMPARED {
                match (was.next(), is.next()) {
                    (None, None) => return false,
                    (was, is) if was != is => return true,
                    _ => {}
                }
            }
            true
        }
        // Neighbouring runs never have the same highlight, so this looks at two at most.
        (one, other) => one.or(other).is_some_and(|highlights| {
            (highlights.runs(range.clone())).any(|(_, highlight)| highlight.is_some())
        }),
    }
}

/// Whether the worker may have `buffer`: highlighting is on, and the buffer may be highlighted.
fn allowed(editor: &Editor, buffer: &oso_core::buffer::Buffer) -> bool {
    editor.font_lock() && Language::of(buffer.mode()).is_some() && buffer.can_highlight()
}

/// Why a buffer's highlighting stopped, for the echo area: nothing of its text or its path.
fn why(failure: Failure) -> &'static str {
    match failure {
        Failure::TooLarge => "it is too large",
        Failure::TooSlow => "it took too long",
        Failure::Incompatible => "this build cannot parse its language",
        Failure::LongLine => "a line is longer than 16 KiB",
    }
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
        if orders.is_empty() || (orders.jobs.is_empty() && self.syntax.pool.is_none()) {
            return;
        }
        let Some(pool) = self.syntax_pool() else {
            return;
        };
        pool.send(Orders {
            drops: orders.drops.into_iter().map(SyntaxKey::Buffer).collect(),
            jobs: (orders.jobs.into_iter())
                .map(|job| Job {
                    buffer: SyntaxKey::Buffer(job.buffer),
                    language: job.language,
                    epoch: job.epoch,
                    revision: job.revision,
                    text: job.text,
                    edits: job.edits,
                    shown: job.shown,
                })
                .collect(),
        });
    }

    /// The syntax threads, started at the first call; none if they could not start.
    fn syntax_pool(&mut self) -> Option<&Pool<SyntaxKey>> {
        if self.syntax.broken {
            return None;
        }
        if self.syntax.pool.is_none() {
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
                    return None;
                }
            }
        }
        self.syntax.pool.as_ref()
    }

    /// A handle for a review's view to open its sides with, starting the threads; none while
    /// highlighting is off or its threads could not start.
    pub(super) fn side_documents(&mut self) -> Option<Documents<SyntaxKey>> {
        if !self.editor.font_lock() {
            return None;
        }
        Some(self.syntax_pool()?.documents())
    }

    /// The worker highlighted, or stopped, a buffer or a review's side.
    pub(super) fn syntax_reported(&mut self, report: Report<SyntaxKey>) {
        let Report {
            buffer,
            epoch,
            stamp,
            revision,
            outcome,
            done,
        } = report;
        let id = match buffer {
            SyntaxKey::Buffer(id) => id,
            SyntaxKey::Side(review, key) => {
                let report = Report {
                    buffer: key,
                    epoch,
                    stamp,
                    revision,
                    outcome,
                    done,
                };
                return self.review_side_reported(review, report);
            }
        };
        let report = Report {
            buffer: id,
            epoch,
            stamp,
            revision,
            outcome,
            done,
        };
        let taken = self.syntax.take(&mut self.editor, report);
        if let Some(message) = taken.message {
            self.editor.message(message);
            self.request_redraw();
        } else if taken.redraw {
            self.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests;
