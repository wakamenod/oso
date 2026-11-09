//! The kill ring: what kills took, one piece per cursor, for yanks to put back (ADR 0007,
//! ADR 0074).
//!
//! Must not depend on: `buffer`, `window`, `editor` or `command`; it holds text and
//! whether it came from a buffer that may hold secrets, and the editor decides what to
//! kill and where to yank.
//!
//! A piece is made of rope slices, which share chunks with the buffer and its undo
//! history, so a kill copies no text, and a kill appended to the one before adds a
//! slice, or copies a small one into the slice at that end. Text is copied only to yank it
//! or to hand it to the clipboard, which joins the pieces with line breaks off the UI
//! thread. A [`Fingerprint`] of what the clipboard got lets text that comes back from
//! another app keep the secret flag of its entry, and [`FromClipboard`] is what a yank
//! learns from the clipboard.

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;

use crate::rope::Rope;

/// The most entries the ring keeps, as Emacs's `kill-ring-max`.
pub const MAX_ENTRIES: usize = 120;

/// The most text the ring keeps, counted as its entries' lengths. The newest entry
/// stays whatever its size, as the undo history's newest group does.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// The most text one yank inserts across all cursors, the bound typing has (ADR 0056),
/// so that many cursors cannot run the process out of memory, which aborts it with no
/// emergency save (ADR 0051).
pub const MAX_YANK: usize = 16 * 1024 * 1024;

/// The most text that goes to the OS clipboard, or comes from it. A larger entry stays
/// in the ring only.
pub const MAX_CLIPBOARD: usize = 16 * 1024 * 1024;

/// The most fingerprints of secret entries the ring keeps, also of entries it no longer
/// holds, so that a secret copied out and back still counts as secret.
const MAX_SECRET_FINGERPRINTS: usize = 64;

/// The most text an appended kill copies into the piece it joins, rather than adding a
/// part.
const MERGED_PART: usize = 64 * 1024;

/// What joins the pieces of an entry, for the clipboard and for a yank with another
/// number of cursors (ADR 0007).
pub const PIECE_SEPARATOR: &str = "\n";

/// Which way a kill went from its cursor, which decides where a kill appended to it
/// goes: after it, or before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// `M-d`, `C-k`, a region.
    Forward,
    /// `M-DEL`, `C-w` without a region, `C-k` with a negative count.
    Backward,
}

/// What one kill took at one cursor, and what kills appended to it took there.
#[derive(Clone, Default)]
pub struct Piece {
    parts: Vec<Rope>,
    len: usize,
}

impl fmt::Debug for Piece {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Piece({} bytes)", self.len) // Never the text: it could reach a log.
    }
}

impl From<Rope> for Piece {
    fn from(text: Rope) -> Self {
        let len = text.len();
        let parts = if text.is_empty() {
            Vec::new()
        } else {
            vec![text]
        };
        Self { parts, len }
    }
}

impl Piece {
    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether it holds no text.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Its text, as the ropes it is made of, which share chunks with what it was killed from.
    pub fn parts(&self) -> &[Rope] {
        &self.parts
    }

    /// Its text, in chunks.
    pub fn chunks(&self) -> impl Iterator<Item = &str> + '_ {
        self.parts.iter().flat_map(Rope::chunks)
    }

    /// Its text, copied. O(its length).
    pub fn text(&self) -> String {
        let mut text = String::with_capacity(self.len);
        self.chunks().for_each(|chunk| text.push_str(chunk));
        text
    }

    /// Adds what a kill appended to this one took. A small part is copied into the part
    /// at that end, in O(its length + log n), so a run of kills keeps a piece in a few
    /// parts, and cloning a piece stays cheap; a large one stays a part of its own, so a
    /// huge kill is never copied.
    fn add(&mut self, other: Self, direction: Direction) {
        self.len += other.len;
        let edge = match direction {
            Direction::Forward => self.parts.last_mut(),
            Direction::Backward => self.parts.first_mut(),
        };
        if let Some(edge) = edge.filter(|_| other.len <= MERGED_PART) {
            let text = other.text();
            match direction {
                Direction::Forward => edge.push_str(&text),
                Direction::Backward => edge.insert(0, &text),
            };
            return;
        }
        match direction {
            Direction::Forward => self.parts.extend(other.parts),
            Direction::Backward => {
                self.parts.splice(0..0, other.parts);
            }
        }
    }
}

/// The length and a hash of an entry's text as the clipboard got it, joined. The worker
/// that writes the clipboard works it out, so the UI thread never reads the text, and
/// text from another app that matches a secret entry counts as secret too (ADR 0074).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    /// The text's length in bytes.
    pub len: usize,
    /// A keyed hash of it, from a key that lives only in this process.
    pub hash: u64,
}

