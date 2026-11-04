//! The text of a buffer: a rope of UTF-8 chunks on a [`SumTree`]
//! (ADR 0022, ADR 0035).
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.
//!
//! - Byte offsets are the primary coordinate. A [`Point`] is a line and a
//!   byte column; conversions in both directions take O(log n).
//! - Line endings stay as in the file. `\n` and `\r\n` (one break) end a
//!   line; a lone `\r` does not.
//! - Chunks hold at most [`MAX_CHUNK_BYTES`] bytes and end on grapheme
//!   cluster boundaries, so a character, a `\r\n` pair or an emoji sequence
//!   is never split between chunks. Only a cluster longer than a whole chunk
//!   is split, at a character boundary.
//! - Cloning a rope is O(1), and the clone is a snapshot that later edits
//!   do not change.
//!
//! Offsets past the end clamp to it, and offsets inside a UTF-8 character
//! move back to its start. No method panics on any input.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use crate::sum_tree::{Bias, Cursor, Dimension, Item, SumTree, Summary};

/// The most bytes in one chunk, so that with an `Arc`'s counts it fits 1 KB.
#[cfg(not(test))]
pub const MAX_CHUNK_BYTES: usize = 1008;
/// Small in tests, so that chunk boundaries are everywhere.
#[cfg(test)]
pub const MAX_CHUNK_BYTES: usize = 32;

/// How many neighboring chunks an edit takes in, at most, to keep chunk
/// boundaries on grapheme cluster boundaries. Only regional indicator runs
/// (flags) longer than this many chunks can exceed it.
const MAX_BOUNDARY_CHUNKS: usize = 8;

/// A line and a byte column in that line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Point {
    /// The line, from 0.
    pub row: usize,
    /// Bytes from the start of the line.
    pub column: usize,
}

impl Point {
    /// A point at `row` and `column`.
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// The summary of a stretch of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSummary {
    /// Length in bytes.
    pub bytes: usize,
    /// Length in Unicode scalar values.
    pub chars: usize,
    /// Line breaks: `\n` and `\r\n` each count once.
    pub lines: usize,
    /// Bytes after the last line break, or all bytes if there is none.
    pub last_line_bytes: usize,
}

impl TextSummary {
    /// Summarizes `text`.
    pub fn from_text(text: &str) -> Self {
        // `\r\n` contains exactly one `\n`, and a lone `\r` is no break, so
        // counting `\n` counts line breaks.
        let lines = text.bytes().filter(|&byte| byte == b'\n').count();
        let last_line_bytes = match text.rfind('\n') {
            Some(index) => text.len() - index - 1,
            None => text.len(),
        };
        Self {
            bytes: text.len(),
            chars: text.chars().count(),
            lines,
            last_line_bytes,
        }
    }
}

impl Summary for TextSummary {
    fn add_summary(&mut self, other: &Self) {
        self.bytes += other.bytes;
        self.chars += other.chars;
        if other.lines > 0 {
            self.lines += other.lines;
            self.last_line_bytes = other.last_line_bytes;
        } else {
            self.last_line_bytes += other.bytes;
        }
    }
}

/// Byte offsets are the rope's primary dimension.
impl Dimension<TextSummary> for usize {
    fn add_summary(&mut self, summary: &TextSummary) {
        *self += summary.bytes;
    }
}

impl Dimension<TextSummary> for Point {
    fn add_summary(&mut self, summary: &TextSummary) {
        if summary.lines > 0 {
            self.row += summary.lines;
            self.column = summary.last_line_bytes;
        } else {
            self.column += summary.bytes;
        }
    }
}

/// One leaf item of a rope: a non-empty piece of text.
#[derive(Clone, Debug)]
pub(crate) struct Chunk(Arc<str>);

impl Chunk {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn text(&self) -> &Arc<str> {
        &self.0
    }
}

impl Item for Chunk {
    type Summary = TextSummary;

    fn summary(&self) -> TextSummary {
        TextSummary::from_text(&self.0)
    }
}

/// What a rope edit replaced, for display layers to follow (ADR 0035).
///
/// Both ranges start and end on chunk boundaries: the chunks in `old` were
/// replaced by the chunks in `new`, and every other chunk is unchanged. The
/// ranges cover at least the edited text, and usually a little more.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RopeEdit {
    /// The replaced byte range, in the rope before the edit.
    pub old: Range<usize>,
    /// The replacing byte range, in the rope after the edit.
    pub new: Range<usize>,
}

/// UTF-8 text in a [`SumTree`]. See the module docs.
#[derive(Clone, Default)]
pub struct Rope {
    tree: SumTree<Chunk>,
}

