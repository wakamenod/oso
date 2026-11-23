//! What the editor knows of dictation (ADR 0175): what the person asked of it, for the host;
//! whether one records, waits for its text or is being fixed, for the modeline; where its text
//! goes; the provisional spans its text holds while Claude fixes it (step 2); and the settled
//! spans the host watches for what they teach (step 3, `settled`).
//!
//! Must not depend on: processes, threads or the host's dictation; the host says what holds,
//! takes what was asked, and gives the text and the fix.
//!
//! - **The target** is taken at the start: the selected window, its buffer, the buffer's
//!   revision and its cursors. A buffer, the minibuffer and a native session's composer are all
//!   windows, so one path covers them. Refused at the start: a passphrase prompt, a question, a
//!   terminal and a read-only buffer.
//! - **The text goes in at each of those cursors, in one undo step**, or into the kill ring when
//!   the window is gone or shows another buffer, the buffer is gone or read-only, or its text
//!   changed meanwhile; and when the text times the cursors is over [`MAX_YANK`]. A buffer that
//!   may hold secrets makes a secret entry.
//! - **A span** is where one dictation's text went, a range at each cursor. A provisional one
//!   is drawn in a face of its own until its fix lands or fails. None is made in the minibuffer
//!   or in a buffer that may hold secrets, nor at more than [`MAX_SPAN_RANGES`] cursors; at
//!   most [`MAX_SPANS`] are drawn at once, and a dictation past them gets none.
//! - Spans live here, not in the buffer: they follow every edit as markers that keep text typed
//!   at their ends outside, and go with a reload or the buffer. They are not saved, undone,
//!   copied, carried across a restart or told to agents.
//! - **A fix** replaces text inside the span only, at every range, in one undo step, and only
//!   while each range still holds the text that was sent: at most [`MAX_FIX_REPLACEMENTS`].
//! - **A settled span** (step 3) is one whose fix ended, or a plain dictation's: undrawn, kept
//!   so that the host can learn from the person's own rewrites of it. It counts the person's
//!   commands that changed its text, and ends when anything else changes it (an agent's
//!   command, oso's own edit, a slice), when its buffer may hold secrets, or past
//!   [`MAX_SETTLED`], the oldest first.

use std::ops::Range;

use super::Editor;
use crate::buffer::{Buffer, BufferId, Replacement};
use crate::frame::{self, CellRect};
use crate::kill::MAX_YANK;
use crate::minibuffer::Kind;
use crate::sum_tree::Bias;
use crate::window::{Screen, WindowId};

/// The most provisional spans drawn at once: a dictation past them is typed plain, with none.
pub(crate) const MAX_SPANS: usize = 4;

/// The most settled spans at once: one more ends the oldest. Only a bound on memory: the host
/// watches fewer, and ends its own oldest first.
pub(crate) const MAX_SETTLED: usize = 16;

/// The most cursors a span has: a dictation typed at more is typed plain, as its
/// fix would make edits at every one.
pub(crate) const MAX_SPAN_RANGES: usize = 32;

/// The most replacements a fix makes, across the span's ranges, in its one `apply_edits`:
/// about what a slice of query-replace's `!` makes in a turn (ADR 0113), a few milliseconds.
pub(crate) const MAX_FIX_REPLACEMENTS: usize = 1024;

/// What a command asks of the host about dictation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationRequest {
    /// Start a dictation, or stop the one recording (`dictation-toggle`). `prefix` says a
    /// prefix argument came with the key.
    Toggle { prefix: bool },
    /// Drop the dictation under way and what it would type (`dictation-cancel`).
    Cancel,
    /// Forget everything dictation has learned (`dictation-forget-learned`, step 3).
    Forget,
}

/// What the modeline says of dictation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DictationMark {
    /// Nothing: no dictation is under way.
    #[default]
    Off,
    /// One records.
    Recording,
    /// One stopped recording and waits for its text.
    Waiting,
    /// Claude fixes a dictation's text (step 2).
    Fixing,
}

/// Where a dictation started: the place its text goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationTarget {
    window: WindowId,
    buffer: BufferId,
    /// The buffer's revision then: the cursors hold only while it is the same.
    revision: u64,
    /// The cursors' offsets then, in order, each once.
    cursors: Vec<usize>,
    /// The buffer may hold secrets: what goes to the kill ring is a secret entry.
    secret: bool,
    /// It is a prompt's answer, in the minibuffer: its text is never fixed.
    prompt: bool,
}

/// What became of a dictation's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationPlaced {
    /// Typed at each of the target's cursors, with the span it holds, drawn or settled, if one
    /// was made.
    Typed { span: Option<DictationSpan> },
    /// Put in the kill ring, for this reason.
    Kept(&'static str),
}

