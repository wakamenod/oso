//! An EditorConfig section's glob, translated to a `regex` and matched against a file's path
//! from the `.editorconfig`'s directory (ADR 0115).
//!
//! Must not depend on: any other module of the crate; it works on strings alone.
//!
//! `*` is any run of characters but `/`, `**` any run at all, `?` one character but `/`,
//! `[...]` and `[!...]` one character in or not in a set, with ranges such as `a-z`,
//! `{a,b}` one of several globs, which may nest, and `{n1..n2}` a whole number from `n1` to
//! `n2`. A `\` takes the character after it as itself. Anything that does not form one of
//! these, such as a `{` with no `}` or no `,` before it, or a `[` whose set holds a `/`, is
//! itself, as editorconfig-core-test has it. A number range becomes a capture whose text is
//! checked as a number after the match: it is never expanded into alternatives, so `{1..9999999}`
//! costs no more than `{1..2}`.

use regex::{Regex, RegexBuilder};

/// The most bytes a glob may have.
pub const MAX_GLOB: usize = 4 << 10;

/// The deepest braces may nest.
pub const MAX_NESTING: usize = 8;

/// The most a glob's compiled `regex` may take, in bytes, and its lazy DFA's cache.
const SIZE_LIMIT: usize = 1 << 20;

/// Why a glob was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// It is longer than [`MAX_GLOB`].
    TooLong,
    /// Its braces nest deeper than [`MAX_NESTING`].
    NestedTooDeep,
    /// Its `regex` is larger than the size limit.
    TooComplex,
}

/// A glob ready to match paths.
#[derive(Debug)]
pub struct Glob {
    regex: Regex,
    /// The number ranges, in the order of their captures.
    ranges: Vec<(i64, i64)>,
}

impl Glob {
    /// The glob of a section named `glob`.
    pub fn new(glob: &str) -> Result<Self, Refused> {
        if glob.len() > MAX_GLOB {
            return Err(Refused::TooLong);
        }
        let mut translation = Translation::default();
        // Matched against the path with a `/` before it. A glob with a `/` in it, not in a
        // set, is relative to the file's directory, a leading `/` or not; any other matches
        // the name in any directory below it, as if it started with `**/`.
        let chars: Vec<char> = glob.chars().collect();
        if has_separator(&chars) {
            let rest = chars.strip_prefix(&['/']).unwrap_or(&chars);
            translation.sequence(&[&['/'], rest].concat(), 0)?;
        } else {
            translation.regex.push_str("(?:/|/.*/)");
            translation.sequence(&chars, 0)?;
        }
        let regex = RegexBuilder::new(&format!("(?s)^{}$", translation.regex))
            .size_limit(SIZE_LIMIT)
            .dfa_size_limit(SIZE_LIMIT)
            .build()
            .map_err(|_| Refused::TooComplex)?;
        Ok(Self {
            regex,
            ranges: translation.ranges,
        })
    }

    /// Whether it matches `path`, the file's path from the `.editorconfig`'s directory, its
    /// parts joined by `/`, with no `/` before it.
    pub fn matches(&self, path: &str) -> bool {
        let path = format!("/{path}");
        if self.ranges.is_empty() {
            return self.regex.is_match(&path);
        }
        let Some(captures) = self.regex.captures(&path) else {
            return false;
        };
        // A range in an alternative not taken has no capture, and asks nothing.
        captures
            .iter()
            .skip(1)
            .zip(&self.ranges)
            .all(|(number, &(low, high))| {
                number.is_none_or(|number| in_range(number.as_str(), low, high))
            })
    }
}

/// Whether `glob` holds a `/` outside a set.
fn has_separator(glob: &[char]) -> bool {
    let mut at = 0;
    while at < glob.len() {
        match glob[at] {
            '\\' => at += 1,
            '/' => return true,
            '[' => {
                if let Some(end) = set_end(glob, at) {
                    at = end;
                }
            }
            _ => {}
        }
        at += 1;
    }
    false
}

/// Whether `number`, as a capture took it, is a whole number from `low` to `high`, written
/// with no `+` and no leading zero.
fn in_range(number: &str, low: i64, high: i64) -> bool {
    let digits = number.strip_prefix('-').unwrap_or(number);
    if (digits.len() > 1 && digits.starts_with('0')) || number == "-0" {
        return false;
    }
    number
        .parse::<i64>()
        .is_ok_and(|number| (low..=high).contains(&number))
}

/// A glob's `regex` as it is written, and its number ranges.
#[derive(Default)]
struct Translation {
    regex: String,
    ranges: Vec<(i64, i64)>,
}

impl Translation {
    /// Translates `glob`, braces `depth` deep.
    fn sequence(&mut self, glob: &[char], depth: usize) -> Result<(), Refused> {
        let mut at = 0;
        while at < glob.len() {
            at = match glob[at] {
                '\\' => {
                    // A `\` at the end is itself.
                    let escaped = glob.get(at + 1).copied().unwrap_or('\\');
                    self.literal(escaped);
                    at + 2
                }
                '*' => self.star(glob, at),
                '?' => {
                    self.regex.push_str("[^/]");
                    at + 1
                }
                '[' => self.set(glob, at),
                '{' => self.braces(glob, at, depth)?,
                c => {
                    self.literal(c);
                    at + 1
                }
            };
        }
        Ok(())
    }