impl From<&str> for Rope {
    /// Builds a balanced rope in O(n).
    fn from(text: &str) -> Self {
        Self {
            tree: SplitChunks::new(text).map(chunk).collect(),
        }
    }
}

impl fmt::Display for Rope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.chunks().try_for_each(|text| f.write_str(text))
    }
}

impl fmt::Debug for Rope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.chunks()).finish()
    }
}

fn chunk(text: &str) -> Chunk {
    Chunk(Arc::from(text))
}

impl Rope {
    /// An empty rope.
    pub fn new() -> Self {
        Self::default()
    }

    /// The summary of the whole text.
    pub fn summary(&self) -> &TextSummary {
        self.tree.summary()
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.summary().bytes
    }

    /// Whether the rope has no text.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Length in Unicode scalar values.
    pub fn char_count(&self) -> usize {
        self.summary().chars
    }

    /// Number of lines: line breaks plus one. A text that ends with a line
    /// break has an empty last line, and an empty rope has one line.
    pub fn line_count(&self) -> usize {
        self.summary().lines + 1
    }

    /// The point at the end of the text.
    pub fn max_point(&self) -> Point {
        Point::new(self.summary().lines, self.summary().last_line_bytes)
    }

    #[cfg(test)]
    pub(crate) fn tree(&self) -> &SumTree<Chunk> {
        &self.tree
    }

