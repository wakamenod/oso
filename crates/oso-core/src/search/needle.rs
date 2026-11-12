//! What query-replace looks for (ADR 0113): a literal string with isearch's matcher, or a
//! regexp matched one line at a time, and a search through a scope's ranges, match after
//! match, within a budget of work.
//!
//! Must not depend on: `buffer`, `editor` or a log.

use std::ops::Range;

use super::regexp::{self, Line, LineTooLong, Regexp};
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

/// A search for a needle's matches through ranges, ascending and apart.
pub(crate) enum Scan<'a> {
    Literal {
        query: &'a Query,
        snapshot: (&'a Rope, &'a InvalidBytes),
        ranges: &'a [Range<usize>],
        range: usize,
        at: usize,
        work: usize,
        limit: usize,
    },
    Regexp(regexp::Scan<'a>),
}

impl Needle {
    /// A search through `ranges` of `snapshot` from `from`, an empty match there skipped if a
    /// match ended there, within `limit` of work. `line` is a line's text the caller kept,
    /// for a regexp.
    pub(crate) fn scan<'a>(
        &'a self,
        snapshot: (&'a Rope, &'a InvalidBytes),
        ranges: &'a [Range<usize>],
        from: (usize, bool),
        limit: usize,
        line: Option<Line>,
    ) -> Scan<'a> {
        match self {
            Self::Literal(query) => Scan::Literal {
                query,
                snapshot,
                ranges,
                range: ranges.partition_point(|range| range.end <= from.0),
                at: from.0,
                work: 0,
                limit,
            },
            Self::Regexp(regexp) => Scan::Regexp(regexp::Scan::new(
                regexp, snapshot.0, ranges, from, limit, line,
            )),
        }
    }

    /// How far a match can reach past a view row, for the highlights: a literal query's
    /// longest match, and nothing for a regexp, which reads around the view itself.
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
            Self::Regexp(regexp) => regexp.matches_in(text, spans, (most, LONGEST_SHOWN), out),
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

impl Scan<'_> {
    /// The next match: [`Found::Match`], [`Found::None`] past the last range, or
    /// [`Found::Stopped`] where it would go on, past its limit or as `stop` says.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn next(&mut self, stop: &dyn Fn() -> bool) -> Result<Found, LineTooLong> {
        let Self::Literal {
            query,
            snapshot: (text, invalid),
            ranges,
            range,
            at,
            work,
            limit,
        } = self
        else {
            let Self::Regexp(scan) = self else {
                return Ok(Found::None);
            };
            return scan.next(stop);
        };
        while let Some(scope) = ranges.get(*range) {
            *at = (*at).max(scope.start);
            if *at >= scope.end {
                *range += 1;
                continue;
            }
            let left = limit.saturating_sub(*work);
            match find_in(text, invalid, query, *at..scope.end, left, stop) {
                Found::Match(found) => {
                    *work += found.end - *at;
                    *at = found.end;
                    return Ok(Found::Match(found));
                }
                Found::None => {
                    *work += scope.end - *at;
                    *range += 1;
                }
                Found::Stopped { resume } => {
                    *at = resume;
                    return Ok(Found::Stopped { resume });
                }
            }
            if *work >= *limit {
                return Ok(Found::Stopped { resume: *at });
            }
        }
        Ok(Found::None)
    }

    /// Stops at `until` too, for a search on the UI thread. A literal query's reads only
    /// the text it spans, so its budget alone bounds it.
    pub(crate) fn until(&mut self, until: std::time::Instant) {
        if let Self::Regexp(scan) = self {
            scan.until(until);
        }
    }

    /// Where it would go on, and whether a match ended there.
    pub(crate) fn place(&self) -> (usize, bool) {
        match self {
            Self::Literal { at, .. } => (*at, false),
            Self::Regexp(scan) => scan.place(),
        }
    }

    /// The line a regexp's search is in, for the caller to keep.
    pub(crate) fn into_line(self) -> Option<Line> {
        match self {
            Self::Literal { .. } => None,
            Self::Regexp(scan) => scan.into_line(),
        }
    }
}