/// A span: where one dictation's text went, a range at each of its cursors, provisional
/// while its fix runs and settled after.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DictationSpan(u64);

/// What a fix of a span sends: the text of its first range, the text around it, and its
/// buffer's mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictationContext {
    /// Up to the lines asked for before the text, cut at a character's start to the bytes
    /// asked for, from the end.
    pub before: String,
    /// The text the span holds.
    pub text: String,
    /// Up to the lines asked for after the text, cut at a character's end.
    pub after: String,
    /// The buffer's mode's name: never its path.
    pub mode: &'static str,
}

/// One span: provisional while drawn, settled once not.
#[derive(Debug)]
struct Span {
    id: u64,
    buffer: BufferId,
    /// Ascending and apart, each where one cursor's text went.
    ranges: Vec<Range<usize>>,
    /// Drawn in its face, while its fix runs.
    drawn: bool,
    /// The person's commands that changed its text.
    edits: u64,
    /// The command running changed its text, and has not been seen to be the person's.
    pending: bool,
    /// Something other than the person's command changed its text: it ends.
    foreign: bool,
}

/// The editor's part of dictation.
#[derive(Debug, Default)]
pub(super) struct Dictation {
    requests: Vec<DictationRequest>,
    pub(super) mark: DictationMark,
    spans: Vec<Span>,
    /// The id the next span gets.
    next_span: u64,
    /// The spans' starts and ends as they follow an edit, kept so that a key allocates none.
    starts: Vec<usize>,
    ends: Vec<usize>,
}

impl Editor {
    /// A command asks the host to start, stop or cancel a dictation.
    pub fn request_dictation(&mut self, request: DictationRequest) {
        self.dictation.requests.push(request);
    }

    /// What commands asked of dictation since the host last looked, oldest first.
    pub fn take_dictation_requests(&mut self) -> Vec<DictationRequest> {
        std::mem::take(&mut self.dictation.requests)
    }

    /// The host says whether a dictation records, waits or is being fixed. It asks for a frame
    /// itself.
    pub fn set_dictation_mark(&mut self, mark: DictationMark) {
        self.dictation.mark = mark;
    }

    /// What the modeline says of dictation.
    pub fn dictation_mark(&self) -> DictationMark {
        self.dictation.mark
    }