    /// The shared texts of the whole chunks in `range`, which starts and
    /// ends on chunk boundaries (see [`RopeEdit`]). For display layers that
    /// mirror the rope's chunks.
    pub(crate) fn chunk_texts(&self, range: Range<usize>) -> impl Iterator<Item = &Arc<str>> + '_ {
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&range.start, Bias::Right);
        std::iter::from_fn(move || {
            if range.is_empty() || *cursor.start() >= range.end {
                return None;
            }
            let text = cursor.item()?.text();
            cursor.next();
            Some(text)
        })
    }

    /// Clamps `offset` to the text and moves it out of a UTF-8 character:
    /// back to its start with `Bias::Left`, forward to its end with
    /// `Bias::Right`.
    pub fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        if offset >= self.len() {
            return self.len();
        }
        let (start, text) = self.chunk_at(offset);
        let mut index = offset - start;
        while !text.is_char_boundary(index) {
            match bias {
                Bias::Left => index -= 1,
                Bias::Right => index += 1,
            }
        }
        start + index
    }

    /// The chunk that contains `offset` and its start offset. `offset` must
    /// be less than the length.
    fn chunk_at(&self, offset: usize) -> (usize, &str) {
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        match cursor.item() {
            Some(chunk) => (*cursor.start(), chunk.as_str()),
            None => (self.len(), ""),
        }
    }

    /// Converts a byte offset to a line and byte column. O(log n).
    pub fn offset_to_point(&self, offset: usize) -> Point {
        let offset = self.clip_offset(offset, Bias::Left);
        let mut cursor = self.tree.cursor::<(usize, Point)>();
        cursor.seek(&offset, Bias::Right);
        let (start, mut point) = *cursor.start();
        if let Some(chunk) = cursor.item() {
            let before = &chunk.as_str().as_bytes()[..offset - start];
            match before.iter().rposition(|&byte| byte == b'\n') {
                Some(newline) => {
                    point.row += before.iter().filter(|&&byte| byte == b'\n').count();
                    point.column = before.len() - newline - 1;
                }
                None => point.column += before.len(),
            }
        }
        point
    }

    /// Converts a line and byte column to a byte offset. O(log n).
    ///
    /// A row past the last line gives the end of the text. A column past the
    /// end of its line gives the end of the line, before the line break; a
    /// column inside a character gives the start of the character.
    pub fn point_to_offset(&self, point: Point) -> usize {
        if point.row > self.summary().lines {
            return self.len();
        }
        let line_start = self.line_start(point.row);
        let line_end = self.line_end(point.row);
        let offset = line_start.saturating_add(point.column).min(line_end);
        self.clip_offset(offset, Bias::Left)
    }

    /// Clamps `point` to the text, as [`Rope::point_to_offset`] does.
    pub fn clip_point(&self, point: Point) -> Point {
        self.offset_to_point(self.point_to_offset(point))
    }

    /// The offset where line `row` starts. A row past the last line gives
    /// the end of the text.
    pub fn line_start(&self, row: usize) -> usize {
        if row == 0 {
            return 0;
        }
        if row > self.summary().lines {
            return self.len();
        }
        // The line starts right after the `\n` that ends line `row - 1`.
        let mut cursor = self.tree.cursor::<(Point, usize)>();
        cursor.seek(&Point::new(row, 0), Bias::Right);
        let (start, offset) = *cursor.start();
        if start.row == row {
            return offset;
        }
        let Some(chunk) = cursor.item() else {
            return self.len();
        };
        let text = chunk.as_str();
        let breaks = row - start.row;
        let index = text
            .match_indices('\n')
            .nth(breaks - 1)
            .map_or(text.len(), |(index, _)| index + 1);
        offset + index
    }

    /// The offset where line `row` ends, before its line break. A row past
    /// the last line gives the end of the text.
    pub fn line_end(&self, row: usize) -> usize {
        if row >= self.summary().lines {
            return self.len();
        }
        let newline = self.line_start(row + 1) - 1;
        if newline > 0 && self.byte_at(newline - 1) == Some(b'\r') {
            newline - 1
        } else {
            newline
        }
    }

    /// The length of line `row` in bytes, without its line break.
    pub fn line_len(&self, row: usize) -> usize {
        self.line_end(row) - self.line_start(row)
    }

    fn byte_at(&self, offset: usize) -> Option<u8> {
        if offset >= self.len() {
            return None;
        }
        let (start, text) = self.chunk_at(offset);
        text.as_bytes().get(offset - start).copied()
    }

    /// The text in `range`, as a rope. O(log n).
    pub fn slice(&self, range: Range<usize>) -> Rope {
        let (start, end) = self.clip_range(range);
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&start, Bias::Right);
        let Some(first) = cursor.item() else {
            return Rope::new();
        };
        let (first_start, first_end) = (*cursor.start(), cursor.end());
        if end <= first_end {
            return Rope::from(&first.as_str()[start - first_start..end - first_start]);
        }
        let mut tree = SumTree::from_item(chunk(&first.as_str()[start - first_start..]));
        // The whole chunks in between, then the part of the last one.
        let (_, rest) = self.tree.split(&first_end, Bias::Right);
        let (whole, last) = rest.split(&(end - first_end), Bias::Right);
        let whole_end = first_end + whole.summary().bytes;
        tree.append(whole);
        if let Some(last) = last.iter().next() {
            let tail = &last.as_str()[..end - whole_end];
            if !tail.is_empty() {
                tree.push(chunk(tail));
            }
        }
        Rope { tree }
    }

    /// Clamps both ends to the text and to character starts; a reversed
    /// range becomes empty at its start.
    fn clip_range(&self, range: Range<usize>) -> (usize, usize) {
        let start = self.clip_offset(range.start, Bias::Left);
        let end = self.clip_offset(range.end, Bias::Left).max(start);
        (start, end)
    }

    /// Iterates over the text in chunks.
    pub fn chunks(&self) -> Chunks<'_> {
        self.chunks_in_range(0..self.len())
    }

    /// Iterates over the text in `range` in chunks. The first and last
    /// pieces may be parts of chunks.
    pub fn chunks_in_range(&self, range: Range<usize>) -> Chunks<'_> {
        let (start, end) = self.clip_range(range);
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&start, Bias::Right);
        Chunks {
            cursor,
            range: start..end,
        }
    }

    /// Iterates over the lines, without their line breaks. A line within
    /// one chunk is borrowed; a longer one is copied into a `String`, so
    /// rendering and other per-frame code use `chunks_in_range` or the
    /// display map instead, which never copy a long line.
    pub fn lines(&self) -> Lines<'_> {
        Lines {
            chunks: self.chunks(),
            rest: "",
            done: false,
        }
    }

    /// The chunks before `offset`, nearest first. `offset` must be a chunk
    /// boundary.
    fn chunks_before(&self, offset: usize) -> impl Iterator<Item = &str> + '_ {
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&offset, Bias::Right);
        std::iter::from_fn(move || {
            if *cursor.start() == 0 || !cursor.prev() {
                return None;
            }
            cursor.item().map(Chunk::as_str)
        })
    }

    /// Inserts `text` at `offset`. O(log n + text length).
    pub fn insert(&mut self, offset: usize, text: &str) -> RopeEdit {
        self.replace(offset..offset, text)
    }

    /// Deletes the text in `range`. O(log n).
    pub fn delete(&mut self, range: Range<usize>) -> RopeEdit {
        self.replace(range, "")
    }

    /// Appends `text`, for example the next piece of a file being loaded.
    pub fn push_str(&mut self, text: &str) -> RopeEdit {
        let len = self.len();
        self.replace(len..len, text)
    }

    /// Replaces the text in `range` with `text`. O(log n + text length).
    ///
    /// The range is clamped as in [`Rope::slice`]. The returned edit gives
    /// the replaced chunks, for display layers to follow.
    #[expect(
        clippy::too_many_lines,
        reason = "predates the 80-line limit (ADR 0038); split it when it next changes"
    )]
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> RopeEdit {
        let (start, end) = self.clip_range(range);
        if start == end && text.is_empty() {
            return RopeEdit {
                old: start..start,
                new: start..start,
            };
        }

        // Rebuild the chunks that the edit touches: from the chunk that ends
        // at or contains `start` to the chunk that contains `end`.
        let mut cursor = self.tree.cursor::<usize>();
        cursor.seek(&start, Bias::Left);
        let mut window_start = *cursor.start();
        cursor.seek(&end, Bias::Right);
        let mut window_end = cursor.end();

        let mut core = String::with_capacity(window_end - window_start + text.len());
        core.extend(self.chunks_in_range(window_start..start));
        core.push_str(text);
        core.extend(self.chunks_in_range(end..window_end));

        // The edit may join grapheme clusters across the window's edges, for
        // example a combining mark inserted after a chunk that ends in its
        // base letter. Take in neighbors until both edges are boundaries:
        // first on the right, then on the left, so that an emptied window is
        // checked where its neighbors now meet.
        let mut suffix: Vec<&str> = Vec::new();
        for _ in 0..MAX_BOUNDARY_CHUNKS {
            let mut cursor = self.tree.cursor::<usize>();
            cursor.seek(&window_end, Bias::Right);
            let Some(next) = cursor.item().map(Chunk::as_str) else {
                break;
            };
            let edge =
                window_start + core.len() + suffix.iter().map(|text| text.len()).sum::<usize>();
            let before = suffix
                .iter()
                .rev()
                .copied()
                .chain(std::iter::once(core.as_str()))
                .chain(self.chunks_before(window_start));
            if is_grapheme_boundary(edge, next, before) {
                break;
            }
            suffix.push(next);
            window_end += next.len();
        }
        let mut prefix: Vec<&str> = Vec::new();
        for _ in 0..MAX_BOUNDARY_CHUNKS {
            let after = prefix
                .last()
                .copied()
                .into_iter()
                .chain(std::iter::once(core.as_str()))
                .chain(suffix.iter().copied())
                .find(|text| !text.is_empty())
                .unwrap_or_default();
            let mut before = self.chunks_before(window_start);
            let Some(previous) = before.next() else {
                break;
            };
            if is_grapheme_boundary(window_start, after, std::iter::once(previous).chain(before)) {
                break;
            }
            prefix.push(previous);
            window_start -= previous.len();
        }
        // Keep chunks from shrinking under repeated small edits.
        let len = prefix
            .iter()
            .chain(&suffix)
            .map(|text| text.len())
            .sum::<usize>()
            + core.len();
        if len < MAX_CHUNK_BYTES / 2 {
            let mut cursor = self.tree.cursor::<usize>();
            cursor.seek(&window_end, Bias::Right);
            if let Some(next) = cursor.item() {
                suffix.push(next.as_str());
                window_end += next.as_str().len();
            } else if let Some(previous) = self.chunks_before(window_start).next() {
                prefix.push(previous);
                window_start -= previous.len();
            }
        }

        let mut middle = String::with_capacity(len + MAX_CHUNK_BYTES);
        middle.extend(prefix.iter().rev().copied());
        middle.push_str(&core);
        middle.extend(suffix.iter().copied());

        let (mut tree, _) = self.tree.split(&window_start, Bias::Right);
        let (_, right) = self.tree.split(&window_end, Bias::Right);
        tree.append(SplitChunks::new(&middle).map(chunk).collect());
        tree.append(right);
        self.tree = tree;
        RopeEdit {
            old: window_start..window_end,
            new: window_start..window_start + middle.len(),
        }
    }

    /// Checks the chunk invariants and the tree's. Test-only.
    #[cfg(test)]
    pub(crate) fn check_invariants(&self) {
        self.tree.check_invariants();
        let text = self.to_string();
        // Clusters longer than a chunk are split on purpose.
        let check_clusters = !tests::has_long_cluster(&text);
        let mut offset = 0;
        for chunk in self.tree.iter() {
            let piece = chunk.as_str();
            assert!(!piece.is_empty(), "empty chunk");
            assert!(piece.len() <= MAX_CHUNK_BYTES, "oversized chunk");
            offset += piece.len();
            if !check_clusters {
                continue;
            }
            let mut cursor = GraphemeCursor::new(offset, text.len(), true);
            assert_eq!(
                cursor.is_boundary(&text, 0),
                Ok(true),
                "chunk boundary inside a grapheme cluster at {offset} in {text:?}",
            );
        }
    }
}

