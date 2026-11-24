//! A native session's replies highlighted as Markdown (ADR 0165, part 2g; ADR 0111): once a
//! reply is whole, its text is opened as a document no buffer owns on the syntax threads, and
//! the report's runs are put on its row, over a fence's code and what is drawn as written; the
//! rest keeps the faces it is drawn with. The threads parse,
//! fences in their own language too, and the session's pure part projects the runs.
//!
//! Must not depend on: key presses or how a frame is drawn; nothing here parses.
//!
//! - A reply is opened once its block closed and only if its text is at most
//!   [`MAX_REPLY`](crate::sessions::native::markdown::MAX_REPLY) bytes, under a key never used before; a report counts only while its reply
//!   waits under that key, and only the one that says it is done. A row drawn again
//!   makes what was asked for it stale, closing it if open, and a whole reply drawn again is
//!   opened again.
//! - At most [`MAX_OPEN`] replies are open at once, of every session, and at most
//!   [`MAX_WAITING`] wait, the newest first; the oldest past that keep the faces they
//!   are drawn with. They share the
//!   schedule's documents with a review's sides: a full schedule makes them wait a second. A
//!   reply on screen as it opens ranks as shown; the others go after buffers no window shows.
//!   The worker's own limits (ADR 0111) bound its time: a reply that fails, too slow or with a
//!   long line, stays as drawn, and nothing is said or logged.
//! - A reply is parsed whatever it quotes, as its buffer is never judged secret (an exception
//!   to ADR 0111, Q2, that ADR 0165, part 2g, gives the reasons for). The buffer's verdict still
//!   goes with the text, and one that is not ordinary would not be opened.
//! - `global-font-lock-mode` off closes what is open; the editor drops every buffer's
//!   highlights, a session's faces too. On again, a session's rows' faces are laid again once a
//!   window shows it, in slices ([`faces`]), and then its newest whole replies are asked for,
//!   before the others, at most [`MAX_WAITING`] of them; so turning it on costs the sessions on screen,
//!   not every session.
//! - What a report held is freed off the UI thread.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::editor::Editor;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::{Language, Outcome, Refused, Report};

mod faces;

use faces::{Faces, lay_faces};

use super::{Native, NativeSessions};
use crate::app::App;
use crate::app::freer::Freer;
use crate::app::syntax::{Documents, SyntaxKey};
use crate::sessions::native::markdown::{highlightable, project};
use crate::sessions::native::rows::{Kind, Row};
use crate::sessions::worker::SessionKey;

/// The most replies open on the syntax threads at once: as many as hold a tree there at once
/// (`MAX_STARTED` in `oso_syntax`'s documents no buffer owns, ADR 0122, step 8), so that a
/// reply finishing has the next one ready and no more wait there.
pub(in crate::app) const MAX_OPEN: usize = 4;

/// The most replies that wait to be opened: past it, the oldest stay as drawn.
pub(in crate::app) const MAX_WAITING: usize = 256;

/// How long a schedule that was full waits before a reply is offered again.
const FULL_WAIT: Duration = Duration::from_secs(1);

/// What a native session's rows say for highlighting, kept with the session.
#[derive(Debug, Default)]
pub(super) struct Marks {
    /// Rows drawn since the app last looked.
    drawn: Vec<usize>,
    /// Replies drawn while they streamed: opened once whole.
    streaming: Vec<usize>,
    /// The key each reply waits or is open under.
    keys: HashMap<usize, u64>,
    /// Replies whose highlights are on their row.
    highlighted: HashSet<usize>,
    /// Keys made stale by a row drawn again: closed if open.
    stale: Vec<u64>,
    /// The session's faces, once highlighting turned off dropped them.
    faces: Faces,
}

impl Marks {
    /// Whether the faces of its rows are, or may be, missing.
    fn lost(&self) -> bool {
        self.faces.lost()
    }

    /// Row `row` was drawn again: what was asked for it is stale. A reply that grows
    /// draws its row once a delta: noted once.
    pub(super) fn drawn(&mut self, row: usize) {
        if let Some(key) = self.keys.remove(&row) {
            self.stale.push(key);
        }
        self.highlighted.remove(&row);
        if self.drawn.last() != Some(&row) {
            self.drawn.push(row);
        }
    }
}

/// What opens and closes replies on the syntax threads: the app's, or a test's stand-in.
pub(in crate::app) trait Opener {
    /// Opens `text` under `key`, `shown` if a window shows its row.
    fn open(&self, key: u64, verdict: Verdict, text: Rope, shown: bool) -> Result<(), Refused>;
    /// Closes what is open under `key`.
    fn close(&self, key: u64);
}

