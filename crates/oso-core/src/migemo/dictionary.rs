//! A migemo dictionary (ADR 0116): readings in hiragana and the words each gives, from a
//! file's text in C/Migemo's `migemo-dict` format or SKK's, and the words of the readings a
//! romaji string's kana start.
//!
//! Must not depend on: any module of this crate but its parent's, I/O or a log. `Debug`
//! prints only its counts, as the file is the person's own.
//!
//! - Each line is read on its own, by its shape: a reading, a tab and words between tabs
//!   (`migemo-dict`), or a reading, ` /` and words each ended by `/` (SKK). A `;` line is a
//!   comment, an SKK word's `;annotation` is cut off, an SKK Lisp word (`(concat …)`) and
//!   an okuri-ari block (`[す/愛/]`) are left out, and an SKK reading with okurigana (`かk`)
//!   drops its last letter. A word of only white space is left out in either format.
//! - A line in neither shape, a reading that is not 1 to [`MAX_READING`] characters of
//!   hiragana, `ー` and `ゔ`, a word over [`MAX_WORD`] bytes or with a control character,
//!   and what passes [`MAX_WORDS_A_READING`], [`MAX_ENTRIES`] or [`MAX_WORDS`] are left out
//!   and counted, so a crafted file gives a bounded table.
//! - The table is the text itself and two lists of ranges into it: entries sorted by
//!   reading, one a reading, and their words, so the entries a prefix starts are found by
//!   binary search. It takes the text's size, 16 bytes an entry and 8 bytes a word; while it
//!   is built, a word of a reading that several lines give takes 8 bytes more.
//! - A lookup spends a [`Budget`]: its steps as it narrows and orders the entries, then its
//!   words, so one input's lookups stay bounded whatever the file holds.

use std::cmp::Reverse;
use std::fmt;
use std::ops::Range;

use super::budget::word_cost;
use super::{Budget, Romaji, Spent};

/// The most bytes of a file's text read: past it, the rest is left out.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// The most characters of a reading.
pub const MAX_READING: usize = 64;

/// The most bytes of a word.
pub const MAX_WORD: usize = 64;

/// The most words of one reading, over every line that gives it.
pub const MAX_WORDS_A_READING: usize = 256;

/// The most entries, lines that give a reading, of a dictionary.
pub const MAX_ENTRIES: usize = 1 << 20;

/// The most words of a dictionary.
pub const MAX_WORDS: usize = 1 << 22;

/// The most entries a string's kana may start for their words to go in its pattern: past
/// it, only the readings that are the kana themselves give theirs (ADR 0116, Q7). It also
/// bounds the ranges either of two kana (`ず` or `づ`) splits the table into.
pub const MAX_PREFIX_ENTRIES: usize = 2048;

/// A range of bytes of the text, or of indices into the words.
#[derive(Clone, Copy)]
struct Span {
    start: u32,
    end: u32,
}

impl Span {
    /// The span of `range`, which [`MAX_BYTES`] and [`MAX_WORDS`] keep within `u32`.
    fn of(range: Range<usize>) -> Self {
        let narrow = |at: usize| u32::try_from(at).unwrap_or(u32::MAX);
        Self {
            start: narrow(range.start),
            end: narrow(range.end),
        }
    }

    fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// A reading and its words.
#[derive(Clone, Copy)]
struct Entry {
    /// Its bytes in the text.
    reading: Span,
    /// Its words' indices in [`Dictionary::words`].
    words: Span,
}

/// A migemo dictionary, from a file's text.
pub struct Dictionary {
    text: String,
    /// Sorted by reading, one a reading.
    entries: Vec<Entry>,
    /// Each word's bytes in the text.
    words: Vec<Span>,
    skipped: usize,
}

impl fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dictionary")
            .field("readings", &self.readings())
            .field("words", &self.words())
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

impl Dictionary {
    /// The dictionary `text` holds, in either format, each line told apart by its shape.
    /// Only its first [`MAX_BYTES`] are read, to the last line that ends within them. It
    /// never fails: what is not a dictionary's is left out, and [`Self::skipped`] counts it.
    pub fn new(mut text: String) -> Self {
        if text.len() > MAX_BYTES {
            let end = text.as_bytes()[..MAX_BYTES]
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |at| at + 1);
            text.truncate(end);
        }
        let mut read = Read::default();
        let mut at = 0;
        for line in text.split_inclusive('\n') {
            read.line(line, at);
            at += line.len();
        }
        read.into_dictionary(text)
    }