/// Whether `offset` is a grapheme cluster boundary in the text formed by
/// the chunks in `before` (nearest first, ending at `offset`) followed by
/// `after`. Looks back at most `MAX_BOUNDARY_CHUNKS` chunks, and treats a
/// longer context as a boundary.
fn is_grapheme_boundary<'a>(
    offset: usize,
    after: &str,
    before: impl Iterator<Item = &'a str>,
) -> bool {
    if offset == 0 || after.is_empty() {
        return true;
    }
    let mut cursor = GraphemeCursor::new(offset, offset + after.len(), true);
    let mut context_start = offset;
    let mut before = before
        .filter(|text| !text.is_empty())
        .take(MAX_BOUNDARY_CHUNKS);
    loop {
        match cursor.is_boundary(after, offset) {
            Ok(is_boundary) => return is_boundary,
            Err(GraphemeIncomplete::PreContext(end)) if end == context_start => {
                let Some(previous) = before.next() else {
                    return true;
                };
                let Some(start) = context_start.checked_sub(previous.len()) else {
                    return true;
                };
                cursor.provide_context(previous, start);
                context_start = start;
            }
            Err(_) => return true,
        }
    }
}

/// Splits text into chunks of at most `MAX_CHUNK_BYTES`, as even in size as
/// grapheme cluster boundaries allow.
struct SplitChunks<'a> {
    text: &'a str,
    position: usize,
    target: usize,
}

