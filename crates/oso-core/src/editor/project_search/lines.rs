//! `consult-line` (ADR 0154): the lines of the buffer the selected window shows, searched in the
//! prompt a project's search uses. Its input, its streaming rows, their highlights, the count
//! and the preview's split are the same; what is searched, its order, the rows' labels, the
//! preview and what `RET` does are this module's.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O. The host runs a
//! [`LineSearchRequest`] on a thread of its own, and hands the rows back.
//!
//! - Each search reads a snapshot of the buffer's text, which a rope gives in O(1), so the UI
//!   thread copies nothing of it. The rows go from the line the cursor was on as the prompt
//!   opened to the end, then from the first line to the one before it, as consult's own does
//!   with `consult-line-start-from-top` off: the first row is the nearest match at or after the
//!   cursor, and it is the one selected. Rows never move once shown.
//! - The input is read as a project's search reads it (ADR 0114, ADR 0116): orderless
//!   components, `=` and `!`, romaji as migemo's. Every input searches, the empty one too,
//!   which lists every line that is not empty; so does one of `!` components only.
//! - A search keeps at most [`MAX_ROWS`] rows, and the count says `10000+` past them. The text
//!   is read in blocks of [`BLOCK`] bytes cut at a line's end; a line longer than [`MAX_LINE`]
//!   is matched in pieces of that size, and shows as one row at most.
//! - An input that narrows the one before, whose search ended without a limit on the same
//!   revision of the buffer, looks only through the lines that search found, in a text of
//!   [`NARROW_PAST`] bytes or more.
//! - A row is the line's number, padded to the widest the buffer has, and its text around its
//!   first match, as a project's search shows one. The preview shows the buffer from memory,
//!   at the row's match, unless it may hold secrets, which no preview ever shows.
//! - The buffer's rows show, as the person sees the buffer already; a buffer that may hold
//!   secrets keeps them from `get-state`, the history and migemo's cache, as every prompt
//!   beside one does (ADR 0072).
//! - Nothing of the input, the text or the rows reaches a log; `Debug` gives a generation at
//!   most.

use std::fmt;
use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Place, Source, To};
use crate::buffer::BufferId;
use crate::complete::{Candidate, WORKER_PAST};
use crate::editor::{CompletionStyle, Editor, Preview, WalkEnd};
use crate::minibuffer::{PromptError, PromptId};
use crate::project::{BLOCK, LineQuery, MAX_LINE, MAX_ROWS, row_shown};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// How many lines a search that narrows looks through between two looks at whether it is
/// still the newest.
const NARROW_EVERY: usize = 1024;

/// The smallest text a search narrows: below it, looking through the whole text a block at a
/// time costs a few milliseconds at most, less than finding each line found before in it.
const NARROW_PAST: usize = 8 * 1024 * 1024;

/// What the prompt searching a buffer's lines keeps of it.
pub(super) struct Lines {
    /// The buffer searched, which the tiled window showed as the prompt opened.
    pub(super) buffer: BufferId,
    /// Where the cursor was then, which the rows start from.
    from: usize,
    /// The revision of the buffer the newest search read, which its rows' offsets are in.
    revision: u64,
    /// How many digits the widest line number of that revision has.
    digits: usize,
}

/// A search of a buffer's lines the host should run (ADR 0154), with
/// [`LineSearchRequest::run`]. `Debug` gives its generation only.
pub struct LineSearchRequest {
    /// The search it is, which [`Editor::lines_found`] is given back.
    pub generation: u64,
    /// The live search's generation: [`LineSearchRequest::run`] stops at its next block once
    /// this has moved past [`LineSearchRequest::generation`].
    live: Arc<AtomicU64>,
    text: Rope,
    /// The row the search starts at, from 0.
    from_row: usize,
    query: Arc<LineQuery>,
    /// The rows, from 0, of the lines the search before found, which hold every one this can
    /// find, in their order.
    narrow: Option<Arc<[usize]>>,
}

impl fmt::Debug for LineSearchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LineSearchRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A line a search of a buffer found: its number from 1, where its first match starts in the
/// text searched, and what of it a row shows. `Debug` says nothing of it.
pub struct LineRow {
    line: usize,
    offset: usize,
    text: Box<str>,
}

impl fmt::Debug for LineRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LineRow").finish_non_exhaustive()
    }
}

/// How a search of a buffer's lines ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinesEnd {
    /// A newer search took its place, or `found` said to stop: nothing more is sent.
    Stopped,
    /// It looked through every line it was to, or stopped at [`MAX_ROWS`] if `cut`.
    Ended(WalkEnd),
}

