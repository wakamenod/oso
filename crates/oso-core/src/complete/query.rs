//! A completion input read as components, and how each matches a candidate (ADR 0076).
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::fmt;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::{AFFIXES, Candidate};

/// The most memory a component's expression may take once compiled, so that one pasted in
/// cannot take long to compile. A larger one matches as a literal.
const REGEX_SIZE_LIMIT: usize = 64 * 1024;

/// The deepest an expression may nest.
const REGEX_NEST_LIMIT: u32 = 32;

/// The characters that make a component an expression, as well as a literal.
const REGEX_META: &[char] = &[
    '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$',
];

/// How a component matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// Anywhere in the candidate, as typed.
    Literal,
    /// At its start.
    Prefix,
    /// Each character starts a word of it, in order: `,sbk` is `save-buffers-kill-terminal`.
    Initialism,
    /// Each character in order, anything between: `~fwc` is `forward-char`.
    Flex,
}

/// One component of a query.
#[derive(Clone)]
struct Component {
    style: Style,
    /// For a literal one with a character an expression reads, that expression: it matches
    /// where either does, as orderless's default styles, `orderless-literal` and
    /// `orderless-regexp`, do.
    regex: Option<Regex>,
    /// The candidate must not match it.
    not: bool,
    /// It matches the annotation, not the candidate.
    annotation: bool,
    /// What it matches, in lower case where case is ignored.
    pattern: Box<str>,
}

impl PartialEq for Component {
    /// The expression follows from the rest.
    fn eq(&self, other: &Self) -> bool {
        (self.style, self.not, self.annotation, &self.pattern)
            == (other.style, other.not, other.annotation, &other.pattern)
    }
}

impl Eq for Component {}

/// `pattern` compiled within the limits, ignoring case if `ignore_case`, or `None` if it
/// does not compile.
fn compile(pattern: &str, ignore_case: bool) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(REGEX_SIZE_LIMIT)
        .nest_limit(REGEX_NEST_LIMIT)
        .build()
        .ok()
}

impl Component {
    /// The component `raw` stands for, with its affix read, or `None` if nothing is left
    /// to match once the affix is.
    fn parse(raw: &str, ignore_case: bool, before: &Query) -> Option<Self> {
        let first = raw.chars().next()?;
        let last = raw.chars().next_back()?;
        let (affix, rest) = if AFFIXES.contains(&first) {
            (Some(first), &raw[first.len_utf8()..])
        } else if AFFIXES.contains(&last) {
            (Some(last), &raw[..raw.len() - last.len_utf8()])
        } else {
            (None, raw)
        };
        if rest.is_empty() {
            return None;
        }
        let style = match affix {
            Some(',') => Style::Initialism,
            Some('^') => Style::Prefix,
            Some('~') => Style::Flex,
            _ => Style::Literal,
        };
        let pattern = if ignore_case {
            rest.to_lowercase().into()
        } else {
            rest.into()
        };
        // `=` and the other affixes but `!` and `&` take one style only. An expression the
        // query before compiled is reused: its text and case decide it.
        let regex = (matches!(affix, None | Some('!' | '&')) && rest.contains(REGEX_META))
            .then(|| {
                let same = |old: &&Component| old.pattern == pattern && old.regex.is_some();
                let compiled = (before.ignore_case == ignore_case)
                    .then(|| before.components.iter().find(same))
                    .flatten()
                    .and_then(|old| old.regex.clone());
                compiled.or_else(|| compile(rest, ignore_case))
            })
            .flatten();
        Some(Self {
            style,
            regex,
            not: affix == Some('!'),
            annotation: affix == Some('&'),
            pattern,
        })
    }

    /// Whether it matches `candidate`, and where, in the original text, into `out` if it is
    /// given. Nothing is recorded for a component that says what must not match, or that
    /// matches the annotation.
    fn matches(
        &self,
        candidate: &Candidate,
        ignore_case: bool,
        out: Option<&mut Vec<Range<usize>>>,
    ) -> bool {
        let (text, folded) = if self.annotation {
            (&*candidate.annotation, &candidate.folded_annotation)
        } else {
            (&*candidate.text, &candidate.folded)
        };
        let haystack = match (&folded.text, ignore_case) {
            (Some(folded), true) => folded,
            _ => text,
        };
        let pattern = &*self.pattern;
        let mut out = out.filter(|_| !self.not && !self.annotation);
        let start = out.as_ref().map_or(0, |out| out.len());
        let mut push = |range: Range<usize>| {
            if let Some(out) = out.as_mut() {
                out.push(range);
            }
        };
        // An expression matches the original text, ignoring case itself, so where it matched
        // needs no mapping back.
        let mut original = false;
        let hit = match self.style {
            Style::Literal => match haystack.find(pattern) {
                Some(at) => {
                    push(at..at + pattern.len());
                    true
                }
                None => match self.regex.as_ref().and_then(|regex| regex.find(text)) {
                    Some(hit) => {
                        original = true;
                        push(hit.range());
                        true
                    }
                    None => false,
                },
            },
            Style::Prefix => {
                let hit = haystack.starts_with(pattern);
                if hit {
                    push(0..pattern.len());
                }
                hit
            }
            Style::Flex => in_order(haystack, pattern, |_, _| true, push),
            Style::Initialism => in_order(haystack, pattern, starts_word, push),
        };
        if let Some(out) = out {
            if !hit {
                out.truncate(start);
            } else if ignore_case && folded.text.is_some() && !original {
                for range in &mut out[start..] {
                    *range = folded.original(range.clone());
                }
            }
        }
        hit != self.not
    }
}

