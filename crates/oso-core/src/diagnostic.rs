//! A buffer's diagnostics, as a language server reported them (ADR 0138, Diagnostics): each
//! entry's byte range, severity, tags, source, code and message, sorted by start, with the
//! strings in one arena of the set. The ranges follow each batch of edits, as the syntax runs
//! do, until the next report replaces the set.
//!
//! Must not depend on: `buffer`, `editor` or `window`; it sees only offsets.
//!
//! - A range keeps to the text it named: text inserted at either edge stays outside, text
//!   inserted inside grows it, and a deletion around it leaves an empty range where the
//!   deletion was. An empty range is kept, as servers send them for a missing `;`.
//! - A hint is kept, for the message at point, but not counted, and the moves skip it (Q16).
//! - `Debug` gives counts, never a message: a server's text can quote what it read.
//! - Nothing here panics: a range past the text is cut to it, and the arena stops taking
//!   entries before its offsets would overflow.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use crate::syntax::Splice;

#[cfg(test)]
mod tests;

/// How serious a diagnostic is, the most serious first, as the protocol numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Information,
    /// Shown at point only: no underline and no count (Q16).
    Hint,
}

/// The protocol's tags of a diagnostic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Tags {
    /// Unused or unreachable code, drawn dim.
    pub unnecessary: bool,
    /// Deprecated code, struck through once the renderer can.
    pub deprecated: bool,
}

/// A diagnostic to put in a set, as its maker read it.
#[derive(Clone, Debug)]
pub struct New<'a> {
    /// Its bytes in the text the set is for; cut to the text, and an end before the start is
    /// read as the start.
    pub range: Range<usize>,
    pub severity: Severity,
    pub tags: Tags,
    /// What made it, as the server names it (`rustc`), or nothing.
    pub source: &'a str,
    /// Its code (`E0308`), or nothing.
    pub code: &'a str,
    pub message: &'a str,
}

/// How many of a set's diagnostics are of each counted severity, for the modeline's
/// `E2 W5 I1`. Hints are not counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Counts {
    pub errors: u32,
    pub warnings: u32,
    pub information: u32,
}

/// One entry, its strings as ranges of the arena.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    start: usize,
    end: usize,
    severity: Severity,
    tags: Tags,
    source: (u32, u32),
    code: (u32, u32),
    message: (u32, u32),
}

/// A buffer's diagnostics, sorted by start, then the most severe first. Cloning copies the
/// entries, at most a few thousand small ones, and shares the strings.
#[derive(Clone, PartialEq, Eq)]
pub struct Diagnostics {
    entries: Vec<Entry>,
    /// The largest end of each entry and those before it, for finding what meets a range by
    /// binary search though ranges overlap.
    reach: Vec<usize>,
    /// The server's name, then each entry's strings.
    arena: Arc<str>,
    server: (u32, u32),
    counts: Counts,
}

impl fmt::Debug for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diagnostics")
            .field("len", &self.entries.len())
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

/// One diagnostic of a set.
#[derive(Clone, Copy)]
pub struct Diagnostic<'a> {
    set: &'a Diagnostics,
    entry: &'a Entry,
}

impl fmt::Debug for Diagnostic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Diagnostic")
            .field("range", &self.range())
            .field("severity", &self.entry.severity)
            .finish_non_exhaustive()
    }
}

impl<'a> Diagnostic<'a> {
    /// Its bytes in the text now.
    pub fn range(&self) -> Range<usize> {
        self.entry.start..self.entry.end
    }

    pub fn severity(&self) -> Severity {
        self.entry.severity
    }

    pub fn tags(&self) -> Tags {
        self.entry.tags
    }

    /// What made it, as the server names it, or `""`.
    pub fn source(&self) -> &'a str {
        self.set.string(self.entry.source)
    }

    /// Its code, or `""`.
    pub fn code(&self) -> &'a str {
        self.set.string(self.entry.code)
    }

    /// Its whole message, which may have several lines.
    pub fn message(&self) -> &'a str {
        self.set.string(self.entry.message)
    }

    /// Its message's first line, up to any line break: `\n`, `\r`, NEL, or the line and
    /// paragraph separators, so that no text after one reads as another line.
    pub fn first_line(&self) -> &'a str {
        let message = self.message();
        message
            .split(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'])
            .next()
            .unwrap_or(message)
    }
}