    /// How many readings it holds.
    pub fn readings(&self) -> usize {
        self.entries.len()
    }

    /// How many words it holds.
    pub fn words(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.words.range().len())
            .sum()
    }

    /// How many lines, and words of the lines read, were left out.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    fn reading(&self, entry: Entry) -> &str {
        &self.text[entry.reading.range()]
    }

    /// The words to put in `romaji`'s pattern, at most a pattern's words and bytes of them
    /// by [`word_cost`] ([`Budget::PATTERN`]): those of
    /// every reading its kana and an unfinished end start, while at most
    /// [`MAX_PREFIX_ENTRIES`] do, else of the readings that are its kana and an end exactly,
    /// none if it is only an unfinished end. The exact readings' come first, then the
    /// shorter readings'.
    #[cfg(test)]
    pub(crate) fn pattern_words(&self, romaji: &Romaji) -> Vec<&str> {
        self.pattern_words_with_budget(romaji, Budget::PATTERN).0
    }

    /// [`Self::pattern_words`], at most as many and as costly as `budget` leaves, and what
    /// they spent of it. The lookup takes at most the steps `budget` leaves, and spends what
    /// it took; if it stopped short, it gives no words.
    pub(crate) fn pattern_words_with_budget(
        &self,
        romaji: &Romaji,
        budget: Budget,
    ) -> (Vec<&str>, Spent) {
        let budget = budget.within(Budget::PATTERN);
        let mut steps = budget.steps;
        let found = self.found(romaji, &mut steps);
        let looked = budget.steps - steps;
        // Stopped short: a budget with the steps it lacked goes on.
        let short = |cost: usize| {
            let used = Budget {
                steps: looked,
                ..Budget::default()
            };
            let next = Budget {
                steps: looked + cost,
                ..Budget::default()
            };
            let next = Some(next);
            (Vec::new(), Spent { used, next })
        };
        let found = match found {
            Ok(Some(found)) => found,
            Ok(None) => Vec::new(),
            Err(Short(cost)) => return short(cost),
        };
        let count: usize = found.iter().map(|(range, _)| range.len()).sum();
        let all = count <= MAX_PREFIX_ENTRIES;
        // Choosing and ordering the entries takes a step for each looked at, so a lookup's
        // whole work is bounded.
        let choose = if all { count } else { found.len() };
        if choose > steps {
            return short(choose);
        }
        let looked = looked + choose;
        let mut chosen: Vec<(usize, bool)> = Vec::new();
        if all {
            for (range, depth) in &found {
                let exact = |at: usize| self.entries[at].reading.range().len() == *depth;
                chosen.extend(range.clone().map(|at| (at, exact(at))));
            }
        } else if !romaji.hiragana.is_empty() {
            // The entries are one a reading, so an exact one is its range's first.
            for (range, depth) in &found {
                let first = range.start;
                if self.entries[first].reading.range().len() == *depth {
                    chosen.push((first, true));
                }
            }
        }
        chosen.sort_by_key(|&(at, exact)| {
            let length = self.entries[at].reading.range().len();
            (Reverse(exact), length, at)
        });
        let words = chosen
            .into_iter()
            .flat_map(|(at, _)| self.words[self.entries[at].words.range()].iter());
        let mut spent = Spent {
            used: Budget {
                steps: looked,
                ..Budget::default()
            },
            next: None,
        };
        let mut given = Vec::new();
        for word in words.map(|span| &self.text[span.range()]) {
            let with = Budget {
                words: spent.used.words + 1,
                bytes: spent.used.bytes + word_cost(word),
                steps: looked,
            };
            if !budget.holds(with) {
                spent.next = Some(with);
                break;
            }
            spent.used = with;
            given.push(word);
        }
        (given, spent)
    }

    /// The ranges of entries whose reading starts with `romaji`'s kana and then one of its
    /// ends, apart, each with the bytes that prefix takes. Either of two kana (`ず` or `づ`) splits a
    /// range in two, and a split that starts no reading goes; `None` if more ranges than
    /// [`MAX_PREFIX_ENTRIES`] are left, as then more entries are too. Each narrowing takes
    /// its steps (see [`Budget::PATTERN`]) from `steps`, and [`Short`] if too few are left.
    fn found(&self, romaji: &Romaji, steps: &mut usize) -> Result<Option<Ranges>, Short> {
        let mut live = vec![(0..self.entries.len(), 0)];
        let mut next = Vec::new();
        let mut one = [0; 4];
        for &(c, other) in &romaji.hiragana {
            for (range, depth) in live.drain(..) {
                for c in std::iter::once(c).chain(other) {
                    let c = c.encode_utf8(&mut one).as_bytes();
                    let narrowed = self.narrow(range.clone(), depth, c, steps)?;
                    if !narrowed.is_empty() {
                        next.push((narrowed, depth + c.len()));
                    }
                }
            }
            if next.len() > MAX_PREFIX_ENTRIES {
                return Ok(None);
            }
            std::mem::swap(&mut live, &mut next);
            if live.is_empty() {
                return Ok(Some(live));
            }
        }
        if romaji.ends.is_empty() {
            return Ok(Some(live));
        }
        // An end another starts (`きゃ` after `き`) is in that one's range already.
        let ends = romaji.ends.iter().filter(|end| {
            !(romaji.ends.iter()).any(|other| other != *end && end.starts_with(other))
        });
        let ends: Vec<&str> = ends.copied().collect();
        for (range, depth) in &live {
            for end in &ends {
                let narrowed = self.narrow(range.clone(), *depth, end.as_bytes(), steps)?;
                if !narrowed.is_empty() {
                    next.push((narrowed, depth + end.len()));
                }
            }
            if next.len() > MAX_PREFIX_ENTRIES {
                return Ok(None);
            }
        }
        Ok(Some(next))
    }

    /// The entries of `range`, which share their first `depth` bytes, whose reading goes on
    /// with `bytes`, taking one step from `steps` and one more for each time the range's
    /// length halves, or [`Short`] if fewer are left.
    fn narrow(
        &self,
        range: Range<usize>,
        depth: usize,
        bytes: &[u8],
        steps: &mut usize,
    ) -> Result<Range<usize>, Short> {
        let cost = 1 + (usize::BITS - range.len().leading_zeros()) as usize;
        *steps = steps.checked_sub(cost).ok_or(Short(cost))?;
        let entries = &self.entries[range.clone()];
        let rest = |entry: &Entry| &self.reading(*entry).as_bytes()[depth..];
        let start = entries.partition_point(|entry| rest(entry) < bytes);
        let len = entries[start..].partition_point(|entry| rest(entry).starts_with(bytes));
        Ok(range.start + start..range.start + start + len)
    }
}