impl<'a> SplitChunks<'a> {
    fn new(text: &'a str) -> Self {
        let count = text.len().div_ceil(MAX_CHUNK_BYTES).max(1);
        Self {
            text,
            position: 0,
            target: text.len().div_ceil(count),
        }
    }
}

impl<'a> Iterator for SplitChunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let (text, start) = (self.text, self.position);
        if start >= text.len() {
            return None;
        }
        let end = if text.len() - start <= MAX_CHUNK_BYTES {
            text.len()
        } else {
            chunk_end(text, start, self.target)
        };
        self.position = end;
        Some(&text[start..end])
    }
}

/// Where the chunk that starts at `start` ends: the last grapheme cluster
/// boundary at most `target` bytes on, or failing that the first one within
/// `MAX_CHUNK_BYTES`, or failing that a character boundary.
fn chunk_end(text: &str, start: usize, target: usize) -> usize {
    let limit = text.floor_char_boundary(start + MAX_CHUNK_BYTES);
    let want = text.floor_char_boundary(start + target.min(MAX_CHUNK_BYTES));
    // Look only at a window around the chunk, so that a long cluster or run
    // of flags costs O(chunk) per chunk, not O(text). A boundary the window
    // cannot decide counts as none.
    let from = text.floor_char_boundary(start.saturating_sub(MAX_CHUNK_BYTES));
    let to = text.ceil_char_boundary((limit + 4).min(text.len()));
    let window = &text[from..to];
    let at = |offset| GraphemeCursor::new(offset, text.len(), true);
    if at(want).is_boundary(window, from) == Ok(true) {
        return want;
    }
    if let Ok(Some(previous)) = at(want).prev_boundary(window, from)
        && previous > start
    {
        return previous;
    }
    if let Ok(Some(next)) = at(want).next_boundary(window, from)
        && next <= limit
    {
        return next;
    }
    // One cluster longer than a chunk: split it between characters.
    limit
}

/// Iterates over a rope's text in chunks. See [`Rope::chunks_in_range`].
pub struct Chunks<'a> {
    cursor: Cursor<'a, Chunk, usize>,
    range: Range<usize>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.range.is_empty() {
            return None;
        }
        let chunk = self.cursor.item()?;
        let start = *self.cursor.start();
        let text = chunk.as_str();
        let from = self.range.start - start;
        let to = (self.range.end - start).min(text.len());
        self.range.start = start + to;
        self.cursor.next();
        Some(&text[from..to])
    }
}

/// Iterates over a rope's lines. See [`Rope::lines`].
pub struct Lines<'a> {
    chunks: Chunks<'a>,
    /// The unread part of the current chunk.
    rest: &'a str,
    done: bool,
}

impl<'a> Iterator for Lines<'a> {
    type Item = Cow<'a, str>;