impl Opener for Documents<SyntaxKey> {
    fn open(&self, key: u64, verdict: Verdict, text: Rope, shown: bool) -> Result<(), Refused> {
        // The whole reply goes first while a window shows its row.
        let shown = shown.then(|| 0..text.len()).into_iter().collect();
        let key = SyntaxKey::Reply(key);
        Documents::open(self, key, Language::Markdown, verdict, text, shown)
    }

    fn close(&self, key: u64) {
        Documents::close(self, SyntaxKey::Reply(key));
    }
}

/// The replies of every native session that wait or are open.
#[derive(Default)]
pub(in crate::app) struct Replies {
    /// The last key given out.
    next: u64,
    /// The replies open, by key: whose session, and which row.
    open: HashMap<u64, (SessionKey, usize)>,
    /// The replies that wait, the newest first.
    waiting: VecDeque<(SessionKey, usize, u64)>,
    /// The threads' handle, once a reply was opened while highlighting is on.
    opener: Option<Box<dyn Opener>>,
    /// Whether highlighting was on when last looked.
    on: bool,
    /// Until when nothing is offered, as the schedule was full.
    full_until: Option<Instant>,
    /// What windows show, read when replies are opened.
    shown: Vec<(BufferId, Range<usize>)>,
}

impl fmt::Debug for Replies {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Replies")
            .field("open", &self.open.len())
            .field("waiting", &self.waiting.len())
            .field("on", &self.on)
            .finish_non_exhaustive()
    }
}

impl Replies {
    /// Asks for reply `row` of session `session` to be highlighted, the newest first.
    fn ask(&mut self, session: SessionKey, marks: &mut Marks, row: usize) {
        self.next += 1;
        marks.keys.insert(row, self.next);
        self.waiting.push_front((session, row, self.next));
        // The oldest past the bound stay as drawn; its key is left stale.
        self.waiting.truncate(MAX_WAITING);
    }

    /// Looks at the rows each session drew since, and at the replies that streamed: each
    /// that is whole now is asked for if `asking`, as highlighting is on, and the session's
    /// faces are on its rows. What a row drawn again made stale is closed.
    fn look(&mut self, natives: &mut HashMap<SessionKey, Native>, asking: bool) {
        for (session, native) in natives {
            let marks = &mut native.markdown;
            for key in marks.stale.drain(..) {
                if self.open.remove(&key).is_some()
                    && let Some(opener) = &self.opener
                {
                    opener.close(key);
                }
            }
            if marks.drawn.is_empty() && marks.streaming.is_empty() {
                continue;
            }
            let rows = native.rows.rows();
            let mut drawn = std::mem::take(&mut marks.drawn);
            drawn.sort_unstable();
            drawn.dedup();
            let mut whole = Vec::new();
            // A reply drawn while it streams is looked at again; one drawn whole is asked for.
            for row in drawn {
                if streams(rows.get(row)) {
                    if !marks.streaming.contains(&row) {
                        marks.streaming.push(row);
                    }
                } else {
                    marks.streaming.retain(|at| *at != row);
                    whole.push(row);
                }
            }
            marks.streaming.retain(|row| {
                let streams = streams(rows.get(*row));
                if !streams {
                    whole.push(*row);
                }
                streams
            });
            // A session whose faces were dropped asks once they are laid again.
            let asking = asking && !marks.lost();
            for row in whole.into_iter().filter(|_| asking) {
                if rows.get(row).and_then(highlightable).is_some() {
                    self.ask(*session, marks, row);
                }
            }
        }
    }

    /// Asks again for the newest whole replies, as highlighting turned on, of the sessions
    /// whose faces it did not drop, started while it was off: those a window shows first, at
    /// most [`MAX_WAITING`], so that turning it on costs that many asks and the rows walked to
    /// find them.
    fn ask_all(&mut self, natives: &mut HashMap<SessionKey, Native>, editor: &Editor) {
        let shown = |native: &Native| editor.windows().any(|w| w.buffer() == native.buffer);
        let mut sessions: Vec<(bool, SessionKey)> = (natives.iter())
            .filter(|(_, native)| !native.markdown.lost())
            .map(|(session, native)| (!shown(native), *session))
            .collect();
        sessions.sort_unstable();
        for (_, session) in sessions {
            if let Some(native) = natives.get_mut(&session) {
                self.ask_session(session, native, false);
            }
        }
    }