/// What the OS clipboard held when a yank asked (ADR 0074).
#[derive(Clone)]
pub enum FromClipboard {
    /// What oso wrote last, with nothing newer from another app: the ring's newest entry
    /// is what it holds.
    Own,
    /// Text another app put there.
    Text {
        /// The text, at most [`MAX_CLIPBOARD`] bytes, built into a rope off the UI thread.
        text: Rope,
        /// The clipboard marks it concealed, as a password manager does.
        concealed: bool,
        /// Its fingerprint, to find a secret entry that went there.
        fingerprint: Fingerprint,
    },
    /// No text, or no clipboard on this system.
    Nothing,
    /// Text over [`MAX_CLIPBOARD`] bytes, which was not read.
    TooLarge,
    /// No answer in time.
    NoAnswer,
}

impl PartialEq for FromClipboard {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Text {
                    text,
                    concealed,
                    fingerprint,
                },
                Self::Text {
                    text: other_text,
                    concealed: other_concealed,
                    fingerprint: other_fingerprint,
                },
            ) => {
                (concealed, fingerprint) == (other_concealed, other_fingerprint)
                    && text.len() == other_text.len()
                    && text
                        .chunks()
                        .flat_map(str::bytes)
                        .eq(other_text.chunks().flat_map(str::bytes))
            }
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

impl Eq for FromClipboard {}

impl fmt::Debug for FromClipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Own => f.write_str("Own"),
            // Never the text: it could reach a log.
            Self::Text {
                text, concealed, ..
            } => f
                .debug_struct("Text")
                .field("len", &text.len())
                .field("concealed", concealed)
                .finish_non_exhaustive(),
            Self::Nothing => f.write_str("Nothing"),
            Self::TooLarge => f.write_str("TooLarge"),
            Self::NoAnswer => f.write_str("NoAnswer"),
        }
    }
}

/// One entry of the ring: the pieces of one kill, or of kills appended to each other.
#[derive(Debug)]
pub struct KillEntry {
    id: u64,
    pieces: Vec<Piece>,
    secret: bool,
    /// What the clipboard got of its text as it is now, or gave it.
    written: Option<Fingerprint>,
    /// It holds text another app put on the clipboard.
    from_clipboard: bool,
}

impl KillEntry {
    /// What tells this entry from every other the ring has held, also once it has grown.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Its pieces, one per cursor that killed, in buffer order.
    pub fn pieces(&self) -> &[Piece] {
        &self.pieces
    }

    /// Whether it holds text another app put on the clipboard, which no client of the dev
    /// socket may read through oso (ADR 0074).
    pub fn came_from_clipboard(&self) -> bool {
        self.from_clipboard
    }

    /// Whether it came from a buffer that may hold secrets, or from the clipboard with
    /// text that counts as secret.
    pub fn is_secret(&self) -> bool {
        self.secret
    }

    /// The length of its pieces joined with [`PIECE_SEPARATOR`].
    pub fn len(&self) -> usize {
        let text: usize = self.pieces.iter().map(Piece::len).sum();
        text + PIECE_SEPARATOR.len() * self.pieces.len().saturating_sub(1)
    }

    /// Whether its pieces hold no text.
    pub fn is_empty(&self) -> bool {
        self.pieces.iter().all(Piece::is_empty)
    }

    /// Its pieces joined with [`PIECE_SEPARATOR`], copied. O(its length).
    pub fn joined(&self) -> String {
        join(&self.pieces)
    }

    /// What the clipboard got of its text, or gave it, if anything.
    pub(crate) fn fingerprint(&self) -> Option<Fingerprint> {
        self.written
    }
}

/// `pieces` joined with [`PIECE_SEPARATOR`], copied. O(their length).
pub fn join(pieces: &[Piece]) -> String {
    let len = pieces.iter().map(Piece::len).sum::<usize>()
        + PIECE_SEPARATOR.len() * pieces.len().saturating_sub(1);
    let mut text = String::with_capacity(len);
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            text.push_str(PIECE_SEPARATOR);
        }
        piece.chunks().for_each(|chunk| text.push_str(chunk));
    }
    text
}

/// The kill ring of an editor, the newest entry first. It lives in memory only and is
/// never written anywhere (ADR 0028).
#[derive(Debug, Default)]
pub struct KillRing {
    entries: VecDeque<KillEntry>,
    bytes: usize,
    next_id: u64,
    /// The fingerprints secret entries went to the clipboard with, or came from it with,
    /// the newest last.
    secrets_written: VecDeque<Fingerprint>,
    /// What the clipboard was last seen to hold: an entry it got from the ring, or text it
    /// gave it, wherever that entry is in the ring now.
    clipboard: Option<Fingerprint>,
}

