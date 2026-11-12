//! What a project's search looks for, and how a line matches it (ADR 0114): the input read as
//! completion's orderless components, each matched against one line, and the row a matching
//! line shows as.
//!
//! Must not depend on: `complete`, which shares this layer, `editor` or file I/O. The host reads
//! the files, on threads of its own, and hands the bytes here.
//!
//! - An input is split at spaces, a space after a backslash kept in a component, as completion
//!   splits one. A line matches when every component does, in any order.
//! - A component is a literal and, if it holds a character an expression reads, also that
//!   expression: it matches where either does, as in completion. `=` at its start keeps it a
//!   literal, and `!` at its start says what a line must not hold. Completion's other affixes
//!   are plain text here, and `^` and `$` are an expression's line start and end; an affix is
//!   read only at the start, as code often ends in `=` or `!`.
//! - Case is ignored unless the input holds an upper-case letter, over the whole input.
//! - Expressions are the `regex` crate's, compiled with limits, so a line costs time linear in
//!   its length; one that does not compile matches as a literal.
//! - `Debug` gives how many components, never what they say, which may be about a file that
//!   may hold secrets.

use std::fmt;
use std::ops::Range;

use regex::bytes::{Regex, RegexBuilder};

/// The most rows a search keeps; past them it stops, and the count says `10000+` (ADR 0114).
pub const MAX_ROWS: usize = 10_000;

/// The most bytes of a line a row shows, around its first match.
pub const MAX_ROW_BYTES: usize = 512;

/// The longest line a search reads; past it the rest of its file is skipped.
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// The bytes a search reads of a file at once, cut at a line's end.
pub const BLOCK: usize = 256 * 1024;

/// The most bytes of input matched; a longer input searches for nothing.
const MAX_INPUT_BYTES: usize = 1024;

/// The most components an input is matched with; one with more searches for nothing.
const MAX_COMPONENTS: usize = 32;

/// How many characters a component that is not excluded needs before a search starts.
const STARTS_AT: usize = 2;

/// The most memory a component's expression may take once compiled, as in completion.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// The deepest an expression may nest, as in completion.
const REGEX_NEST_LIMIT: u32 = 32;

/// The characters that make a component an expression, as well as a literal, as in completion.
const REGEX_META: &[char] = &[
    '\\', '.', '+', '*', '?', '(', ')', '|', '[', ']', '{', '}', '^', '$',
];

/// How many bytes before its first match a row starts, when the match is past what a row from
/// the line's start would show.
const BEFORE_MATCH: usize = 64;

/// What a row shows where it leaves part of its line out.
const CUT: char = '…';

/// One component of a search.
#[derive(Clone)]
struct Component {
    regex: Regex,
    /// A line must not hold it.
    not: bool,
    /// It was read as an expression too, so a longer one may match more.
    expression: bool,
    /// What was typed, without its affix, in lower case where case is ignored.
    pattern: Box<str>,
}

/// A search's input read as components, which a line matches when it matches all of them.
#[derive(Clone)]
pub struct LineQuery {
    components: Vec<Component>,
    ignore_case: bool,
    /// The input is past [`MAX_INPUT_BYTES`] or [`MAX_COMPONENTS`]: nothing matches.
    too_long: bool,
}

/// A line of a block that matched: which line it is, counted from the block's first, where it
/// is in the block, without its line break, and where its first match is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHit {
    pub line: usize,
    pub range: Range<usize>,
    pub first: Range<usize>,
}

/// `pattern` as an expression that matches where it does as an expression or as a literal, or as
/// a literal alone, ignoring case if `ignore_case`. `None` if not even the literal compiles
/// within the limits.
fn compile(pattern: &str, expression: bool, ignore_case: bool) -> Option<Regex> {
    let build = |source: &str| {
        RegexBuilder::new(source)
            .case_insensitive(ignore_case)
            .multi_line(true)
            .crlf(true)
            .size_limit(REGEX_SIZE_LIMIT)
            .nest_limit(REGEX_NEST_LIMIT)
            .build()
            .ok()
    };
    let literal = regex::escape(pattern);
    expression
        .then(|| build(&format!("(?:{pattern})|(?:{literal})")))
        .flatten()
        .or_else(|| build(&literal))
}