impl Diagnostics {
    /// A set for a text of `len` bytes, from the server named `server`. Each range is cut to the
    /// text; the entries are sorted by start, then the most severe first. Entries whose strings
    /// would take the arena past 4 GiB are left out.
    pub fn new<'a>(server: &str, entries: impl IntoIterator<Item = New<'a>>, len: usize) -> Self {
        let mut arena = String::new();
        let server = push(&mut arena, server).unwrap_or((0, 0));
        let mut kept = Vec::new();
        for new in entries {
            let start = new.range.start.min(len);
            let end = new.range.end.clamp(start, len);
            let mark = arena.len();
            let strings = push(&mut arena, new.source).and_then(|source| {
                let code = push(&mut arena, new.code)?;
                Some((source, code, push(&mut arena, new.message)?))
            });
            let Some((source, code, message)) = strings else {
                arena.truncate(mark);
                break;
            };
            kept.push(Entry {
                start,
                end,
                severity: new.severity,
                tags: new.tags,
                source,
                code,
                message,
            });
        }
        kept.sort_by_key(|entry| (entry.start, entry.severity, entry.end));
        let mut counts = Counts::default();
        for entry in &kept {
            match entry.severity {
                Severity::Error => counts.errors += 1,
                Severity::Warning => counts.warnings += 1,
                Severity::Information => counts.information += 1,
                Severity::Hint => {}
            }
        }
        let mut set = Self {
            entries: kept,
            reach: Vec::new(),
            arena: Arc::from(arena),
            server,
            counts,
        };
        set.reach_again();
        set
    }

    /// How many diagnostics it holds, hints included.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The name of the server that reported it, which the echo area puts first.
    pub fn server(&self) -> &str {
        self.string(self.server)
    }

    /// Its counts of errors, warnings and information.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The `index`th diagnostic by start.
    pub fn get(&self, index: usize) -> Option<Diagnostic<'_>> {
        self.entries
            .get(index)
            .map(|entry| Diagnostic { set: self, entry })
    }

    /// Every diagnostic, by start.
    pub fn iter(&self) -> impl Iterator<Item = Diagnostic<'_>> {
        self.entries
            .iter()
            .map(|entry| Diagnostic { set: self, entry })
    }

    /// The diagnostics that meet `range`, by start: a range that overlaps it, or an empty one
    /// inside it or at its start. O(log n) to the first, and O(1) each after that, plus those
    /// skipped among them that end before it.
    pub fn in_range(&self, range: Range<usize>) -> impl Iterator<Item = Diagnostic<'_>> {
        // The first entry whose reach gets to the range; none before it can meet it.
        let first = self.reach.partition_point(|&reach| reach < range.start);
        self.entries[first..]
            .iter()
            .take_while(move |entry| entry.start <= range.end)
            .filter(move |entry| meets(entry, &range))
            .map(|entry| Diagnostic { set: self, entry })
    }

    /// The diagnostics that cover `offset`: from their start to before their end, or an empty
    /// one at it. By start, so not by severity.
    pub fn at(&self, offset: usize) -> impl Iterator<Item = Diagnostic<'_>> {
        self.in_range(offset..offset)
    }

    /// Where a move from `offset` goes, as `C-M-n` and `C-M-p` do (Q15): the first diagnostic
    /// that is not a hint and starts after it, `forward`, or the last that starts before it;
    /// past either end, the first or the last, with `true` for the wrap. `None` with only
    /// hints, or none. O(log n), plus the hints passed.
    pub fn step(&self, offset: usize, forward: bool) -> Option<(Diagnostic<'_>, bool)> {
        let counted = |index: &usize| self.entries[*index].severity != Severity::Hint;
        let len = self.entries.len();
        let found = if forward {
            let first = self.entries.partition_point(|entry| entry.start <= offset);
            (first..len).find(counted)
        } else {
            let end = self.entries.partition_point(|entry| entry.start < offset);
            (0..end).rev().find(counted)
        };
        let (index, wrapped) = match found {
            Some(index) => (index, false),
            None if forward => ((0..len).find(counted)?, true),
            None => ((0..len).rev().find(counted)?, true),
        };
        self.get(index).map(|diagnostic| (diagnostic, wrapped))
    }

    /// Follows a batch of edits, as [`Splice`] says, in its offsets before the batch, in place.
    /// O(entries × log splices), allocating only the shift before each splice. Each start and
    /// end moves by itself, as a mark does; see the module's rules.
    pub fn splice(&mut self, splices: &[Splice]) {
        if splices.is_empty() || self.entries.is_empty() {
            return;
        }
        // The shift before each splice.
        let mut shifts = Vec::with_capacity(splices.len());
        let mut shift = 0isize;
        for splice in splices {
            shifts.push(shift);
            shift += splice.new_len as isize - splice.old_len as isize;
        }
        for entry in &mut self.entries {
            let start = map_start(entry.start, splices, &shifts);
            let end = map_end(entry.end, splices, &shifts);
            entry.start = start;
            entry.end = end.max(start);
        }
        // The starts kept their order, as each moves as its offset's place does; a tie may
        // have changed which comes first only among equal starts, so they are sorted only then,
        // and a sorted set allocates nothing.
        let key = |entry: &Entry| (entry.start, entry.severity, entry.end);
        if !self.entries.is_sorted_by_key(key) {
            self.entries.sort_by_key(key);
        }
        self.reach_again();
    }

    /// Whether every range ends within a text of `len` bytes.
    pub(crate) fn fits(&self, len: usize) -> bool {
        self.reach.last().is_none_or(|&reach| reach <= len)
    }

    fn reach_again(&mut self) {
        self.reach.clear();
        let mut reach = 0;
        self.reach.extend(self.entries.iter().map(|entry| {
            reach = reach.max(entry.end);
            reach
        }));
    }

    fn string(&self, (start, end): (u32, u32)) -> &str {
        self.arena.get(start as usize..end as usize).unwrap_or("")
    }
}