impl KillRing {
    /// An empty ring.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entries it holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The `n`th newest entry: 0 is the newest.
    pub fn get(&self, n: usize) -> Option<&KillEntry> {
        self.entries.get(n)
    }

    /// The entries, the newest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &KillEntry> {
        self.entries.iter()
    }

    /// Where the entry `id` is, 0 the newest, if the ring still holds it.
    pub(crate) fn position(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|entry| entry.id == id)
    }

    /// Moves the entry `id` to the front, as consult's `consult-yank-rotate` does, so the
    /// next yank takes it (ADR 0091). Its text and id stay.
    pub(crate) fn bring_to_front(&mut self, id: u64) {
        if let Some(entry) = self.position(id).and_then(|at| self.entries.remove(at)) {
            self.entries.push_front(entry);
        }
    }

    /// Puts `pieces` first as a new entry and returns its id. The oldest go past the
    /// bounds.
    pub fn push(&mut self, pieces: Vec<Piece>, secret: bool) -> u64 {
        self.push_entry(pieces, secret, false)
    }

    /// Puts text another app put on the clipboard first, as an entry of one piece, and
    /// returns its id.
    pub fn push_from_clipboard(&mut self, text: Rope, secret: bool) -> u64 {
        self.push_entry(vec![Piece::from(text)], secret, true)
    }

    fn push_entry(&mut self, pieces: Vec<Piece>, secret: bool, from_clipboard: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let entry = KillEntry {
            id,
            pieces,
            secret,
            written: None,
            from_clipboard,
        };
        self.bytes += entry.len();
        self.entries.push_front(entry);
        self.trim();
        id
    }

    /// Adds `pieces` to the newest entry, each to its own piece, after them or before
    /// them as `direction` says, and returns the entry's new id. If there is no entry or
    /// its number of pieces differs, nothing changes, and the pieces come back.
    ///
    /// # Errors
    ///
    /// `pieces`, when they cannot be appended.
    pub fn append(
        &mut self,
        pieces: Vec<Piece>,
        direction: Direction,
        secret: bool,
    ) -> Result<u64, Vec<Piece>> {
        let id = self.next_id;
        let Some(entry) = self
            .entries
            .front_mut()
            .filter(|entry| entry.pieces.len() == pieces.len())
        else {
            return Err(pieces);
        };
        let before = entry.len();
        for (piece, more) in entry.pieces.iter_mut().zip(pieces) {
            piece.add(more, direction);
        }
        entry.secret |= secret;
        // It is new text for the clipboard, and its old fingerprint no longer says what
        // the clipboard holds. If it was secret, the ring keeps that one.
        entry.written = None;
        entry.id = id;
        self.next_id += 1;
        self.bytes = self.bytes - before + entry.len();
        self.trim();
        Ok(id)
    }

    /// Records what the clipboard got for the entry `id`, if it is still in the ring and
    /// has not grown since.
    pub fn note_written(&mut self, id: u64, fingerprint: Fingerprint) {
        self.clipboard = Some(fingerprint);
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.written = Some(fingerprint);
            if entry.secret {
                self.remember_secret(fingerprint);
            }
        }
    }

    fn remember_secret(&mut self, fingerprint: Fingerprint) {
        if self.secrets_written.len() == MAX_SECRET_FINGERPRINTS {
            self.secrets_written.pop_front();
        }
        self.secrets_written.push_back(fingerprint);
    }

    /// Whether the clipboard holds what it was last seen to: what it got from the ring, or
    /// gave it, as Emacs compares the selection with the last one it saw.
    pub(crate) fn clipboard_seen(&self, fingerprint: Fingerprint) -> bool {
        self.clipboard == Some(fingerprint)
    }

    /// Whether a secret entry went to the clipboard as text with this fingerprint, or came
    /// from it so: of the last [`MAX_SECRET_FINGERPRINTS`], also of entries the ring has
    /// dropped or that have grown since.
    pub fn holds_secret(&self, fingerprint: Fingerprint) -> bool {
        self.secrets_written.contains(&fingerprint)
    }

    /// Makes the entry that went to the clipboard or came from it as `fingerprint` secret,
    /// as the same text came back from the clipboard marked so.
    pub(crate) fn make_written_secret(&mut self, fingerprint: Fingerprint) {
        let written = self
            .entries
            .iter_mut()
            .find(|entry| entry.written == Some(fingerprint));
        if let Some(entry) = written {
            entry.secret = true;
        }
        self.remember_secret(fingerprint);
    }

    /// Drops the oldest entries past the bounds, but never the newest.
    fn trim(&mut self) {
        while self.entries.len() > 1 && (self.entries.len() > MAX_ENTRIES || self.bytes > MAX_BYTES)
        {
            if let Some(entry) = self.entries.pop_back() {
                self.bytes -= entry.len();
            }
        }
    }
}