impl LineQuery {
    /// `input` read as components, as the module says.
    pub fn parse(input: &str) -> Self {
        let too_long = Self {
            components: Vec::new(),
            ignore_case: true,
            too_long: true,
        };
        if input.len() > MAX_INPUT_BYTES {
            return too_long;
        }
        let ignore_case = !input.chars().any(char::is_uppercase);
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
            return too_long;
        }
        let components = raw
            .iter()
            .filter_map(|raw| {
                let (not, literal, rest) = match raw.chars().next() {
                    Some('!') => (true, false, &raw[1..]),
                    Some('=') => (false, true, &raw[1..]),
                    _ => (false, false, raw.as_str()),
                };
                if rest.is_empty() {
                    return None;
                }
                let expression = !literal && rest.contains(REGEX_META);
                let regex = compile(rest, expression, ignore_case)?;
                let pattern = if ignore_case {
                    rest.to_lowercase().into()
                } else {
                    rest.into()
                };
                Some(Component {
                    regex,
                    not,
                    expression,
                    pattern,
                })
            })
            .collect();
        Self {
            components,
            ignore_case,
            too_long: false,
        }
    }

    /// Whether a search starts for it: a component that is not excluded has two characters.
    pub fn starts(&self) -> bool {
        !self.too_long
            && self
                .components
                .iter()
                .any(|component| !component.not && component.pattern.chars().count() >= STARTS_AT)
    }

    /// Whether every line this query matches is one `before` matched, so that the lines found
    /// for `before` hold every one: each of `before`'s components is one of this query's, in its
    /// place, a literal one at most longer at its end, and an expression or an exclusion the
    /// same. Case may stop being ignored, and never start.
    pub fn narrows(&self, before: &Self) -> bool {
        if before.too_long || self.too_long {
            return false;
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
                if new.not != old.not {
                    return false;
                }
                if new.not || new.expression || old.expression {
                    return new.pattern == old.pattern && self.ignore_case == before.ignore_case;
                }
                if self.ignore_case == before.ignore_case {
                    new.pattern.starts_with(&*old.pattern)
                } else {
                    new.pattern.to_lowercase().starts_with(&*old.pattern)
                }
            })
    }

    /// Where the first match of `line`, which holds no line break, is, if it matches: every
    /// component that is not excluded matches it, and no excluded one does. The first match is
    /// the one that starts first.
    pub fn matches_line(&self, line: &[u8]) -> Option<Range<usize>> {
        if self.too_long {
            return None;
        }
        let mut first: Option<Range<usize>> = None;
        for component in &self.components {
            let found = component.regex.find(line);
            match (component.not, found) {
                (true, Some(_)) | (false, None) => return None,
                (true, None) => {}
                (false, Some(found)) => {
                    if first
                        .as_ref()
                        .is_none_or(|first| found.start() < first.start)
                    {
                        first = Some(found.range());
                    }
                }
            }
        }
        first
    }

    /// The lines of `block` that match, in order, each handed to `hit`. A `\r` before a line's
    /// end is not part of it. The first component that is not excluded is looked for through
    /// the whole block, and only a line it is found on is matched whole.
    pub fn find_in_block(&self, block: &[u8], mut hit: impl FnMut(LineHit)) {
        let Some(driver) = self.components.iter().find(|component| !component.not) else {
            return;
        };
        if self.too_long {
            return;
        }
        // `at` is always a line's start, and `line` that line's number.
        let (mut at, mut line) = (0, 0);
        while at < block.len() {
            let Some(found) = driver.regex.find_at(block, at) else {
                break;
            };
            let start = block[at..found.start()]
                .iter()
                .rposition(|&byte| byte == b'\n')
                .map_or(at, |offset| at + offset + 1);
            line += newlines(&block[at..start]);
            let end = block[found.start()..]
                .iter()
                .position(|&byte| byte == b'\n')
                .map_or(block.len(), |offset| found.start() + offset);
            let text_end = if block[start..end].ends_with(b"\r") {
                end - 1
            } else {
                end
            };
            if let Some(first) = self.matches_line(&block[start..text_end]) {
                hit(LineHit {
                    line,
                    range: start..text_end,
                    first,
                });
            }
            if end == block.len() {
                break;
            }
            at = end + 1;
            line += 1;
        }
    }

    /// Where the components that are not excluded match `text`, a row's, sorted and joined
    /// where they touch, into `out`, which is cleared first. Each range is on character
    /// boundaries.
    pub fn highlights(&self, text: &str, out: &mut Vec<Range<usize>>) {
        out.clear();
        if self.too_long {
            return;
        }
        for component in self.components.iter().filter(|component| !component.not) {
            for found in component.regex.find_iter(text.as_bytes()) {
                let start = floor_boundary(text, found.start());
                let end = ceil_boundary(text, found.end());
                if start < end {
                    out.push(start..end);
                }
            }
        }
        out.sort_unstable_by_key(|range| range.start);
        let mut kept: usize = 0;
        for index in 0..out.len() {
            let range = out[index].clone();
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
}

impl fmt::Debug for LineQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LineQuery")
            .field("components", &self.components.len())
            .field("ignore_case", &self.ignore_case)
            .finish()
    }
}

/// How many line breaks `bytes` holds.
fn newlines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&byte| byte == b'\n').count()
}

/// `at`, or the character boundary of `text` before it.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `at`, or the character boundary of `text` after it.
fn ceil_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}

/// The bytes of `line` as text, as a buffer reads them: each invalid byte one U+FFFD.
fn decoded(line: &[u8]) -> String {
    let mut text = String::with_capacity(line.len());
    for chunk in line.utf8_chunks() {
        text.push_str(chunk.valid());
        for _ in chunk.invalid() {
            text.push(char::REPLACEMENT_CHARACTER);
        }
    }
    text
}

/// The text a row shows of `line`, which holds no line break, whose first match is `first`: at
/// most [`MAX_ROW_BYTES`] of it around that match, cut at characters, each invalid byte one
/// U+FFFD, with `…` where part of the line is left out. Also the match's column, in characters
/// from the line's start, as a buffer of the file counts them.
pub fn row_text(line: &[u8], first: Range<usize>) -> (Box<str>, usize) {
    let first_start = first.start.min(line.len());
    let column = decoded(&line[..first_start]).chars().count();
    let mut start = if first.end.min(line.len()) <= MAX_ROW_BYTES {
        0
    } else {
        first_start.saturating_sub(BEFORE_MATCH)
    };
    let mut end = (start + MAX_ROW_BYTES).min(line.len());
    // A character a bound would cut in two is left out, at most three bytes of it.
    let continues = |at: usize| line.get(at).is_some_and(|byte| byte & 0xC0 == 0x80);
    for _ in 0..3 {
        if start > 0 && continues(start) {
            start += 1;
        }
        if end < line.len() && continues(end) {
            end -= 1;
        }
    }
    let end = end.max(start);
    let mut text = String::with_capacity(end - start + 2 * CUT.len_utf8());
    if start > 0 {
        text.push(CUT);
    }
    text.push_str(&decoded(&line[start..end]));
    if end < line.len() {
        text.push(CUT);
    }
    (text.into(), column)
}

#[cfg(test)]
mod tests;
