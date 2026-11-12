//! A regexp's match in a range that ends inside a line (ADR 0113), as Emacs's bounded search
//! takes it: the match ends by the range's end, and `$` and `\b` there see the text after it,
//! so the range's end is neither a line's end nor a word's.
//!
//! Must not depend on: `buffer`, `editor` or a log.
//!
//! The range is the haystack, so a search reads no further than it. A match that ends at the
//! range's end is checked against the text after it, by two regexps made from the pattern:
//! one whose match ends just there, the next character read, and one whose match ends before
//! it.

use std::ops::Range;

use regex::Regex;

use super::{PER_MATCH, Regexp, builder};

/// The two regexps made from a pattern, for a match at a range's end.
#[derive(Clone)]
pub(super) struct AtCut {
    /// The pattern, then one character, then the haystack's end: a match of the pattern that
    /// ends just before the character after the range.
    ending: Regex,
    /// The pattern, then one character: a match that ends before the range's end.
    short: Regex,
}

impl AtCut {
    /// The two regexps for `pattern`, with smart case as `insensitive` says.
    pub(super) fn new(pattern: &str, insensitive: bool) -> Result<Self, regex::Error> {
        Ok(Self {
            ending: build(pattern, r"(?s:.)\z", insensitive)?,
            short: build(pattern, "(?s:.)", insensitive)?,
        })
    }
}

/// `pattern` as a group, then `then`. A `(?x)` pattern's comment would run past the group's
/// end, so that one is ended by a line break, which `(?x)` ignores.
fn build(pattern: &str, then: &str, insensitive: bool) -> Result<Regex, regex::Error> {
    builder(&format!("(?:{pattern}){then}"), insensitive)
        .build()
        .or_else(|_| builder(&format!("(?:{pattern}\n){then}"), insensitive).build())
}

/// Where the first match from an offset that ends at a cut starts, kept for one cut of one
/// line, as the offsets a search asks from only grow.
#[derive(Default)]
pub(super) struct Ending {
    /// The line's start and the cut, the offset asked from, and the answer.
    kept: Option<((usize, usize), usize, Option<usize>)>,
}

/// The first match of `regexp` from `from` in `text[..cut]`, a line's text, as if the text
/// went on past `cut`, the line starting at `line_start`: it ends by `cut`, and an assertion
/// at `cut` sees the text after it. `work` counts what it reads.
pub(super) fn find(
    regexp: &Regexp,
    (text, line_start, cut): (&str, usize, usize),
    mut from: usize,
    ending: &mut Ending,
    work: &mut usize,
) -> Option<Range<usize>> {
    let haystack = &text[..cut];
    loop {
        if from > cut {
            return None;
        }
        let found = regexp.regex.find_at(haystack, from);
        // A look that finds nothing read to the haystack's end; one that finds a match read
        // about to its end, and a pattern that reads further is what `until` is for.
        *work =
            work.saturating_add(found.map_or(cut - from, |found| found.end() - from) + PER_MATCH);
        let found = found?.range();
        // A match that ends before the cut, or at the line's end, is one. So is one at the
        // cut if the regexps for a cut do not compile, which only a pattern near the size
        // limit can make so.
        let Some(at_cut) = regexp
            .at_cut()
            .filter(|_| found.end == cut && cut < text.len())
        else {
            return Some(found);
        };
        // It ends at the cut, before the line does: whether it is a match depends on what
        // follows, as `$` or `\b` there may hold only at the haystack's end.
        let ends_at = ending_from(at_cut, (text, line_start, cut), found.start, ending, work);
        if ends_at == Some(found.start) {
            return Some(found);
        }
        // A match from its start that ends before the cut, or the next to start anywhere.
        let short = at_cut.short.find_at(haystack, found.start);
        *work =
            work.saturating_add(short.map_or(cut, |short| short.end()) - found.start + PER_MATCH);
        let short = short.map(|short| {
            let last = haystack[..short.end()]
                .chars()
                .next_back()
                .map_or(0, char::len_utf8);
            short.start()..short.end() - last
        });
        match (short, ends_at) {
            (Some(short), _) if short.start == found.start => return Some(short),
            (None, None) => return None,
            (short, ends_at) => {
                from = short
                    .map(|short| short.start)
                    .into_iter()
                    .chain(ends_at)
                    .min()?;
            }
        }
    }
}

/// Where the first match from `from` that ends at `cut` starts, the character after the cut
/// read: from what `ending` kept while it still holds.
fn ending_from(
    at_cut: &AtCut,
    (text, line_start, cut): (&str, usize, usize),
    from: usize,
    ending: &mut Ending,
    work: &mut usize,
) -> Option<usize> {
    if let Some((key, asked, start)) = ending.kept
        && key == (line_start, cut)
        && asked <= from
        && start.is_none_or(|start| from <= start)
    {
        return start;
    }
    let next = text[cut..].chars().next().map_or(0, char::len_utf8);
    let found = at_cut.ending.find_at(&text[..cut + next], from);
    *work = work.saturating_add(cut + next - from + PER_MATCH);
    let start = found.map(|found| found.start());
    ending.kept = Some(((line_start, cut), from, start));
    start
}