/// The ranges a kill takes, from one range per cursor, in buffer order: those that
/// overlap become their union, as an edit merges them, so that no text is taken twice
/// and each range is one piece (ADR 0074). When the kill deletes, ranges that touch, and
/// an empty range at the edge of another, join too, as their cursors meet once the text
/// is gone, so that the pieces match the cursors left. An empty range left alone stays,
/// as an empty piece for its cursor.
pub fn kill_ranges(
    ranges: impl IntoIterator<Item = Range<usize>>,
    deletes: bool,
) -> Vec<Range<usize>> {
    let mut sorted: Vec<Range<usize>> = ranges
        .into_iter()
        .map(|range| range.start..range.end.max(range.start))
        .collect();
    sorted.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last)
                if range.start < last.end
                    || range.start == last.start
                    || (deletes && range.start == last.end) =>
            {
                last.end = last.end.max(range.end);
            }
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(texts: &[&str]) -> Vec<Piece> {
        texts
            .iter()
            .map(|text| Piece::from(Rope::from(*text)))
            .collect()
    }

    fn texts(entry: &KillEntry) -> Vec<String> {
        entry.pieces().iter().map(Piece::text).collect()
    }

    #[test]
    fn a_kill_is_the_newest_entry() {
        let mut ring = KillRing::new();
        assert!(ring.get(0).is_none());
        ring.push(pieces(&["one"]), false);
        ring.push(pieces(&["two", "three"]), true);
        let newest = ring.get(0).unwrap();
        assert_eq!(texts(newest), ["two", "three"]);
        assert!(newest.is_secret());
        assert_eq!(newest.len(), 9, "joined with one line break");
        assert_eq!(newest.joined(), "two\nthree");
        assert_eq!(texts(ring.get(1).unwrap()), ["one"]);
    }

    #[test]
    fn appending_goes_after_or_before_each_piece() {
        let mut ring = KillRing::new();
        let first = ring.push(pieces(&["a", "x"]), false);
        let grown = ring
            .append(pieces(&["b", "y"]), Direction::Forward, false)
            .unwrap();
        assert_ne!(grown, first, "a grown entry is new text for the clipboard");
        ring.append(pieces(&["0", "9"]), Direction::Backward, true)
            .unwrap();
        let entry = ring.get(0).unwrap();
        assert_eq!(texts(entry), ["0ab", "9xy"]);
        assert!(entry.is_secret(), "a secret kill makes the entry secret");
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn small_appended_kills_join_a_part_and_large_ones_stay_apart() {
        let mut ring = KillRing::new();
        ring.push(pieces(&["b"]), false);
        for _ in 0..100 {
            ring.append(pieces(&["c"]), Direction::Forward, false)
                .unwrap();
            ring.append(pieces(&["a"]), Direction::Backward, false)
                .unwrap();
        }
        let piece = &ring.get(0).unwrap().pieces()[0];
        assert_eq!(piece.parts.len(), 1, "a run of small kills stays one part");
        assert_eq!(
            piece.text(),
            format!("{}b{}", "a".repeat(100), "c".repeat(100))
        );

        let large = "x".repeat(MERGED_PART + 1);
        ring.append(pieces(&[&large]), Direction::Forward, false)
            .unwrap();
        let piece = &ring.get(0).unwrap().pieces()[0];
        assert_eq!(piece.parts.len(), 2, "a large kill is not copied");
        assert_eq!(piece.len(), 201 + large.len());
    }

    #[test]
    fn appending_needs_as_many_pieces() {
        let mut ring = KillRing::new();
        assert!(
            ring.append(pieces(&["a"]), Direction::Forward, false)
                .is_err()
        );
        ring.push(pieces(&["a", "b"]), false);
        assert!(
            ring.append(pieces(&["c"]), Direction::Forward, false)
                .is_err()
        );
        assert_eq!(texts(ring.get(0).unwrap()), ["a", "b"], "unchanged");
    }

    #[test]
    fn the_ring_keeps_its_bounds_and_always_the_newest() {
        let mut ring = KillRing::new();
        for _ in 0..MAX_ENTRIES + 5 {
            ring.push(pieces(&["x"]), false);
        }
        assert_eq!(ring.len(), MAX_ENTRIES);

        let big = "y".repeat(MAX_BYTES / 2 + 1);
        ring.push(pieces(&[&big]), false);
        ring.push(pieces(&[&big]), false);
        assert_eq!(ring.len(), 1, "two halves are over the bound");
        let huge = "z".repeat(MAX_BYTES + 1);
        ring.push(pieces(&[&huge]), false);
        assert_eq!(ring.len(), 1);
        assert_eq!(
            ring.get(0).unwrap().len(),
            MAX_BYTES + 1,
            "the newest stays"
        );
    }

    #[test]
    fn a_fingerprint_finds_only_a_secret_entry_with_the_same_text_written() {
        let mut ring = KillRing::new();
        let plain = ring.push(pieces(&["a"]), false);
        let secret = ring.push(pieces(&["b"]), true);
        let fingerprint = |hash| Fingerprint { len: 1, hash };
        ring.note_written(plain, fingerprint(1));
        ring.note_written(secret, fingerprint(2));
        assert!(!ring.holds_secret(fingerprint(1)), "not secret");
        assert!(ring.holds_secret(fingerprint(2)));
        assert!(!ring.holds_secret(fingerprint(3)));

        let grown = ring
            .append(pieces(&["c"]), Direction::Forward, false)
            .unwrap();
        assert_eq!(
            ring.get(0).unwrap().fingerprint(),
            None,
            "a grown entry waits for its own"
        );
        assert!(
            ring.holds_secret(fingerprint(2)),
            "what went out before it grew still counts"
        );
        ring.note_written(grown, fingerprint(4));
        assert!(ring.holds_secret(fingerprint(4)));
        for _ in 0..MAX_ENTRIES {
            ring.push(pieces(&["x"]), false);
        }
        assert!(
            ring.holds_secret(fingerprint(4)),
            "a secret the ring dropped still counts"
        );
    }

    /// `kill_ranges` over pairs, as clippy reads an array of one range as a mistake.
    fn merged(ranges: &[(usize, usize)], deletes: bool) -> Vec<(usize, usize)> {
        kill_ranges(ranges.iter().map(|&(start, end)| start..end), deletes)
            .into_iter()
            .map(|range| (range.start, range.end))
            .collect()
    }

    #[test]
    fn overlapping_ranges_become_one() {
        assert_eq!(merged(&[(4, 7), (1, 5)], false), [(1, 7)]);
        assert_eq!(
            merged(&[(1, 7), (3, 4)], true),
            [(1, 7)],
            "one inside the other"
        );
        assert_eq!(
            merged(&[(3, 3), (3, 5)], false),
            [(3, 5)],
            "an empty one at its start"
        );
        assert_eq!(
            merged(&[(7, 3)], false),
            [(7, 7)],
            "a reversed one is empty"
        );
    }

    #[test]
    fn touching_ranges_join_only_when_the_kill_deletes() {
        assert_eq!(merged(&[(0, 5), (5, 8)], false), [(0, 5), (5, 8)]);
        assert_eq!(merged(&[(0, 5), (5, 8)], true), [(0, 8)]);
        assert_eq!(
            merged(&[(0, 5), (5, 5)], true),
            [(0, 5)],
            "the cursors meet"
        );
        assert_eq!(merged(&[(0, 5), (5, 5)], false), [(0, 5), (5, 5)]);
        assert_eq!(
            merged(&[(0, 2), (4, 4), (6, 9)], true),
            [(0, 2), (4, 4), (6, 9)]
        );
    }

    proptest::proptest! {
        #[test]
        fn kill_ranges_are_sorted_apart_and_cover_the_same_text(
            ranges in proptest::collection::vec((0usize..30, 0usize..30), 0..8),
            deletes: bool,
        ) {
            let ranges: Vec<Range<usize>> = ranges.into_iter().map(|(a, b)| a..b).collect();
            let merged = kill_ranges(ranges.clone(), deletes);
            for pair in merged.windows(2) {
                proptest::prop_assert!(pair[0].end <= pair[1].start);
                proptest::prop_assert!(pair[0].start < pair[1].start);
                if deletes {
                    proptest::prop_assert!(pair[0].end < pair[1].start);
                }
            }
            for at in 0..30 {
                let covered = |list: &[Range<usize>]| list.iter().any(|range| range.contains(&at));
                proptest::prop_assert_eq!(covered(&ranges), covered(&merged));
            }
        }
    }
}
