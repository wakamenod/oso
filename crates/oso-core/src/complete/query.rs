//! A completion input read as components, and how each matches a candidate (ADR 0076).
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::fmt;
use std::ops::Range;

use regex::{Regex, RegexBuilder};

use super::{AFFIXES, Candidate};

/// The most memory a component's expression may take once compiled, so that one pasted in
/// cannot take long to compile. A larger one matches as a literal. It admits ordinary Perl
/// classes, such as `\w{10}`, whose Unicode tables are large.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// The most memory a component's literal and expression joined in one may take, far below
/// [`REGEX_SIZE_LIMIT`]: a larger one, such as a long repetition, is not joined, so that its
/// compiling gives up early and the two are searched in turn, as they would be.
const JOINED_SIZE_LIMIT: usize = 1 << 14;

/// The characters that can start a class in an expression, whose Unicode tables cost the most to
/// compile: an expression with one is not joined to its literal, so that a class is compiled once
/// a key, as before, on the UI thread too.
const CLASS_START: &[char] = &['\\', '['];

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
    /// An expression was tried for it, whether or not it compiled, so that it is tried once
    /// per input (ADR 0076).
    tried: bool,
    /// For a literal one, a searcher for it compiled once, which a candidate's text is
    /// searched with: `None` where it did not compile, when the text is searched as is.
    finder: Option<Regex>,
    /// How many characters it has: a candidate with fewer bytes cannot hold it.
    chars: usize,
    /// For a literal one with an expression too, both in one expression, which a candidate
    /// is matched with in one search when the place it matched is not wanted: `None` where they
    /// did not compile together, when each is searched in turn.
    joined: Option<Regex>,
    /// The candidate must not match it.
    not: bool,
    /// It matches the annotation, not the candidate.
    annotation: bool,
    /// What it matches, in lower case where case is ignored.
    pattern: Box<str>,
}

impl PartialEq for Component {
    /// The expression and the searcher follow from the rest: whether it is tried as an
    /// expression too, as `=` says it is not.
    fn eq(&self, other: &Self) -> bool {
        (
            self.style,
            self.not,
            self.annotation,
            self.tried,
            &self.pattern,
        ) == (
            other.style,
            other.not,
            other.annotation,
            other.tried,
            &other.pattern,
        )
    }
}

impl Eq for Component {}

/// `pattern` compiled within the limits, ignoring case if `ignore_case`, or `None` if it
/// does not compile.
fn compile(pattern: &str, ignore_case: bool) -> Option<Regex> {
    compile_within(pattern, ignore_case, REGEX_SIZE_LIMIT)
}

