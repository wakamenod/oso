//! What query-replace looks for (ADR 0113): a literal string with isearch's matcher, or a
//! regexp matched one line at a time.
//!
//! Must not depend on: `buffer`, `editor` or a log.

use std::ops::Range;

use super::regexp::{LineTooLong, Regexp};
use super::{Found, Query, find_in, matches_in};
use crate::decode::InvalidBytes;
use crate::rope::Rope;

/// The longest line a frame reads for a regexp's highlights.
const LONGEST_SHOWN: usize = 64 * 1024;

/// A literal query or a regexp.
#[derive(Clone, Debug)]
pub(crate) enum Needle {
    Literal(Query),
    Regexp(Regexp),
}

impl Needle {
    /// The first match within `range`, reading at most about `limit` bytes, and none past
    /// `stop`.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp at a line too long to match in.
    pub(crate) fn find_in(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        range: Range<usize>,
        limit: usize,
        stop: &dyn Fn() -> bool,
    ) -> Result<Found, LineTooLong> {
        match self {
            Self::Literal(query) => Ok(find_in(text, invalid, query, range, limit, stop)),
            Self::Regexp(regexp) => regexp.find_in(text, range, limit, stop),
        }
    }

    /// How far a match can reach past a view row, for the highlights: a literal query's
    /// longest match, and nothing for a regexp, whose lines are read whole.
    pub(crate) fn reach(&self) -> usize {
        match self {
            Self::Literal(query) => query.max_bytes(),
            Self::Regexp(_) => 0,
        }
    }

    /// The matches within `spans`, at most `most` and reading at most `limit` bytes, for a
    /// frame's highlights.
    pub(crate) fn matches_in(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        spans: &[Range<usize>],
        (most, limit): (usize, usize),
        out: &mut Vec<Range<usize>>,
    ) {
        match self {
            Self::Literal(query) => matches_in((text, invalid), query, spans, (most, limit), out),
            Self::Regexp(regexp) => {
                regexp.matches_in(text, spans, (most, limit, LONGEST_SHOWN), out);
            }
        }
    }

    /// The replacement for the match at `range`: `to`, with a regexp's groups expanded.
    pub(crate) fn replacement(&self, text: &Rope, range: &Range<usize>, to: &str) -> String {
        match self {
            Self::Literal(_) => to.to_owned(),
            Self::Regexp(regexp) => regexp.expand(text, range, to),
        }
    }
}