    fn next(&mut self) -> Option<Cow<'a, str>> {
        if self.done {
            return None;
        }
        let mut long: Option<String> = None;
        loop {
            if let Some(index) = self.rest.find('\n') {
                let piece = &self.rest[..index];
                self.rest = &self.rest[index + 1..];
                let line = match long {
                    None => Cow::Borrowed(piece),
                    Some(mut line) => {
                        line.push_str(piece);
                        Cow::Owned(line)
                    }
                };
                return Some(strip_cr(line));
            }
            if !self.rest.is_empty() {
                long.get_or_insert_with(String::new).push_str(self.rest);
            }
            match self.chunks.next() {
                Some(text) => self.rest = text,
                None => {
                    self.done = true;
                    return Some(long.map_or(Cow::Borrowed(""), Cow::Owned));
                }
            }
        }
    }
}

fn strip_cr(line: Cow<'_, str>) -> Cow<'_, str> {
    match line {
        Cow::Borrowed(text) => Cow::Borrowed(text.strip_suffix('\r').unwrap_or(text)),
        Cow::Owned(mut text) => {
            if text.ends_with('\r') {
                text.pop();
            }
            Cow::Owned(text)
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Pieces of text that random tests are built from: ASCII, Japanese,
    /// emoji sequences, combining marks, tabs and every line ending.
    pub(crate) const ATOMS: &[&str] = &[
        "a",
        "Z",
        " ",
        "\t",
        "\n",
        "\r\n",
        "\r",
        "日本語",
        "の",
        "ｱ",
        "é",
        "e\u{301}",
        "\u{301}",
        "👍",
        "👍🏽",
        "👨\u{200d}👩\u{200d}👧",
        "❤\u{fe0f}",
        "🇯🇵",
        "\u{200b}",
        "\u{200c}",
        "\u{202e}",
        "\u{e0041}",
        "\u{7f}",
        "\u{85}",
        "\0",
    ];

    pub(crate) fn text_strategy(max_atoms: usize) -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(ATOMS), 0..max_atoms)
            .prop_map(|atoms| atoms.concat())
    }

    /// Whether `text` has a grapheme cluster longer than a chunk.
    pub(crate) fn has_long_cluster(text: &str) -> bool {
        use unicode_segmentation::UnicodeSegmentation;
        text.graphemes(true)
            .any(|cluster| cluster.len() > MAX_CHUNK_BYTES)
    }

    /// The largest char boundary at or before `offset`, clamped to `text`.
    pub(crate) fn floor_boundary(text: &str, offset: usize) -> usize {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    /// Line starts by a naive scan.
    fn naive_line_starts(text: &str) -> Vec<usize> {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        starts
    }

    fn naive_point(text: &str, offset: usize) -> Point {
        let before = &text[..offset];
        let row = before.matches('\n').count();
        let column = before
            .rfind('\n')
            .map_or(offset, |index| offset - index - 1);
        Point::new(row, column)
    }

    /// Lines without their breaks. Only a `\r` before `\n` is part of one.
    fn naive_lines(text: &str) -> Vec<String> {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let last = lines.len() - 1;
        for line in &mut lines[..last] {
            if line.ends_with('\r') {
                line.pop();
            }
        }
        lines
    }

    fn check_against(rope: &Rope, text: &str) {
        rope.check_invariants();
        assert_eq!(rope.to_string(), text);
        assert_eq!(rope.len(), text.len());
        assert_eq!(rope.char_count(), text.chars().count());
        let starts = naive_line_starts(text);
        assert_eq!(rope.line_count(), starts.len());
        let lines = naive_lines(text);
        let rope_lines: Vec<String> = rope.lines().map(Cow::into_owned).collect();
        assert_eq!(rope_lines, lines);
        for (row, start) in starts.iter().enumerate() {
            assert_eq!(rope.line_start(row), *start, "line_start({row})");
            assert_eq!(rope.line_len(row), lines[row].len(), "line_len({row})");
        }
        assert_eq!(rope.line_start(starts.len()), text.len());
        for offset in 0..=text.len() {
            let clipped = floor_boundary(text, offset);
            let point = naive_point(text, clipped);
            assert_eq!(rope.offset_to_point(offset), point, "offset {offset}");
            assert_eq!(rope.clip_offset(offset, Bias::Left), clipped);
            // Points inside a `\r\n` pair clamp to the line end before it.
            let inside_crlf = text[..clipped].ends_with('\r') && text[clipped..].starts_with('\n');
            if text.is_char_boundary(offset) && !inside_crlf {
                assert_eq!(rope.point_to_offset(point), offset, "point {point:?}");
            }
        }
        assert_eq!(rope.max_point(), naive_point(text, text.len()));
    }

    #[test]
    fn empty_rope() {
        let rope = Rope::new();
        check_against(&rope, "");
        assert!(rope.is_empty());
        assert_eq!(rope.chunks().count(), 0);
        assert_eq!(rope.line_end(0), 0);
        assert_eq!(rope.point_to_offset(Point::new(5, 5)), 0);
        assert_eq!(rope.offset_to_point(10), Point::new(0, 0));
        assert_eq!(rope.slice(0..10).to_string(), "");
        let mut rope = rope;
        rope.delete(0..10);
        check_against(&rope, "");
    }

    #[test]
    fn line_endings() {
        let text = "a\r\nb\rc\n\nd\r";
        let rope = Rope::from(text);
        check_against(&rope, text);
        assert_eq!(rope.line_count(), 4);
        assert_eq!(rope.line_end(0), 1, "before the CRLF");
        assert_eq!(rope.line_len(1), 3, "a lone CR stays in the line");
        assert_eq!(rope.line_len(3), 2, "a trailing CR is no line break");
    }

    #[test]
    fn out_of_range_inputs_are_clamped() {
        let text = "日本\nab";
        let rope = Rope::from(text);
        assert_eq!(rope.clip_offset(1, Bias::Left), 0);
        assert_eq!(rope.clip_offset(1, Bias::Right), 3);
        assert_eq!(rope.clip_offset(99, Bias::Right), text.len());
        assert_eq!(rope.point_to_offset(Point::new(0, 99)), 6);
        assert_eq!(rope.point_to_offset(Point::new(0, 4)), 3);
        assert_eq!(rope.point_to_offset(Point::new(9, 0)), text.len());
        assert_eq!(rope.clip_point(Point::new(1, 99)), Point::new(1, 2));
        let (start, end) = (5, 2);
        assert_eq!(rope.slice(start..end).to_string(), "", "a reversed range");
        assert_eq!(rope.slice(4..usize::MAX).to_string(), "本\nab");
        assert_eq!(rope.chunks_in_range(7..99).collect::<String>(), "ab");
    }

    #[test]
    fn chunks_are_bounded_and_end_on_clusters() {
        let text = "x".repeat(MAX_CHUNK_BYTES - 1) + "e\u{301}\u{301}" + &"y".repeat(100);
        let rope = Rope::from(text.as_str());
        check_against(&rope, &text);
        assert!(rope.chunks().all(|chunk| chunk.len() <= MAX_CHUNK_BYTES));
    }

    #[test]
    fn crlf_stays_in_one_chunk() {
        for prefix in 0..MAX_CHUNK_BYTES + 2 {
            let text = "a".repeat(prefix) + "\r\n" + &"b".repeat(MAX_CHUNK_BYTES);
            let rope = Rope::from(text.as_str());
            assert!(rope.chunks().all(|chunk| !chunk.ends_with('\r')));
            check_against(&rope, &text);
        }
    }

    #[test]
    fn line_exactly_at_a_chunk_boundary() {
        let line = "a".repeat(MAX_CHUNK_BYTES - 1) + "\n";
        let text = line.repeat(4);
        let rope = Rope::from(text.as_str());
        assert!(rope.chunks().all(|chunk| chunk.ends_with('\n')));
        check_against(&rope, &text);
    }

    #[test]
    fn a_cluster_longer_than_a_chunk_is_split_between_chars() {
        let text = "e".to_owned() + &"\u{301}".repeat(MAX_CHUNK_BYTES * 2);
        let rope = Rope::from(text.as_str());
        assert_eq!(rope.to_string(), text);
        assert!(rope.chunks().all(|chunk| chunk.len() <= MAX_CHUNK_BYTES));
        rope.tree().check_invariants();
    }

    #[test]
    fn combining_mark_inserted_at_a_chunk_boundary_joins_the_cluster() {
        let text = "a".repeat(MAX_CHUNK_BYTES) + "b";
        let mut rope = Rope::from(text.as_str());
        let mut model = text.clone();
        rope.insert(MAX_CHUNK_BYTES, "\u{301}");
        model.insert(MAX_CHUNK_BYTES, '\u{301}');
        check_against(&rope, &model);
    }

    #[test]
    fn flags_keep_their_pairs_across_edits() {
        let mut model = "🇯🇵".repeat(20);
        let mut rope = Rope::from(model.as_str());
        // Deleting one regional indicator re-pairs every later one.
        rope.delete(0..4);
        model.replace_range(0..4, "");
        check_against(&rope, &model);
    }

    #[test]
    fn adversarial_text_builds_in_linear_time() {
        // Long runs of flags and one huge cluster used to rescan the whole
        // text for every chunk. Debug test builds are slow, so the bound is
        // loose; quadratic behavior would take minutes.
        let started = std::time::Instant::now();
        for text in [
            "🇯🇵".repeat(64 * 1024),
            "e".to_owned() + &"\u{301}".repeat(128 * 1024),
            "a".to_owned() + &"\u{200d}👍".repeat(32 * 1024),
        ] {
            let rope = Rope::from(text.as_str());
            assert_eq!(rope.len(), text.len());
            assert!(rope.chunks().all(|chunk| chunk.len() <= MAX_CHUNK_BYTES));
            let mut pasted = Rope::from("xy");
            pasted.insert(1, &text);
            assert_eq!(pasted.len(), text.len() + 2);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }

    #[test]
    fn small_edits_do_not_fragment_chunks() {
        let text = "x".repeat(MAX_CHUNK_BYTES * 20);
        let mut rope = Rope::from(text.as_str());
        for _ in 0..text.len() - MAX_CHUNK_BYTES * 2 {
            rope.delete(5..6);
        }
        rope.check_invariants();
        assert!(rope.chunks().count() <= 4);
    }

    #[test]
    fn long_line() {
        let text = "ab\t日本".repeat(5000);
        let rope = Rope::from(text.as_str());
        assert_eq!(rope.line_count(), 1);
        assert_eq!(rope.offset_to_point(text.len()), Point::new(0, text.len()));
        assert_eq!(rope.point_to_offset(Point::new(0, 30_001)), 30_000);
        assert_eq!(rope.line_end(0), text.len());
    }

    #[test]
    fn edits_report_chunk_aligned_windows() {
        let text = "0123456789".repeat(20);
        let mut rope = Rope::from(text.as_str());
        let before = rope.clone();
        let edit = rope.replace(55..60, "XY");
        let boundaries = |rope: &Rope| {
            let mut offsets = vec![0];
            for chunk in rope.chunks() {
                offsets.push(offsets.last().copied().unwrap_or(0) + chunk.len());
            }
            offsets
        };
        assert!(boundaries(&before).contains(&edit.old.start));
        assert!(boundaries(&before).contains(&edit.old.end));
        assert!(boundaries(&rope).contains(&edit.new.start));
        assert!(boundaries(&rope).contains(&edit.new.end));
        assert!(edit.old.start <= 55 && edit.old.end >= 60);
        assert_eq!(edit.new.len() as isize - edit.old.len() as isize, -3);
        assert_eq!(before.to_string(), text, "snapshots are unchanged");
    }

    #[test]
    fn progressive_appends_match_building_at_once() {
        let text = "行1\r\nline two\tend\n".repeat(40);
        let mut rope = Rope::new();
        let bytes = text.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            let end = floor_boundary(&text, start + 7);
            rope.push_str(&text[start..end]);
            start = end;
        }
        check_against(&rope, &text);
    }

    proptest! {
        #[test]
        fn building_matches_naive_scans(text in text_strategy(80)) {
            check_against(&Rope::from(text.as_str()), &text);
        }

        #[test]
        fn edits_match_a_string(
            text in text_strategy(60),
            edits in prop::collection::vec((0usize..400, 0usize..400, text_strategy(12)), 1..12),
        ) {
            let mut rope = Rope::from(text.as_str());
            let mut model = text;
            for (a, b, insert) in edits {
                let start = floor_boundary(&model, a.min(b));
                let end = floor_boundary(&model, a.max(b));
                let snapshot = rope.clone();
                let snapshot_text = model.clone();
                rope.replace(start..end, &insert);
                model.replace_range(start..end, &insert);
                check_against(&rope, &model);
                prop_assert_eq!(snapshot.to_string(), snapshot_text);
            }
        }

        #[test]
        fn slices_match_a_string(text in text_strategy(80), a in 0usize..400, b in 0usize..400) {
            let rope = Rope::from(text.as_str());
            let start = floor_boundary(&text, a.min(b));
            let end = floor_boundary(&text, a.max(b));
            let slice = rope.slice(start..end);
            slice.tree().check_invariants();
            prop_assert_eq!(slice.to_string(), &text[start..end]);
            prop_assert_eq!(slice.summary(), &TextSummary::from_text(&text[start..end]));
            prop_assert_eq!(rope.chunks_in_range(start..end).collect::<String>(), &text[start..end]);
        }

        #[test]
        fn appending_in_pieces_matches_building_at_once(
            pieces in prop::collection::vec(text_strategy(10), 0..20),
        ) {
            let mut rope = Rope::new();
            for piece in &pieces {
                rope.push_str(piece);
            }
            check_against(&rope, &pieces.concat());
        }
    }
}