    /// Asks for the whole replies of `native` not highlighted, the newest first, until
    /// [`MAX_WAITING`] wait: after those that wait, or before them if `first`, the oldest that
    /// wait then going past the bound.
    fn ask_session(&mut self, session: SessionKey, native: &mut Native, first: bool) {
        let room = if first {
            MAX_WAITING
        } else {
            MAX_WAITING.saturating_sub(self.waiting.len())
        };
        let marks = &mut native.markdown;
        let rows = native.rows.rows().iter().enumerate().rev();
        let asked: Vec<usize> = rows
            .filter(|(row, at)| highlightable(at).is_some() && !marks.highlighted.contains(row))
            .map(|(row, _)| row)
            .take(room)
            .collect();
        let at = if first { 0 } else { self.waiting.len() };
        for (index, row) in asked.into_iter().enumerate() {
            self.next += 1;
            marks.keys.insert(row, self.next);
            self.waiting.insert(at + index, (session, row, self.next));
        }
        // Those past the bound stay as drawn; their keys are left stale.
        self.waiting.truncate(MAX_WAITING);
    }

    /// Opens the newest replies that wait while fewer than [`MAX_OPEN`] are, skipping those
    /// asked for again since, or no longer whole.
    fn fill(&mut self, natives: &mut HashMap<SessionKey, Native>, editor: &Editor) {
        let Some(opener) = &self.opener else {
            return;
        };
        if self.waiting.is_empty() || self.open.len() >= MAX_OPEN {
            return;
        }
        if self.full_until.is_some_and(|until| Instant::now() < until) {
            return;
        }
        self.full_until = None;
        // Two seeks a window, with no row laid out.
        editor.shown_ranges(&mut self.shown);
        while self.open.len() < MAX_OPEN {
            let Some((session, row, key)) = self.waiting.pop_front() else {
                return;
            };
            let Some(native) = natives.get_mut(&session) else {
                continue;
            };
            if native.markdown.keys.get(&row) != Some(&key) {
                continue;
            }
            let Some(text) = native.rows.rows().get(row).and_then(highlightable) else {
                native.markdown.keys.remove(&row);
                continue;
            };
            let Some(buffer) = editor.buffer(native.buffer) else {
                continue;
            };
            let range = row_range(native, row);
            let shown = (self.shown.iter()).any(|(id, shown)| {
                *id == native.buffer && shown.start < range.end && range.start < shown.end
            });
            // One copy, of at most 64 KiB.
            match opener.open(key, buffer.verdict(), Rope::from(text), shown) {
                Ok(()) => {
                    self.open.insert(key, (session, row));
                }
                Err(Refused::Full) => {
                    // Another document closes first: until a second has passed, the reply
                    // is not copied again.
                    self.waiting.push_front((session, row, key));
                    self.full_until = Some(Instant::now() + FULL_WAIT);
                    return;
                }
                Err(Refused::MayHoldSecrets | Refused::TooLarge) => {
                    native.markdown.keys.remove(&row);
                }
            }
        }
    }

    /// Forgets what waits or is open for a session that is gone.
    fn forget_gone(&mut self, natives: &HashMap<SessionKey, Native>) {
        let opener = &self.opener;
        self.open.retain(|key, (session, _)| {
            let keep = natives.contains_key(session);
            if !keep && let Some(opener) = opener {
                opener.close(*key);
            }
            keep
        });
        self.waiting
            .retain(|(session, _, _)| natives.contains_key(session));
    }

    /// Closes everything, as highlighting turned off.
    fn close_all(&mut self) {
        if let Some(opener) = self.opener.take() {
            for key in self.open.keys() {
                opener.close(*key);
            }
        }
        self.open.clear();
        self.waiting.clear();
    }
}

impl App {
    /// After each event: highlighting turned off or on, the replies that are whole since are
    /// asked for, and the newest that wait are opened, starting the syntax threads if they
    /// have not.
    pub(super) fn sync_reply_markdown(&mut self) {
        let on = self.editor.font_lock();
        if sync(&mut self.hosting.natives, &mut self.editor, &self.freer, on) {
            self.request_redraw();
        }
        let replies = &self.hosting.natives.replies;
        if replies.on && replies.opener.is_none() && !replies.waiting.is_empty() {
            let opener = self.unowned_documents();
            self.hosting.natives.replies.opener =
                opener.map(|documents| Box::new(documents) as Box<dyn Opener>);
        }
        let natives = &mut self.hosting.natives;
        natives.replies.fill(&mut natives.map, &self.editor);
    }

    /// The syntax threads highlighted, or gave up on, a reply.
    pub(in crate::app) fn native_reply_reported(&mut self, report: Report<u64>) {
        if reported(
            &mut self.hosting.natives,
            &mut self.editor,
            &self.freer,
            report,
        ) {
            self.request_redraw();
        }
    }
}