/// Ranges of entries, each with the bytes of the prefix they share.
type Ranges = Vec<(Range<usize>, usize)>;

/// A lookup ran out of its steps before a narrowing that takes these.
#[derive(Debug)]
struct Short(usize);

/// A dictionary being read, line by line.
#[derive(Default)]
struct Read {
    entries: Vec<Entry>,
    words: Vec<Span>,
    skipped: usize,
}

impl Read {
    /// Reads `line`, which starts at byte `at` of the text.
    fn line(&mut self, line: &str, mut at: usize) {
        let line = line.strip_suffix('\n').unwrap_or(line);
        let mut line = line.strip_suffix('\r').unwrap_or(line);
        if at == 0
            && let Some(rest) = line.strip_prefix('\u{feff}')
        {
            line = rest;
            at = '\u{feff}'.len_utf8();
        }
        if line.is_empty() || line.starts_with(';') {
            return;
        }
        let parsed = if let Some(tab) = line.find('\t') {
            Some((tab, tab + 1, Format::MigemoDict))
        } else if let Some(slash) = line.find(" /")
            && line.ends_with('/')
            && line.len() > slash + 2
        {
            Some((slash, slash + 2, Format::Skk))
        } else {
            None
        };
        let Some((reading_end, words_at, format)) = parsed else {
            self.skipped += 1;
            return;
        };
        let mut reading = &line[..reading_end];
        if format == Format::Skk
            && reading.chars().count() > 1
            && reading.ends_with(|c: char| c.is_ascii_lowercase())
        {
            reading = &reading[..reading.len() - 1];
        }
        if !is_reading(reading) || self.entries.len() == MAX_ENTRIES {
            self.skipped += 1;
            return;
        }
        let first = self.words.len();
        let body = match format {
            Format::MigemoDict => &line[words_at..],
            Format::Skk => &line[words_at..line.len() - 1],
        };
        let separator = match format {
            Format::MigemoDict => '\t',
            Format::Skk => '/',
        };
        let mut word_at = at + words_at;
        // In an SKK okuri-ari line, `[す/愛/]` repeats words for one okurigana.
        let mut in_block = false;
        for word in body.split(separator) {
            let start = word_at;
            word_at += word.len() + 1;
            if format == Format::Skk && (in_block || word.starts_with('[')) {
                in_block = word != "]";
                self.skipped += 1;
                continue;
            }
            let word = match format {
                Format::Skk => word.split(';').next().unwrap_or_default(),
                Format::MigemoDict => word,
            };
            if word.is_empty() {
                continue;
            }
            let full =
                self.words.len() - first == MAX_WORDS_A_READING || self.words.len() == MAX_WORDS;
            if full || word.trim().is_empty() || !is_word(word, format) {
                self.skipped += 1;
                continue;
            }
            self.words.push(Span::of(start..start + word.len()));
        }
        if self.words.len() == first {
            self.skipped += 1;
            return;
        }
        self.entries.push(Entry {
            reading: Span::of(at..at + reading.len()),
            words: Span::of(first..self.words.len()),
        });
    }