/// Puts `text` at the arena's end, and returns where it is, if its offsets fit.
fn push(arena: &mut String, text: &str) -> Option<(u32, u32)> {
    let start = u32::try_from(arena.len()).ok()?;
    let end = u32::try_from(arena.len().checked_add(text.len())?).ok()?;
    arena.push_str(text);
    Some((start, end))
}

/// Whether `entry` meets `range`, as [`Diagnostics::in_range`] says.
fn meets(entry: &Entry, range: &Range<usize>) -> bool {
    if entry.start == entry.end {
        range.start <= entry.start && (entry.start < range.end || entry.start == range.start)
    } else {
        entry.start < range.end.max(range.start + 1) && entry.end > range.start
    }
}

fn shifted(offset: usize, shift: isize) -> usize {
    offset.saturating_add_signed(shift)
}

/// Where a start at `offset` goes: text inserted at it goes before it, a replacement that
/// starts at it stays in, and one around it leaves it after the new text.
fn map_start(offset: usize, splices: &[Splice], shifts: &[isize]) -> usize {
    // The last splice that starts at or before the offset.
    let index = splices.partition_point(|splice| splice.start <= offset);
    let Some(i) = index.checked_sub(1) else {
        return offset;
    };
    let (splice, shift) = (splices[i], shifts[i]);
    let old_end = splice.start.saturating_add(splice.old_len);
    if offset == splice.start && splice.old_len > 0 {
        shifted(splice.start, shift)
    } else if offset <= old_end {
        shifted(splice.start + splice.new_len, shift)
    } else {
        shifted(
            offset,
            shift + splice.new_len as isize - splice.old_len as isize,
        )
    }
}

/// Where an end at `offset` goes: text inserted at it stays after it, a replacement that ends
/// at it stays in, and one around it leaves it where the replaced text started.
fn map_end(offset: usize, splices: &[Splice], shifts: &[isize]) -> usize {
    // The last splice that starts before the offset.
    let index = splices.partition_point(|splice| splice.start < offset);
    let Some(i) = index.checked_sub(1) else {
        return offset;
    };
    let (splice, shift) = (splices[i], shifts[i]);
    let old_end = splice.start.saturating_add(splice.old_len);
    if offset == old_end {
        shifted(splice.start + splice.new_len, shift)
    } else if offset < old_end {
        shifted(splice.start, shift)
    } else {
        shifted(
            offset,
            shift + splice.new_len as isize - splice.old_len as isize,
        )
    }
}
