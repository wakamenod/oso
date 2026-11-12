//! What query-replace and isearch look for (ADR 0113, ADR 0116): a literal string with
//! isearch's matcher, or a regexp matched one line at a time; a search through a scope's
//! ranges, match after match, within a budget of work; and isearch's searches forward,
//! backward and its count over the whole text.
//!
//! Must not depend on: `buffer`, `editor` or a log.

use std::ops::Range;
use std::time::{Duration, Instant};

use super::regexp::{self, Line, LineTooLong, Regexp};
use super::{
    Count, Found, KEY_BUDGET, Query, count, find_backward, find_forward, find_in, matches_in,
};
use crate::decode::InvalidBytes;
use crate::rope::Rope;

/// The most time a key's search takes on the UI thread, as a regexp can read far past each
/// place it looks.
pub(crate) const KEY_TIME: Duration = Duration::from_millis(3);

/// The longest line a frame reads for a regexp's highlights.
const LONGEST_SHOWN: usize = 64 * 1024;

/// A literal query or a regexp.
#[derive(Clone, Debug)]
pub(crate) enum Needle {
    Literal(Query),
    Regexp(Regexp),
}

/// How far a search goes before it stops: its work, which for a literal query is the bytes it
/// reads, a time, and whether a regexp's stops before a line longer than the work it has
/// left rather than read it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) work: usize,
    pub(crate) until: Option<Instant>,
    pub(crate) short_lines: bool,
}

impl Limits {
    /// No bound but `stop`, for the worker.
    pub(crate) const NONE: Self = Self {
        work: usize::MAX,
        until: None,
        short_lines: false,
    };

    /// A key's on the UI thread: [`KEY_BUDGET`], [`KEY_TIME`] from now, and a long line left
    /// to the worker.
    pub(crate) fn key() -> Self {
        Self {
            work: KEY_BUDGET,
            until: Some(Instant::now() + KEY_TIME),
            short_lines: true,
        }
    }
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

    /// The first match through the whole text that starts at `from` or after, an empty match
    /// there skipped if `after_match`, within `limits`. [`Found::Stopped`] gives where to go
    /// on from, with `after_match` still holding if that is `from`.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn find_forward(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        (from, after_match): (usize, bool),
        limits: Limits,
        stop: &dyn Fn() -> bool,
    ) -> Result<Found, LineTooLong> {
        let regexp = match self {
            Self::Literal(query) => {
                return Ok(find_forward(text, invalid, query, from, limits.work, stop));
            }
            Self::Regexp(regexp) => regexp,
        };
        let whole = 0..text.len();
        let mut scan = regexp::Scan::new(
            regexp,
            text,
            std::slice::from_ref(&whole),
            (from, after_match),
            limits.work,
            None,
        );
        if let Some(until) = limits.until {
            scan.until(until);
        }
        if limits.short_lines {
            scan.short_lines();
        }
        scan.next(stop)
    }

    /// The last match that ends by `before` and starts at `start_at_most` or before, within
    /// `limits`: a literal query's as [`find_backward`] finds it, a regexp's the one that
    /// starts last, as Emacs's `re-search-backward` does. [`Found::Stopped`] gives the
    /// `before` to go on with.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn find_backward(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        (before, start_at_most): (usize, usize),
        limits: Limits,
        stop: &dyn Fn() -> bool,
    ) -> Result<Found, LineTooLong> {
        match self {
            Self::Literal(query) => Ok(find_backward(
                text,
                invalid,
                query,
                before,
                start_at_most,
                limits.work,
                stop,
            )),
            Self::Regexp(regexp) => regexp::find_backward(
                regexp,
                text,
                (before, start_at_most),
                (limits.work, limits.until, limits.short_lines),
                stop,
            ),
        }
    }

    /// Every match in the text, as a scan from its start finds them, and those that start
    /// before `current`. `None` if `stop` said so first.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn count(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        current: Option<usize>,
        stop: &dyn Fn() -> bool,
    ) -> Result<Option<Count>, LineTooLong> {
        let regexp = match self {
            Self::Literal(query) => return Ok(count(text, invalid, query, current, stop)),
            Self::Regexp(regexp) => regexp,
        };
        let whole = 0..text.len();
        let mut scan = regexp::Scan::new(
            regexp,
            text,
            std::slice::from_ref(&whole),
            (0, false),
            usize::MAX,
            None,
        );
        let mut counted = Count {
            total: 0,
            before: 0,
        };
        loop {
            match scan.next(stop)? {
                Found::Match(found) => {
                    counted.total += 1;
                    if current.is_some_and(|current| found.start < current) {
                        counted.before += 1;
                    }
                }
                Found::None => return Ok(Some(counted)),
                Found::Stopped { .. } => return Ok(None),
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