/// Whether the characters of `pattern` are in `haystack` in order, each where `may_take`
/// allows it, taking each as early as it can. Gives `found` where each was.
fn in_order(
    haystack: &str,
    pattern: &str,
    may_take: impl Fn(Option<char>, char) -> bool,
    mut found: impl FnMut(Range<usize>),
) -> bool {
    let mut wanted = pattern.chars().peekable();
    let mut before = None;
    for (offset, c) in haystack.char_indices() {
        let Some(&next) = wanted.peek() else {
            break;
        };
        if c == next && may_take(before, c) {
            found(offset..offset + c.len_utf8());
            wanted.next();
        }
        before = Some(c);
    }
    wanted.peek().is_none()
}

/// Whether `c` starts a word, after `before`: a letter or digit after none, as Emacs's `\<`
/// finds with a word's syntax.
fn starts_word(before: Option<char>, c: char) -> bool {
    c.is_alphanumeric() && !before.is_some_and(char::is_alphanumeric)
}

/// The longest input matched, in bytes. A longer one, such as a whole file yanked into the
/// answer, matches nothing, so that no key costs more than this much matching (ADR 0076).
pub(crate) const MAX_INPUT_BYTES: usize = 1024;

/// The most components an input is matched with. One with more matches nothing.
pub(crate) const MAX_COMPONENTS: usize = 32;

/// An input read as components, which a candidate matches when it matches all of them.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Query {
    components: Vec<Component>,
    ignore_case: bool,
    /// The input is past [`MAX_INPUT_BYTES`] or [`MAX_COMPONENTS`]: nothing matches.
    too_long: bool,
}

impl Query {
    /// `input` split at spaces into components, a space after a backslash kept in one, as
    /// orderless's `orderless-escapable-split-on-space` does. Case is ignored unless `input`
    /// holds an upper-case letter.
    #[cfg(test)]
    pub(crate) fn parse(input: &str) -> Self {
        Self::parse_after(input, &Self::default())
    }

    /// [`Query::parse`], reusing what `before` compiled for a component that is the same.
    pub(crate) fn parse_after(input: &str, before: &Query) -> Self {
        if input.len() > MAX_INPUT_BYTES {
            return Self::too_long();
        }
        let ignore_case = !input.chars().any(char::is_uppercase);
        let mut components = Vec::new();
        let mut current = String::new();
        let mut push = |current: &mut String| {
            components.extend(Component::parse(current, ignore_case, before));
            current.clear();
        };
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => {
                    current.push(' ');
                    chars.next();
                }
                ' ' => push(&mut current),
                _ => current.push(c),
            }
        }
        push(&mut current);
        if components.len() > MAX_COMPONENTS {
            return Self::too_long();
        }
        Self {
            components,
            ignore_case,
            too_long: false,
        }
    }

    /// The query of an input too long to match: nothing matches it.
    pub(crate) fn too_long() -> Self {
        Self {
            too_long: true,
            ..Self::default()
        }
    }

    /// Whether `candidate` matches every component.
    pub(crate) fn matches(&self, candidate: &Candidate) -> bool {
        !self.too_long
            && self
                .components
                .iter()
                .all(|component| component.matches(candidate, self.ignore_case, None))
    }

    /// Where the components matched `candidate`'s text, in its byte offsets, sorted and
    /// joined where they touch, into `out`, which is cleared first. Nothing for a component
    /// that says what must not match, or that matches the annotation.
    pub fn highlights(&self, candidate: &Candidate, out: &mut Vec<Range<usize>>) {
        out.clear();
        for component in &self.components {
            component.matches(candidate, self.ignore_case, Some(out));
        }
        out.sort_unstable_by_key(|range| range.start);
        // Joined in place, so that the caller's vector is reused from row to row.
        let mut kept: usize = 0;
        for index in 0..out.len() {
            let range = out[index].clone();
            // An expression such as `a*` may match nothing, which shows nothing.
            if range.is_empty() {
                continue;
            }
            match kept.checked_sub(1).map(|last| &mut out[last]) {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => {
                    out[kept] = range;
                    kept += 1;
                }
            }
        }
        out.truncate(kept);
    }

    /// Whether every candidate this query matches is one `before` matched, so that it
    /// needs to look only at those: each of `before`'s components is one of this query's,
    /// in its place, at most longer at its end, and none says what must not match unless
    /// it is the same. Case may stop being ignored, and never start.
    pub(crate) fn narrows(&self, before: &Query) -> bool {
        if before.too_long {
            return false;
        }
        if self.too_long {
            return true;
        }
        if self.ignore_case && !before.ignore_case {
            return false;
        }
        if self.components.len() < before.components.len() {
            return false;
        }
        self.components
            .iter()
            .zip(&before.components)
            .all(|(new, old)| {
                if new.style != old.style || new.not != old.not || new.annotation != old.annotation
                {
                    return false;
                }
                // A negation that matches less, or with case, keeps more candidates, and a
                // longer expression may match more.
                if new.not || new.regex.is_some() || old.regex.is_some() {
                    return new.pattern == old.pattern && self.ignore_case == before.ignore_case;
                }
                if self.ignore_case == before.ignore_case {
                    new.pattern.starts_with(&*old.pattern)
                } else {
                    new.pattern.to_lowercase().starts_with(&*old.pattern)
                }
            })
    }
}

impl fmt::Debug for Query {
    /// How many components, and never what they say, which may be about a secret buffer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("components", &self.components.len())
            .field("ignore_case", &self.ignore_case)
            .finish()
    }
}