    /// The place a dictation started now would type into: the selected window, its buffer and
    /// its cursors.
    ///
    /// # Errors
    ///
    /// Why dictation does not type there, for the echo area.
    pub fn dictation_target(&self) -> Result<DictationTarget, &'static str> {
        let (window, buffer) = self.current();
        let prompt = self
            .prompts()
            .iter()
            .find(|prompt| prompt.buffer() == window.buffer());
        match prompt.map(crate::minibuffer::Prompt::kind) {
            Some(Kind::Passphrase) => return Err("Dictation never types a passphrase"),
            Some(Kind::Question { .. }) => return Err("Dictation does not answer a question"),
            _ => {}
        }
        if buffer.is_terminal() {
            return Err("Dictation does not type into a terminal");
        }
        if buffer.read_only().is_some() {
            return Err("Dictation cannot type here: the buffer is read-only");
        }
        let mut cursors: Vec<usize> = window.selections.iter().map(|s| s.cursor).collect();
        cursors.sort_unstable();
        cursors.dedup();
        Ok(DictationTarget {
            window: window.id(),
            buffer: window.buffer(),
            revision: buffer.revision(),
            cursors,
            secret: buffer.is_secret(),
            prompt: prompt.is_some(),
        })
    }

    /// Whether `target`'s text must wait: an input method composes in its window, which is
    /// selected, as the composition is always the selected window's.
    pub fn dictation_waits(&self, target: &DictationTarget) -> bool {
        self.preedit().is_some() && self.window().id() == target.window
    }

    /// Why the text cannot go in at `target` now, if it cannot.
    fn dictation_gone(&self, target: &DictationTarget) -> Option<&'static str> {
        let Some(window) = self.window_by_id(target.window) else {
            return Some("its window is gone");
        };
        if window.buffer() != target.buffer {
            return Some("its window shows another buffer");
        }
        let Some(buffer) = self.buffer(target.buffer) else {
            return Some("its buffer is gone");
        };
        if buffer.read_only().is_some() {
            return Some("its buffer is read-only now");
        }
        (buffer.revision() != target.revision).then_some("its text changed while you spoke")
    }

    /// Types `text` at each of `target`'s cursors, in one undo step, or puts it in the kill
    /// ring if the place is gone. The text typed is a span, unless it went into the minibuffer
    /// or a buffer that may hold secrets, or at more than [`MAX_SPAN_RANGES`] cursors: with
    /// `provisional` a drawn one, and none at all if [`MAX_SPANS`] are drawn already; without,
    /// a settled one. Empty text does nothing.
    pub fn place_dictation(
        &mut self,
        target: &DictationTarget,
        text: &str,
        provisional: bool,
    ) -> DictationPlaced {
        if let Some(why) = self.dictation_gone(target) {
            return self.keep_dictation(target, text, why);
        }
        let total = text.len().checked_mul(target.cursors.len());
        if total.is_none_or(|total| total > MAX_YANK) {
            return self.keep_dictation(target, text, "it is too much text for so many cursors");
        }
        let edits: Vec<Replacement<'_>> = target
            .cursors
            .iter()
            .map(|&at| Replacement {
                range: at..at,
                text,
            })
            .collect();
        if self.apply_edits(target.buffer, &edits).is_err() {
            return self.keep_dictation(target, text, "it could not be typed there");
        }
        // What it typed in a prompt that completes is matched, as typing is.
        self.match_completion();
        let secret = self.buffer(target.buffer).is_none_or(Buffer::is_secret);
        let drawn = self
            .dictation
            .spans
            .iter()
            .filter(|held| held.drawn)
            .count();
        let spanned = !text.is_empty()
            && !target.prompt
            && !target.secret
            && !secret
            && target.cursors.len() <= MAX_SPAN_RANGES
            && (!provisional || drawn < MAX_SPANS);
        let span = spanned.then(|| self.new_dictation_span(target, text.len(), provisional));
        DictationPlaced::Typed { span }
    }

    /// A span over the text of `len` bytes just typed at `target`'s cursors: each cursor's
    /// offset moved by the text typed at the cursors before it.
    fn new_dictation_span(
        &mut self,
        target: &DictationTarget,
        len: usize,
        drawn: bool,
    ) -> DictationSpan {
        let ranges = target
            .cursors
            .iter()
            .enumerate()
            .map(|(before, &at)| {
                let start = at + before * len;
                start..start + len
            })
            .collect();
        let id = self.dictation.next_span;
        self.dictation.next_span += 1;
        self.dictation.spans.push(Span {
            id,
            buffer: target.buffer,
            ranges,
            drawn,
            edits: 0,
            pending: false,
            foreign: false,
        });
        self.keep_settled();
        DictationSpan(id)
    }

    /// Puts `text` in the kill ring, for `why`: a secret entry if `target`'s buffer may have
    /// held secrets at the start or may now.
    pub fn keep_dictation(
        &mut self,
        target: &DictationTarget,
        text: &str,
        why: &'static str,
    ) -> DictationPlaced {
        let now = self.buffer(target.buffer).is_some_and(Buffer::is_secret);
        self.kill_new(text, target.secret || now);
        DictationPlaced::Kept(why)
    }

    fn dictation_span_at(&self, span: DictationSpan) -> Option<&Span> {
        self.dictation.spans.iter().find(|held| held.id == span.0)
    }

    /// What a fix of `span` sends: the text of its first range, up to `lines` lines on each
    /// side of it, each side cut to `max_side` bytes, and its buffer's mode. `None` once the
    /// span is gone or its buffer may hold secrets.
    pub fn dictation_context(
        &self,
        span: DictationSpan,
        lines: usize,
        max_side: usize,
    ) -> Option<DictationContext> {
        let held = self.dictation_span_at(span)?;
        let buffer = self
            .buffer(held.buffer)
            .filter(|buffer| !buffer.is_secret())?;
        let range = held.ranges.first()?.clone();
        let rope = buffer.text();
        let first = rope.offset_to_point(range.start).row;
        let last = rope.offset_to_point(range.end).row;
        let from = rope.line_start(first.saturating_sub(lines));
        let to = rope.line_end((last + lines).min(rope.line_count().saturating_sub(1)));
        let mut from = from.max(range.start.saturating_sub(max_side));
        from = rope.clip_offset(from, Bias::Right);
        let mut to = to.max(range.end).min(range.end + max_side);
        to = rope.clip_offset(to, Bias::Left);
        Some(DictationContext {
            before: rope.slice(from..range.start).to_string(),
            text: rope.slice(range.clone()).to_string(),
            after: rope.slice(range.end..to.max(range.end)).to_string(),
            mode: buffer.mode().name(),
        })
    }

    /// Fixes `span`: `edits`, byte ranges of `sent` with their new text, ascending and apart,
    /// are made at each of the span's ranges in one undo step, and the span is settled over
    /// the fixed text. The fix's own edit is not one of the person's.
    ///
    /// # Errors
    ///
    /// Why nothing was fixed, for the echo area: the span or its buffer is gone, a range no
    /// longer holds exactly `sent`, the buffer may hold secrets or is read-only now, the edits
    /// do not fit `sent`, the result is past a yank's bound, or the edits at every range are
    /// more than [`MAX_FIX_REPLACEMENTS`]. The span is settled then over what it holds, but
    /// ends when it or its buffer is gone or the buffer may hold secrets.
    pub fn fix_dictation(
        &mut self,
        span: DictationSpan,
        sent: &str,
        edits: &[(Range<usize>, &str)],
    ) -> Result<(), &'static str> {
        let Some(at) = self
            .dictation
            .spans
            .iter()
            .position(|held| held.id == span.0)
        else {
            return Err("its text is gone");
        };
        let mut held = self.dictation.spans.remove(at);
        held.drawn = false;
        let buffer = self.buffer(held.buffer).ok_or("its buffer is gone")?;
        if buffer.is_secret() {
            return Err("its buffer may hold secrets");
        }
        match self.fix_span(&held, sent, edits) {
            Ok(grown) => {
                // Each range moves by what the fix grew the ranges before it.
                let growth = grown.cast_signed() - sent.len().cast_signed();
                for (before, range) in held.ranges.iter_mut().enumerate() {
                    let start = range.start.cast_signed() + before.cast_signed() * growth;
                    let start = start.cast_unsigned();
                    *range = start..start + grown;
                }
                self.dictation.spans.push(held);
                self.keep_settled();
                Ok(())
            }
            Err(why) => {
                self.dictation.spans.push(held);
                self.keep_settled();
                Err(why)
            }
        }
    }

    /// Makes `edits` at each of `held`'s ranges, as [`Editor::fix_dictation`] says, with `held`
    /// out of the spans, so that the fix's edit is not counted as a change to it. Returns how
    /// long each range is then.
    fn fix_span(
        &mut self,
        held: &Span,
        sent: &str,
        edits: &[(Range<usize>, &str)],
    ) -> Result<usize, &'static str> {
        let buffer = self.buffer(held.buffer).ok_or("its buffer is gone")?;
        let holds = |range: &Range<usize>| {
            range.len() == sent.len() && range.end <= buffer.text().len() && {
                let mut rest = sent.as_bytes();
                buffer.text().chunks_in_range(range.clone()).all(|chunk| {
                    let (head, tail) = rest.split_at(chunk.len().min(rest.len()));
                    rest = tail;
                    head == chunk.as_bytes()
                })
            }
        };
        if !held.ranges.iter().all(holds) {
            return Err("its text was edited");
        }
        let mut end = 0;
        let mut grown = sent.len();
        for (range, text) in edits {
            let fits = end <= range.start
                && range.start <= range.end
                && range.end <= sent.len()
                && sent.is_char_boundary(range.start)
                && sent.is_char_boundary(range.end);
            if !fits {
                return Err("the fix did not fit the text");
            }
            end = range.end;
            grown = grown - range.len() + text.len();
        }
        let total = grown.checked_mul(held.ranges.len());
        if total.is_none_or(|total| total > MAX_YANK) {
            return Err("the fixed text is too much for so many cursors");
        }
        let count = edits.len().checked_mul(held.ranges.len());
        if count.is_none_or(|count| count > MAX_FIX_REPLACEMENTS) {
            return Err("it would change too many places at once");
        }
        let replacements: Vec<Replacement<'_>> = held
            .ranges
            .iter()
            .flat_map(|at| {
                edits.iter().map(move |(range, text)| Replacement {
                    range: at.start + range.start..at.start + range.end,
                    text,
                })
            })
            .collect();
        self.apply_edits(held.buffer, &replacements)
            .map_err(|_| "its buffer is read-only now")?;
        Ok(grown)
    }

    /// The cells of the text area of the tiled window `id` that provisional spans take, as
    /// `screen` shows it ([`Editor::screen_of`]), into `out`, which is emptied first: none in a
    /// buffer that may hold secrets.
    pub fn dictation_span_cells(&self, id: WindowId, screen: &Screen, out: &mut Vec<CellRect>) {
        out.clear();
        if !self.dictation.spans.iter().any(|held| held.drawn) {
            return;
        }
        let Some((window, buffer, regions)) = self.window_in_tree(id) else {
            return;
        };
        if buffer.is_secret() {
            return;
        }
        let display = buffer.display();
        for held in &self.dictation.spans {
            if held.drawn && held.buffer == window.buffer() {
                frame::match_cells(regions.text, screen, &display, &held.ranges, out);
            }
        }
    }

    /// Ends `span` with no fix: its face goes, and nothing more is learned from it.
    pub fn end_dictation_span(&mut self, span: DictationSpan) {
        self.dictation.spans.retain(|held| held.id != span.0);
    }
}

mod settled;

#[cfg(test)]
mod tests;