    /// `*` or `**` at `at`, where `/**/` may also be one `/`. Returns where the glob goes on.
    fn star(&mut self, glob: &[char], at: usize) -> usize {
        if glob.get(at + 1) != Some(&'*') {
            self.regex.push_str("[^/]*");
            return at + 1;
        }
        // `/**/` matches `/` too, so `a/**/b` matches `a/b`. The `/` before it is in the
        // regex already.
        if at > 0
            && glob[at - 1] == '/'
            && glob.get(at + 2) == Some(&'/')
            && self.regex.ends_with('/')
        {
            self.regex.pop();
            self.regex.push_str("(?:/|/.*/)");
            return at + 3;
        }
        self.regex.push_str(".*");
        at + 2
    }

    /// A set at `at`, or a `[` that is itself. Returns where the glob goes on.
    fn set(&mut self, glob: &[char], at: usize) -> usize {
        let Some(end) = set_end(glob, at) else {
            self.literal('[');
            return at + 1;
        };
        let mut inside = &glob[at + 1..end];
        let negated = inside.first() == Some(&'!');
        if negated {
            inside = &inside[1..];
        }
        let mut members = Vec::new();
        let mut i = 0;
        while i < inside.len() {
            let (low, next) = member(inside, i);
            // `a-z` is a range; a `-` first or last is itself.
            if inside.get(next) == Some(&'-') && next + 1 < inside.len() {
                let (high, after) = member(inside, next + 1);
                if low <= high {
                    members.push(format!(
                        "\\x{{{:x}}}-\\x{{{:x}}}",
                        u32::from(low),
                        u32::from(high)
                    ));
                }
                i = after;
            } else {
                members.push(format!("\\x{{{:x}}}", u32::from(low)));
                i = next;
            }
        }
        match (negated, members.is_empty()) {
            (false, true) => self.regex.push_str("[a&&b]"),
            (false, false) => self.regex.push_str(&format!("[{}]", members.concat())),
            // One character but `/`, as `?`.
            (true, _) => self.regex.push_str(&format!("[^/{}]", members.concat())),
        }
        end + 1
    }

    /// Braces at `at`: one of several globs, a number range, or a `{` that is itself.
    /// Returns where the glob goes on.
    fn braces(&mut self, glob: &[char], at: usize, depth: usize) -> Result<usize, Refused> {
        let Some(end) = brace_end(glob, at) else {
            self.literal('{');
            return Ok(at + 1);
        };
        let inside = &glob[at + 1..end];
        if let Some(range) = number_range(inside) {
            self.regex.push_str("(-?[0-9]+)");
            self.ranges.push(range);
            return Ok(end + 1);
        }
        let choices = split_choices(inside);
        if choices.len() < 2 {
            // `{a}` and `{}` are themselves; what is inside is read as usual.
            self.literal('{');
            return Ok(at + 1);
        }
        if depth + 1 > MAX_NESTING {
            return Err(Refused::NestedTooDeep);
        }
        self.regex.push_str("(?:");
        for (index, choice) in choices.into_iter().enumerate() {
            if index > 0 {
                self.regex.push('|');
            }
            self.sequence(choice, depth + 1)?;
        }
        self.regex.push(')');
        Ok(end + 1)
    }

    fn literal(&mut self, c: char) {
        let mut buffer = [0; 4];
        self.regex
            .push_str(&regex::escape(c.encode_utf8(&mut buffer)));
    }
}

/// Where the set starting at `at` ends, its `]`: `None` if there is none, or if a `/` comes
/// first, as a set never holds one.
fn set_end(glob: &[char], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while i < glob.len() {
        match glob[i] {
            '\\' => i += 1,
            ']' => return Some(i),
            '/' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// The member of a set at `i`, `\` taking the character after it as itself, and where the
/// next one starts.
fn member(set: &[char], i: usize) -> (char, usize) {
    match (set[i], set.get(i + 1)) {
        ('\\', Some(&escaped)) => (escaped, i + 2),
        (c, _) => (c, i + 1),
    }
}

/// Where the braces starting at `at` end, the `}` that closes them, counting those inside.
fn brace_end(glob: &[char], at: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = at;
    while i < glob.len() {
        match glob[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// What braces hold split at their own commas, not those of braces inside them.
fn split_choices(inside: &[char]) -> Vec<&[char]> {
    let mut choices = Vec::new();
    let (mut depth, mut start, mut i) = (0usize, 0, 0);
    while i < inside.len() {
        match inside[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                choices.push(&inside[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    choices.push(&inside[start.min(inside.len())..]);
    choices
}

/// `n1..n2`, two whole numbers with the first the smaller, if braces hold that.
fn number_range(inside: &[char]) -> Option<(i64, i64)> {
    let text: String = inside.iter().collect();
    let (low, high) = text.split_once("..")?;
    let number = |text: &str| {
        let digits = text.strip_prefix('-').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok()
    };
    let (low, high) = (number(low)?, number(high)?);
    (low <= high).then_some((low, high))
}

#[cfg(test)]
mod tests;