impl LineSearchRequest {
    /// Looks through the text for the lines that match, in the order the module says, handing
    /// each row to `found`, and `None` between blocks, so that the host can send what it holds
    /// as time passes; `found` gives `false` to stop. Runs on the host's thread: it costs time
    /// linear in the text, and copies a block of it at a time.
    pub fn run(&self, found: &mut dyn FnMut(Option<LineRow>) -> bool) -> LinesEnd {
        let mut search = Run {
            request: self,
            rows: 0,
            last_line: None,
            cut: false,
            found,
            hits: Vec::new(),
        };
        let ended = match &self.narrow {
            Some(rows) => search.narrowed(rows),
            None => search.blocks(),
        };
        match ended {
            Ok(()) => LinesEnd::Ended(WalkEnd { cut: search.cut }),
            Err(Stop) => LinesEnd::Stopped,
        }
    }

    fn newest(&self) -> bool {
        self.live.load(Ordering::Relaxed) == self.generation
    }
}

/// A search stopped: a newer one took its place, or the rows' taker said so.
struct Stop;

/// A search under way.
struct Run<'a> {
    request: &'a LineSearchRequest,
    rows: usize,
    /// The line, from 0, of the last row found, so that a line matched in pieces shows once.
    last_line: Option<usize>,
    cut: bool,
    found: &'a mut dyn FnMut(Option<LineRow>) -> bool,
    /// Room for a block's matching lines: where each is in the block, and its first match.
    hits: Vec<(usize, Range<usize>, Range<usize>)>,
}

impl Run<'_> {
    /// Takes the line at row `line`, from 0, `bytes` long without its line break, at `start`
    /// in the text, whose first match is `first`, as a row. `false` once no more are taken.
    fn hit(
        &mut self,
        line: usize,
        start: usize,
        bytes: &[u8],
        first: Range<usize>,
    ) -> Result<bool, Stop> {
        if self.last_line == Some(line) {
            return Ok(true);
        }
        if self.rows == MAX_ROWS {
            self.cut = true;
            return Ok(false);
        }
        self.last_line = Some(line);
        self.rows += 1;
        let offset = start + first.start.min(bytes.len());
        let row = LineRow {
            line: line + 1,
            offset,
            text: row_shown(bytes, first),
        };
        if (self.found)(Some(row)) {
            Ok(true)
        } else {
            Err(Stop)
        }
    }

    /// Whether this is still the newest search and the rows' taker still takes them, which it
    /// is told between blocks.
    fn go_on(&mut self) -> Result<(), Stop> {
        if self.request.newest() && (self.found)(None) {
            Ok(())
        } else {
            Err(Stop)
        }
    }

    /// Looks through the text from the row the search starts at to its end, then from its start
    /// to that row, a block at a time.
    fn blocks(&mut self) -> Result<(), Stop> {
        let text = &self.request.text;
        let from_row = self.request.from_row.min(text.max_point().row);
        let from = text.line_start(from_row);
        let mut block = Vec::new();
        for (start, end, first_row) in [(from, text.len(), from_row), (0, from, 0)] {
            let (mut at, mut row) = (start, first_row);
            while at < end {
                self.go_on()?;
                let block_end = block_end(text, at, end);
                block.clear();
                for chunk in text.chunks_in_range(at..block_end) {
                    block.extend_from_slice(chunk.as_bytes());
                }
                let (lines, more) = self.block(&block, at, row)?;
                if !more {
                    return Ok(());
                }
                row += lines;
                at = block_end;
            }
        }
        Ok(())
    }

    /// Looks through `block`, at `start` in the text, whose first line is at `row`: how many
    /// line breaks it holds, and whether more rows are taken.
    fn block(&mut self, block: &[u8], start: usize, row: usize) -> Result<(usize, bool), Stop> {
        let query = &*self.request.query;
        let mut hits = std::mem::take(&mut self.hits);
        hits.clear();
        let lines = if query.lists_every_line() {
            let body = block.strip_suffix(b"\n").unwrap_or(block);
            let mut at = 0;
            // No more than the rows a search keeps, and one to say there were more.
            let room = MAX_ROWS.saturating_sub(self.rows) + 1;
            for (index, line) in body.split(|&byte| byte == b'\n').enumerate() {
                let text = line.strip_suffix(b"\r").unwrap_or(line);
                if hits.len() < room
                    && let Some(first) = query.matches_listed(text)
                {
                    hits.push((index, at..at + text.len(), first));
                }
                at += line.len() + 1;
            }
            block.iter().filter(|&&byte| byte == b'\n').count()
        } else {
            query.find_in_block(block, |hit| hits.push((hit.line, hit.range, hit.first)))
        };
        let mut more = true;
        for (index, range, first) in hits.drain(..) {
            if !self.hit(row + index, start + range.start, &block[range], first)? {
                more = false;
                break;
            }
        }
        self.hits = hits;
        Ok((lines, more))
    }

    /// Looks through the lines at `rows` only, in their order, a line longer than [`MAX_LINE`]
    /// in pieces of that size, as [`Run::blocks`] does.
    fn narrowed(&mut self, rows: &[usize]) -> Result<(), Stop> {
        let text = &self.request.text;
        let last = text.max_point().row;
        let mut line = Vec::new();
        // Lines and bytes read since the last look at whether to go on.
        let (mut lines, mut read) = (0, 0);
        for &row in rows {
            if row > last {
                continue;
            }
            let (mut at, end) = (text.line_start(row), text.line_end(row));
            loop {
                if lines >= NARROW_EVERY || read >= BLOCK {
                    self.go_on()?;
                    (lines, read) = (0, 0);
                }
                let piece = text.clip_offset(end.min(at.saturating_add(MAX_LINE)), Bias::Left);
                let piece = if piece > at { piece } else { end };
                line.clear();
                for chunk in text.chunks_in_range(at..piece) {
                    line.extend_from_slice(chunk.as_bytes());
                }
                read += line.len();
                // `line_end` stops before a `\r\n`, so the line holds no `\r` of its break.
                let bytes = &line[..];
                if let Some(first) = self.request.query.matches_listed(bytes) {
                    if !self.hit(row, at, bytes, first)? {
                        return Ok(());
                    }
                    break;
                }
                if piece >= end {
                    break;
                }
                at = piece;
            }
            lines += 1;
        }
        Ok(())
    }
}