/// `pattern` compiled as [`compile`] does, within `size_limit` bytes.
fn compile_within(pattern: &str, ignore_case: bool, size_limit: usize) -> Option<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(size_limit)
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
        // `=` and the other affixes but `!` and `&` take one style only.
        let wants_regex = matches!(affix, None | Some('!' | '&')) && rest.contains(REGEX_META);
        // What the query before compiled for the same component is reused, a failure too:
        // its text, style, case and whether it is an expression decide it.
        let same = before
            .components
            .iter()
            .filter(|_| before.ignore_case == ignore_case)
            .find(|old| old.pattern == pattern && old.style == style && old.tried == wants_regex);
        let (regex, tried) = match same {
            Some(old) if old.tried => (old.regex.clone(), true),
            _ if wants_regex => (compile(rest, ignore_case), true),
            _ => (None, false),
        };
        let finder = match same {
            Some(old) => old.finder.clone(),
            None if style == Style::Literal => compile(&regex::escape(rest), ignore_case),
            None => None,
        };
        // Only an expression that compiles alone is joined to a literal that does, so that the
        // two in one match exactly where they match in turn.
        let joined = match same {
            Some(old) => old.joined.clone(),
            None if regex.is_some() && finder.is_some() && !rest.contains(CLASS_START) => {
                compile_within(
                    &format!("{}|(?:{rest})", regex::escape(rest)),
                    ignore_case,
                    JOINED_SIZE_LIMIT,
                )
            }
            None => None,
        };
        Some(Self {
            style,
            regex,
            tried,
            finder,
            chars: rest.chars().count(),
            joined,
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
        let Some(out) = out else {
            return self.hits(text, haystack) != self.not;
        };
        let mut out = Some(out).filter(|_| !self.not && !self.annotation);
        let start = out.as_ref().map_or(0, |out| out.len());
        let mut push = |range: Range<usize>| {
            if let Some(out) = out.as_mut() {
                out.push(range);
            }
        };
        // An expression matches the original text, ignoring case itself, so where it matched
        // needs no mapping back.
        let mut original = false;
        // A text of fewer bytes than the component has characters cannot hold it.
        let literal = if self.chars > text.len() {
            None
        } else if let Some(finder) = &self.finder {
            finder.find(text).map(|hit| {
                original = true;
                hit.range()
            })
        } else {
            haystack.find(pattern).map(|at| at..at + pattern.len())
        };
        let hit = match self.style {
            Style::Literal => match literal {
                Some(range) => {
                    push(range);
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

    /// Whether it is found in `text`, or in `haystack`, its folded form where case is ignored,
    /// as [`Component::matches`] finds it, without where: a search that need not report where
    /// it matched stops at the first match it is sure of.
    fn hits(&self, text: &str, haystack: &str) -> bool {
        let pattern = &*self.pattern;
        match self.style {
            Style::Literal => {
                if let Some(joined) = &self.joined {
                    return joined.is_match(text);
                }
                let literal = self.chars <= text.len()
                    && match &self.finder {
                        Some(finder) => finder.is_match(text),
                        None => haystack.contains(pattern),
                    };
                literal
                    || self
                        .regex
                        .as_ref()
                        .is_some_and(|regex| regex.is_match(text))
            }
            Style::Prefix => haystack.starts_with(pattern),
            Style::Flex => in_order(haystack, pattern, |_, _| true, |_| {}),
            Style::Initialism => in_order(haystack, pattern, starts_word, |_| {}),
        }
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
        // Split first, so that an input of too many components compiles none of them.
        let mut raw = Vec::new();
        let mut current = String::new();
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&' ') => {
                    current.push(' ');
                    chars.next();
                }
                ' ' => raw.push(std::mem::take(&mut current)),
                _ => current.push(c),
            }
        }
        raw.push(current);
        raw.retain(|component| !component.is_empty());
        if raw.len() > MAX_COMPONENTS {
            return Self::too_long();
        }
        let components = raw
            .iter()
            .filter_map(|raw| Component::parse(raw, ignore_case, before))
            .collect();
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

    /// Whether `candidate`, which `before` matched, matches the components that are not as
    /// they were in `before`: where this query [`narrows`](Query::narrows) `before`, the others
    /// still match.
    pub(crate) fn matches_after(&self, candidate: &Candidate, before: &Query) -> bool {
        if self.too_long || self.ignore_case != before.ignore_case {
            return self.matches(candidate);
        }
        self.components
            .iter()
            .enumerate()
            .filter(|&(index, component)| before.components.get(index) != Some(component))
            .all(|(_, component)| component.matches(candidate, self.ignore_case, None))
    }

    /// Whether `candidate` matches every component, each found with where it matched, as
    /// highlighting finds it, for tests that [`Query::matches`] agrees.
    #[cfg(test)]
    fn matches_finding_where(&self, candidate: &Candidate) -> bool {
        let mut out = Vec::new();
        !self.too_long
            && self
                .components
                .iter()
                .all(|component| component.matches(candidate, self.ignore_case, Some(&mut out)))
    }

    /// For each component, whether its literal and expression are searched in one, for tests.
    #[cfg(test)]
    fn joined(&self) -> Vec<bool> {
        self.components
            .iter()
            .map(|component| component.joined.is_some())
            .collect()
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
                if new.not || new.tried || old.tried {
                    // A literal read as an expression too matches more, and the other way less.
                    return new.tried == old.tried
                        && new.pattern == old.pattern
                        && self.ignore_case == before.ignore_case;
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

#[cfg(test)]
mod tests;
