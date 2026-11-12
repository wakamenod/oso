//! The shape of one reading of a file's diff (ADR 0122): the lines the diff takes out of the
//! old side and puts into the new, by number, and which old side it was read against. A
//! comment keeps the shape it was last placed in, and is carried from it to the next reading.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O. The host
//! diffs the sides on a worker and hands the hunks in as plain data, which [`Shape::new`]
//! checks.

/// A side of a diff: the text before the change, or after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Old,
    New,
}

/// What the old side of a reading was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// No file: the file was put in.
    Empty,
    /// A blob, by its object id's bytes, compared only for equality.
    Blob(Box<[u8]>),
    /// Not known: lines are then matched by their text and order alone.
    Unknown,
}

/// One hunk of a diff with no context lines. Line numbers count from 1. `removed` are the old
/// side's lines from `old_start`, `added` the new side's from `new_start`; a hunk that removes
/// nothing goes in front of old line `old_start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    pub removed: Vec<Box<[u8]>>,
    pub added: Vec<Box<[u8]>>,
}

/// Why [`Shape::new`] refused a list of hunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// This hunk removes nothing and adds nothing.
    Empty { hunk: usize },
    /// This hunk does not start after the one before it, with an old line kept between them.
    OutOfOrder { hunk: usize },
    /// This hunk's numbers do not follow from the hunks before it.
    Misnumbered { hunk: usize },
    /// A hunk reaches the last 32-bit line number.
    TooLong,
}

/// One reading of a file: its old side and its hunks, checked to be in order and numbered
/// consistently, so that carrying lines from it never meets a contradiction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shape {
    base: Base,
    hunks: Vec<Hunk>,
}

impl Shape {
    /// The shape of a reading against `base` whose hunks are `hunks`, in order. Between two
    /// hunks at least one old line is kept, as in any diff with no context; each new start is
    /// its old start moved by the lines the hunks before it put in and took out.
    pub fn new(base: Base, hunks: Vec<Hunk>) -> Result<Shape, ShapeError> {
        let mut shift: i64 = 0;
        let mut kept_from: i64 = 1;
        for (index, hunk) in hunks.iter().enumerate() {
            let removed = i64::try_from(hunk.removed.len()).map_err(|_| ShapeError::TooLong)?;
            let added = i64::try_from(hunk.added.len()).map_err(|_| ShapeError::TooLong)?;
            if removed == 0 && added == 0 {
                return Err(ShapeError::Empty { hunk: index });
            }
            let old = i64::from(hunk.old_start);
            if old < kept_from || (index > 0 && old == kept_from) {
                return Err(ShapeError::OutOfOrder { hunk: index });
            }
            if i64::from(hunk.new_start) != old + shift {
                return Err(ShapeError::Misnumbered { hunk: index });
            }
            // Below the last number, so a line's successor is always a number too.
            if old + removed >= i64::from(u32::MAX) || old + shift + added >= i64::from(u32::MAX) {
                return Err(ShapeError::TooLong);
            }
            kept_from = old + removed;
            shift += added - removed;
        }
        Ok(Shape { base, hunks })
    }

    /// The old side this reading was read against.
    pub fn base(&self) -> &Base {
        &self.base
    }

    /// The hunks, in order.
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// The text of line `number` of `side`, when the diff shows it: a line it takes out of the
    /// old side or puts into the new.
    pub fn line(&self, side: Side, number: u32) -> Option<&[u8]> {
        let index = self.hunk_at(side, number)?;
        let hunk = &self.hunks[index];
        let (start, lines) = side_of(hunk, side);
        lines.get(offset(number, start)?).map(|line| &**line)
    }

    /// The index of the hunk that shows line `number` of `side`.
    pub fn hunk_at(&self, side: Side, number: u32) -> Option<usize> {
        let after = self
            .hunks
            .partition_point(|hunk| side_of(hunk, side).0 <= number);
        let index = after.checked_sub(1)?;
        let (start, lines) = side_of(&self.hunks[index], side);
        (offset(number, start)? < lines.len()).then_some(index)
    }
}

/// Where `hunk`'s lines of `side` start, and the lines.
pub(super) fn side_of(hunk: &Hunk, side: Side) -> (u32, &[Box<[u8]>]) {
    match side {
        Side::Old => (hunk.old_start, &hunk.removed),
        Side::New => (hunk.new_start, &hunk.added),
    }
}

/// How far `number` is past `start`, when it is not before it.
fn offset(number: u32, start: u32) -> Option<usize> {
    usize::try_from(number.checked_sub(start)?).ok()
}