/// Follows highlighting turning off or on, and asks for the replies whole since. Whether a
/// window shows runs that changed, so that a frame is due.
fn sync(natives: &mut NativeSessions, editor: &mut Editor, freer: &Freer, on: bool) -> bool {
    let replies = &mut natives.replies;
    if on != replies.on {
        replies.on = on;
        for native in natives.map.values_mut() {
            let marks = &mut native.markdown;
            if !on {
                // The editor dropped every buffer's highlights.
                marks.faces.drop_all();
                marks.highlighted.clear();
            }
            marks.keys.clear();
            marks.stale.clear();
        }
        if on {
            replies.ask_all(&mut natives.map, editor);
        } else {
            replies.close_all();
        }
    }
    // Off, the rows drawn are still looked at, so that a reply that streams is known once
    // whole, and nothing is kept for them.
    replies.look(&mut natives.map, on);
    if !on {
        return false;
    }
    replies.forget_gone(&natives.map);
    let (mut redraw, mut slice) = (false, true);
    for (session, native) in &mut natives.map {
        let buffer = native.buffer;
        if native.markdown.lost() && editor.windows().any(|window| window.buffer() == buffer) {
            redraw |= lay_faces(editor, freer, native, &mut replies.shown, slice);
            // One slice an event, of every session.
            slice = false;
            if !native.markdown.lost() {
                replies.ask_session(*session, native, true);
            }
        }
    }
    redraw
}

/// Whether `runs` cover `range` exactly.
fn fits(runs: &[(usize, Option<Highlight>)], range: &Range<usize>) -> bool {
    runs.iter().map(|(len, _)| len).sum::<usize>() == range.len()
}

/// Puts the runs of reply `report.buffer` on its row, if its report is the done one and it
/// still waits under that key; one that failed stays as drawn, and nothing is said. Whether a
/// window shows it.
fn reported(
    natives: &mut NativeSessions,
    editor: &mut Editor,
    freer: &Freer,
    report: Report<u64>,
) -> bool {
    let key = report.buffer;
    let replies = &mut natives.replies;
    let done = report.done || !matches!(report.outcome, Outcome::Highlights(_));
    let Some(&(session, row)) = replies.open.get(&key).filter(|_| done) else {
        freer.free(report);
        return false;
    };
    replies.open.remove(&key);
    if let Some(opener) = &replies.opener {
        opener.close(key);
    }
    // Room for the next.
    replies.full_until = None;
    let native = natives.map.get_mut(&session);
    let Some(native) = native.filter(|native| native.markdown.keys.get(&row) == Some(&key)) else {
        freer.free(report);
        return false;
    };
    native.markdown.keys.remove(&row);
    let runs = match (&report.outcome, native.rows.rows().get(row)) {
        (Outcome::Highlights(highlights), Some(at)) => {
            project(at, highlights, native.row_width(row))
        }
        // Too slow, a long line or lost: it stays as drawn.
        _ => None,
    };
    let range = row_range(native, row);
    let mut redraw = false;
    if let Some(runs) = runs.filter(|runs| fits(runs, &range)) {
        native.markdown.highlighted.insert(row);
        redraw = put_runs(editor, freer, native.buffer, range, &runs);
    }
    freer.free(report);
    redraw
}

/// Where row `row` of `native` is in its buffer.
fn row_range(native: &Native, row: usize) -> Range<usize> {
    native.placed.start(row)..native.placed.start(row + 1)
}

/// Puts `runs` over `range` of native session buffer `buffer`. O(log n) and a step a run.
/// Whether a window shows the buffer, so that a frame is due.
fn put_runs(
    editor: &mut Editor,
    freer: &Freer,
    buffer: BufferId,
    range: Range<usize>,
    runs: &[(usize, Option<Highlight>)],
) -> bool {
    let Some(mut highlights) = editor.buffer(buffer).and_then(Buffer::highlights).cloned() else {
        return false;
    };
    highlights.replace(range, runs.iter().copied());
    set_highlights(editor, freer, buffer, highlights)
}

/// Puts `highlights` in native session buffer `buffer`, freeing the ones before off the UI
/// thread. Whether a window shows the buffer.
fn set_highlights(
    editor: &mut Editor,
    freer: &Freer,
    buffer: BufferId,
    highlights: Highlights,
) -> bool {
    if let Some(old) = editor.set_internal_highlights(buffer, highlights) {
        freer.free(old);
    }
    editor.windows().any(|window| window.buffer() == buffer)
}

/// Whether `row` is a reply that streams.
fn streams(row: Option<&Row>) -> bool {
    matches!(
        row.map(|row| &row.kind),
        Some(Kind::Reply {
            streaming: true,
            ..
        })
    )
}

#[cfg(test)]
pub(super) mod tests;
