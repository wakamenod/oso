//! What query-replace and isearch look for (ADR 0113, ADR 0116): a literal string with
//! isearch's matcher, or a regexp matched one line at a time, which may be `migemo`'s
//! pattern for a romaji string; a search through a scope's
//! ranges, match after match, within a budget of work; and isearch's searches forward,
//! backward and its count over the whole text.
//!
//! Must not depend on: `buffer`, `editor` or a log.

use std::ops::Range;
use std::time::Instant;

use super::regexp::{self, Line, LineRoom, LineTooLong, Regexp};
use super::{Count, Found, Limits, Query, count, find_backward, find_forward, find_in, matches_in};
use crate::decode::InvalidBytes;
use crate::migemo::{Dictionary, Romaji};
use crate::rope::Rope;

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
    /// Migemo's needle for `string` (ADR 0116): its pattern, with `dictionary`'s words if
    /// one is given, compiled on a key with the case `case_sensitive` says. A pattern with
    /// words over their size limit, or nested too deep, drops them and keeps the kana. `None`
    /// if the string gives no kana or its kana's pattern is over a key's size limit, and the
    /// caller searches for it as it is.
    pub(crate) fn migemo(
        string: &str,
        case_sensitive: bool,
        dictionary: Option<&Dictionary>,
    ) -> Option<Self> {
        Self::migemo_within(string, case_sensitive, dictionary, regexp::WORDS_SIZE_LIMIT)
    }

    /// [`Self::migemo`], its words compiled within `limit`.
    pub(super) fn migemo_within(
        string: &str,
        case_sensitive: bool,
        dictionary: Option<&Dictionary>,
        limit: usize,
    ) -> Option<Self> {
        let romaji = Romaji::new(string)?;
        let words = dictionary.map(|dictionary| dictionary.pattern_words(&romaji));
        if let Some(words) = words.filter(|words| !words.is_empty()) {
            let pattern = romaji.pattern(string, &words);
            if let Some(regexp) = regexp::compile_migemo(&pattern, !case_sensitive, limit) {
                return Some(Self::Regexp(regexp));
            }
        }
        let pattern = romaji.pattern(string, &[]);
        regexp::compile_migemo(&pattern, !case_sensitive, regexp::KEY_SIZE_LIMIT).map(Self::Regexp)
    }

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
    /// frame's highlights or a jump's key. A regexp's stop at `until` too if one is given, and
    /// read each line into `line`, with a little of the text around each span; a literal
    /// query's read only the text the spans cover, so its budget alone bounds them. Returns
    /// how many bytes it read, at most `limit`; for a literal query, the spans' bytes up to it.
    pub(crate) fn matches_in(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        spans: &[Range<usize>],
        (most, limit): (usize, usize),
        (until, line): (Option<Instant>, &mut LineRoom),
        out: &mut Vec<Range<usize>>,
    ) -> usize {
        match self {
            Self::Literal(query) => {
                matches_in((text, invalid), query, spans, (most, limit), out);
                spans
                    .iter()
                    .map(|span| span.end.saturating_sub(span.start))
                    .fold(0, usize::saturating_add)
                    .min(limit)
            }
            Self::Regexp(regexp) => {
                regexp.matches_in(text, spans, (most, limit), (until, line), out)
            }
        }
    }

    /// The first match through the whole text that starts at `from` or after, an empty match
    /// there skipped if `after_match`, within `limits`. [`Found::Stopped`] gives where to go
    /// on from, with `after_match` still holding if that is `from`. `kept` is a line a regexp's
    /// search before this one read, which it takes if it is in it, and the line it read last
    /// goes back there.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn find_forward(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        (from, after_match): (usize, bool),
        (limits, stop): (Limits, &dyn Fn() -> bool),
        kept: &mut Option<Line>,
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
            kept.take(),
        );
        if let Some(until) = limits.until {
            scan.until(until);
        }
        if limits.on_key {
            scan.on_key();
        }
        let found = scan.next(stop);
        *kept = scan.into_line();
        found
    }

    /// The last match that ends by `before` and starts at `start_at_most` or before, within
    /// `limits`: a literal query's as [`find_backward`] finds it, a regexp's the one that
    /// starts last, as Emacs's `re-search-backward` does. [`Found::Stopped`] gives the
    /// `before` to go on with. `kept` is as [`Needle::find_forward`] keeps it.
    ///
    /// # Errors
    ///
    /// [`LineTooLong`] for a regexp, at a line too long to match in.
    pub(crate) fn find_backward(
        &self,
        (text, invalid): (&Rope, &InvalidBytes),
        (before, start_at_most): (usize, usize),
        (limits, stop): (Limits, &dyn Fn() -> bool),
        kept: &mut Option<Line>,
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
            Self::Regexp(regexp) => {
                regexp::find_backward(regexp, text, (before, start_at_most), (limits, stop), kept)
            }
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