    /// The dictionary, its entries sorted, those of one reading made one with at most
    /// [`MAX_WORDS_A_READING`] words. A reading of one line keeps its words where they are;
    /// only one of several lines has its words put together at the end, so the build takes
    /// little more than the table.
    fn into_dictionary(self, mut text: String) -> Dictionary {
        let Self {
            mut entries,
            mut words,
            mut skipped,
        } = self;
        // Stable, so a reading's words keep the file's order.
        let reading = |entry: &Entry| &text.as_bytes()[entry.reading.range()];
        entries.sort_by(|a, b| reading(a).cmp(reading(b)));
        let mut kept = 0;
        let mut at = 0;
        while at < entries.len() {
            let first = entries[at];
            let same = |entry: &Entry| reading(entry) == reading(&first);
            let end = at + entries[at..].iter().take_while(|entry| same(entry)).count();
            let mut merged = first;
            if end - at > 1 {
                let start = words.len();
                for entry in &entries[at..end] {
                    for index in entry.words.range() {
                        if words.len() - start == MAX_WORDS_A_READING {
                            skipped += 1;
                        } else {
                            words.push(words[index]);
                        }
                    }
                }
                merged.words = Span::of(start..words.len());
            }
            entries[kept] = merged;
            kept += 1;
            at = end;
        }
        entries.truncate(kept);
        entries.shrink_to_fit();
        // The words of a reading several lines gave are in twice: keep each once, so the
        // table holds at most [`MAX_WORDS`] of them.
        let needed: usize = entries.iter().map(|entry| entry.words.range().len()).sum();
        if words.len() > needed {
            let mut compact = Vec::with_capacity(needed);
            for entry in &mut entries {
                let start = compact.len();
                compact.extend_from_slice(&words[entry.words.range()]);
                entry.words = Span::of(start..compact.len());
            }
            words = compact;
        }
        words.shrink_to_fit();
        // Reading a file may have left room past its text.
        text.shrink_to_fit();
        Dictionary {
            text,
            entries,
            words,
            skipped,
        }
    }
}

/// A line's format.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    /// C/Migemo's: a reading, a tab and words between tabs.
    MigemoDict,
    /// SKK's: a reading, ` /` and words each ended by `/`.
    Skk,
}

/// Whether `reading` is 1 to [`MAX_READING`] characters of hiragana, `ー` and `ゔ`.
fn is_reading(reading: &str) -> bool {
    let hiragana = |c: char| matches!(c, 'ぁ'..='ゖ' | 'ー');
    !reading.is_empty() && reading.chars().count() <= MAX_READING && reading.chars().all(hiragana)
}

/// Whether `word` goes in the table: at most [`MAX_WORD`] bytes with no control character,
/// and not an SKK Lisp expression.
fn is_word(word: &str, format: Format) -> bool {
    let lisp = format == Format::Skk
        && word.ends_with(')')
        && word
            .strip_prefix('(')
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_lowercase()));
    word.len() <= MAX_WORD && !word.chars().any(char::is_control) && !lisp
}

#[cfg(test)]
mod tests;