/// Where the block from `at` ends, at most `end`: at the start of the line after the one
/// [`BLOCK`] bytes in, or [`MAX_LINE`] bytes past that, at a character's start, should that line
/// be longer.
fn block_end(text: &Rope, at: usize, end: usize) -> usize {
    let near = at.saturating_add(BLOCK);
    if near >= end {
        return end;
    }
    let row = text.offset_to_point(near).row;
    // Past the line break, `\r\n` too, which `line_end` stops before.
    let next = if row < text.max_point().row {
        text.line_start(row + 1)
    } else {
        text.len()
    };
    let far = near.saturating_add(MAX_LINE);
    if next <= end && next <= far {
        return next;
    }
    let cut = text.clip_offset(far.min(end), Bias::Left);
    if cut > at { cut } else { end }
}

/// How many digits `n` has in base ten.
fn digits(n: usize) -> usize {
    n.checked_ilog10().map_or(1, |log| log as usize + 1)
}

impl Editor {
    /// Opens a prompt that searches the lines of the buffer the tiled window shows (ADR 0154),
    /// whose input is kept in the history `history`. It asks for the search of every line at
    /// once, and for one with each input that changes ([`Editor::take_line_search_request`]),
    /// whose rows come to [`Editor::lines_found`].
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] while a search is open already. Nothing changes.
    pub fn open_line_search(&mut self, history: &'static str) -> Result<PromptId, PromptError> {
        if self.project_search.prompt.is_some() {
            return Err(PromptError::Invalid);
        }
        let (window, _) = self.tiled();
        let (buffer, from) = (window.buffer(), window.selections.primary().cursor);
        let style = CompletionStyle {
            given_order: true,
            preview: true,
            ..CompletionStyle::default()
        };
        let id = self.open_completion_as("Go to line:", history, Arc::from([]), style)?;
        let lines = Lines {
            buffer,
            from,
            revision: 0,
            digits: 1,
        };
        self.start_search_prompt(id, std::path::PathBuf::new(), Source::Lines(lines));
        self.search_input_changed();
        self.relayout_prompt();
        Ok(id)
    }

    /// The search of a buffer's lines the host should run, once.
    pub fn take_line_search_request(&mut self) -> Option<LineSearchRequest> {
        match self.project_search.request.take() {
            Some(super::Request::Lines(request)) => Some(request),
            other => {
                self.project_search.request = other;
                None
            }
        }
    }

    /// The search of the buffer's lines for `query`, the input that changed, with the
    /// generation `generation`: its snapshot of the text, and the lines of `narrowed`, the rows
    /// of a search that ended with no limit, whose input `query` narrows, if the text is the
    /// same and large enough to look through them only.
    pub(super) fn line_search_request(
        &mut self,
        generation: u64,
        query: &Arc<LineQuery>,
        narrowed: Option<&[Place]>,
    ) -> Option<LineSearchRequest> {
        let lines = self.project_search.source.lines()?;
        let buffer = self.buffer(lines.buffer)?;
        let (text, revision) = (buffer.text().clone(), buffer.revision());
        let from_row = text.offset_to_point(lines.from.min(text.len())).row;
        let narrow = narrowed
            .filter(|_| text.len() >= NARROW_PAST && lines.revision == revision)
            .map(|places| places.iter().map(|place| place.line - 1).collect());
        let digits = digits(text.max_point().row + 1);
        if let Source::Lines(lines) = &mut self.project_search.source {
            lines.revision = revision;
            lines.digits = digits;
        }
        Some(LineSearchRequest {
            generation,
            live: Arc::clone(&self.project_search.live),
            text,
            from_row,
            query: Arc::clone(query),
            narrow,
        })
    }

    /// Takes in rows the search of a buffer's lines `generation` found, and with `end`, that it
    /// ended, as [`Editor::rows_found`] does for a project's files.
    pub fn lines_found(&mut self, generation: u64, rows: Vec<LineRow>, end: Option<WalkEnd>) {
        let search = &mut self.project_search;
        let Some((room, digits)) = search
            .prompt
            .filter(|_| generation == search.generation && search.running)
            .and(search.source.lines())
            .map(|lines| (MAX_ROWS.saturating_sub(search.places.len()), lines.digits))
        else {
            if rows.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(rows));
            }
            return;
        };
        let cut_here = rows.len() > room;
        let mut added = Vec::with_capacity(rows.len().min(room));
        for row in rows.into_iter().take(room) {
            let mut shown = String::with_capacity(digits.max(1) + 2 + row.text.len());
            let _ = write!(shown, "{:>digits$}: ", row.line);
            let prefix = shown.len();
            shown.push_str(&row.text);
            search.places.push(Place {
                line: row.line,
                column: 0,
                prefix,
                to: To::Line { offset: row.offset },
            });
            added.push(Candidate::unmatched(shown, ""));
        }
        self.add_search_rows(added, end, cut_here);
    }

    /// Where the match of the row at `place` is in the buffer searched as it is now: at its
    /// offset if the buffer has not changed since, else at its line's start, clipped.
    fn line_place(&self, place: &Place) -> Option<(BufferId, usize)> {
        let lines = self.project_search.source.lines()?;
        let To::Line { offset } = place.to else {
            return None;
        };
        let buffer = self.buffer(lines.buffer)?;
        let text = buffer.text();
        // An expression can match from inside a character, as `(?-u:\xA9)` in `é`.
        if buffer.revision() == lines.revision {
            return Some((lines.buffer, text.clip_offset(offset, Bias::Left)));
        }
        let row = (place.line - 1).min(text.max_point().row);
        Some((lines.buffer, text.line_start(row)))
    }

    /// What the preview of the selected row shows, if the innermost prompt searches a buffer's
    /// lines: the buffer around the row's match, unless it may hold secrets.
    pub(super) fn line_preview(&self) -> Option<Preview> {
        let item = self.prompts.last()?.candidates()?.selected_item()?;
        let place = self.project_search.places.get(item)?;
        let (id, offset) = self.line_place(place)?;
        self.buffer_preview_at(id, offset)
    }

    /// The answer of the innermost prompt's selected row, if it searches a buffer's lines: its
    /// line's number, and the input, which its history keeps unless the buffer may hold
    /// secrets. The row's place is kept for [`Editor::go_to_line_hit`].
    pub(super) fn line_answer(&mut self, item: usize) -> Option<(String, Option<String>)> {
        let place = self.project_search.places.get(item)?;
        let (id, offset) = self.line_place(place)?;
        let line = place.line;
        let withheld = self
            .buffer(id)
            .is_none_or(|buffer| buffer.verdict().is_withheld());
        self.project_search.line_hit = Some((id, offset));
        let input = self
            .project_search
            .input
            .clone()
            .flatten()
            .unwrap_or_default();
        Some((
            line.to_string(),
            (!withheld).then(|| input.trim().to_owned()),
        ))
    }

    /// Goes to the match of the row the last search of a buffer's lines took with `RET`, once:
    /// one cursor goes there, at a character's start, with the region hidden, and the mark is
    /// set where the cursor was, which puts the old mark in the ring, as `C-.` does (ADR 0088).
    /// Gives whether it went: not once the tiled window shows another buffer.
    pub fn go_to_line_hit(&mut self) -> bool {
        let Some((id, offset)) = self.project_search.line_hit.take() else {
            return false;
        };
        if self.current().0.buffer() != id {
            return false;
        }
        let offset = self.current().1.text().clip_offset(offset, Bias::Left);
        self.go_to_keeping_mark(offset);
        true
    }
}

#[cfg(test)]
#[path = "lines_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lines_costs.rs"]
mod costs;
