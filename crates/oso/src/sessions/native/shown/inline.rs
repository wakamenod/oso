//! The marks inside one line of a reply (ADR 0165, "The look: Markdown"): code spans, `*`
//! emphasis and links, matched on the line, and the pieces left once they are hidden.
//!
//! Must not depend on: anything but the line's text.
//!
//! - A code span first, then emphasis and links outside of one, and none in an address. What
//!   has no partner on its line stays, as does a `*` that could be a wildcard, a pointer or
//!   arithmetic: one opens only after a space, a bracket, a quotation mark or a letter that is
//!   not ASCII, and before what a word starts with; one closes only after what a word ends
//!   with, and before no ASCII letter or digit.
//! - A link's target is hidden only when the link's text is plain words, which no address can
//!   be written in, the target is a web address with no title and the only address of its
//!   line, and nothing next to the link or after it could read as a target; else it follows
//!   the text in parentheses, or is the text ([`super::signs`]).
//! - Each kind of mark is matched in one pass over the line: O(line), and a sort of the marks.

use std::ops::Range;

use super::Style;
use super::signs::{around_link, plain};

/// The longest run of backticks or asterisks read as a mark.
const MAX_TICKS: usize = 8;
const MAX_STARS: usize = 3;

/// The marks of a line.
#[derive(Default)]
pub(super) struct Marks<'a> {
    line: &'a str,
    from: usize,
    style: Option<Style>,
    /// A link's target may be hidden.
    may_hide: bool,
    cuts: Vec<Cut>,
    open: Vec<(usize, usize)>,
}

/// Bytes of the line not drawn, or drawn as `put`, and the styles that start or end there.
#[derive(Clone, Debug)]
struct Cut {
    range: Range<usize>,
    put: &'static str,
    span: Option<(Span, bool)>,
}

#[derive(Clone, Copy, Debug)]
enum Span {
    Code,
    Strong,
    Link,
}

impl<'a> Marks<'a> {
    /// Reads the marks of `line` from byte `from` on, whose text is of `style` outside any;
    /// `may_hide` unless what is beside the line could read as a link's target.
    pub(super) fn read(
        &mut self,
        line: &'a str,
        from: usize,
        style: Option<Style>,
        may_hide: bool,
    ) {
        (self.line, self.from, self.style, self.may_hide) = (line, from, style, may_hide);
        self.cuts.clear();
        self.open.clear();
        let bytes = line.as_bytes();
        // The runs of backticks of each length with none after them on the line.
        let mut alone = [false; MAX_TICKS + 1];
        let mut stars = [0usize; MAX_STARS + 1];
        let mut link = None;
        let mut at = from;
        while at < bytes.len() {
            match bytes[at] {
                b'\\' if bytes.get(at + 1).is_some_and(u8::is_ascii_punctuation) => at += 2,
                b'`' => at = self.code(at, &mut alone),
                b'*' => {
                    let len = run(bytes, at);
                    self.stars(at, len, &mut stars);
                    at += len;
                }
                b'[' => {
                    // An image is drawn as written.
                    link = (at == from || bytes[at - 1] != b'!').then_some(at);
                    at += 1;
                }
                b']' => match link.take().and_then(|open| self.link(open, at)) {
                    Some(end) => at = end,
                    // As written: nor is its target read.
                    None if bytes.get(at + 1) == Some(&b'(') => at = address_end(line, at + 2),
                    None => at += 1,
                },
                b':' if bytes[at..].starts_with(b"://") => at = address_end(line, at),
                _ => at += 1,
            }
        }
        self.cuts.sort_unstable_by_key(|cut| cut.range.start);
    }

    fn cut(&mut self, range: Range<usize>, put: &'static str, span: Option<(Span, bool)>) {
        self.cuts.push(Cut { range, put, span });
    }

    /// A code span from the backticks at `at`, or none; returns where to read on.
    fn code(&mut self, at: usize, alone: &mut [bool; MAX_TICKS + 1]) -> usize {
        let bytes = self.line.as_bytes();
        let len = run(bytes, at);
        let start = at + len;
        if len > MAX_TICKS || alone[len] {
            return start;
        }
        let mut end = start;
        while end < bytes.len() {
            if bytes[end] != b'`' {
                end += 1;
                continue;
            }
            let found = run(bytes, end);
            if found == len {
                let inside = &self.line[start..end];
                // A space at each end is the span's own: its code may start with a backtick.
                let padded = inside.len() > 2
                    && inside.starts_with(' ')
                    && inside.ends_with(' ')
                    && !inside.trim().is_empty();
                let pad = usize::from(padded);
                self.cut(at..start + pad, "", Some((Span::Code, true)));
                self.cut(end - pad..end + len, "", Some((Span::Code, false)));
                return end + len;
            }
            end += found;
        }
        alone[len] = true;
        start
    }

    /// A run of `len` asterisks at `at`: the end of an emphasis opened with as many, the start
    /// of one, or neither.
    fn stars(&mut self, at: usize, len: usize, count: &mut [usize; MAX_STARS + 1]) {
        if len > MAX_STARS {
            return;
        }
        let before = self.line[self.from..at].chars().next_back().unwrap_or(' ');
        let after = self.line[at + len..].chars().next().unwrap_or(' ');
        // What a word starts with; for `**`, what a name in bold may.
        let starts = after.is_ascii_alphanumeric()
            || (!after.is_ascii() && !after.is_whitespace())
            || matches!(after, '`' | '[')
            || (len > 1 && matches!(after, '"' | '\'' | '(' | '.' | '-' | '$' | '@' | '#' | '<'));
        let opens = starts
            && (matches!(before, ' ' | '\t' | '(' | '[' | '"' | '\'' | '_') || !before.is_ascii())
            && !(is_quote(before) && is_quote(after));
        let ends = before.is_ascii_alphanumeric()
            || (!before.is_ascii() && !before.is_whitespace())
            || matches!(before, '`' | ']' | ')' | '.' | '!' | '?' | ':' | '%' | '>')
            || (len > 1 && is_quote(before));
        let goes_on = after.is_ascii_alphanumeric()
            || matches!(after, '/' | '(' | '[')
            || (is_quote(before) && is_quote(after));
        let closes = ends && !goes_on;
        if closes && count[len] > 0 {
            // What opened since has no partner. Each is taken off once.
            while let Some((start, opened)) = self.open.pop() {
                count[opened] -= 1;
                if opened == len {
                    self.cut(start..start + len, "", Some((Span::Strong, true)));
                    self.cut(at..at + len, "", Some((Span::Strong, false)));
                    return;
                }
            }
        }
        if opens {
            self.open.push((at, len));
            count[len] += 1;
        }
    }

    /// A link whose text starts past the `[` at `open` and ends at the `]` at `close`; returns
    /// where it ends, `None` for no link.
    fn link(&mut self, open: usize, close: usize) -> Option<usize> {
        let text = &self.line[open + 1..close];
        let rest = self.line[close + 1..].strip_prefix('(')?;
        // No title, no escapes, no parentheses: any other is drawn as written.
        let len =
            rest.find(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '\\'))?;
        if !rest[len..].starts_with(')') || len == 0 || text.trim().is_empty() {
            return None;
        }
        let target = close + 2..close + 2 + len;
        let end = target.end + 1;
        self.cut(open..open + 1, "", Some((Span::Link, true)));
        if self.same(text, target.clone()) || self.hides(open, text, target.clone()) {
            self.cut(close..end, "", Some((Span::Link, false)));
        } else {
            self.cut(close..close + 2, " (", None);
            self.cut(target.end..target.end, "", Some((Span::Link, false)));
        }
        Some(end)
    }

    /// Whether a link's `text` is drawn as its `target` is written.
    fn same(&self, text: &str, target: Range<usize>) -> bool {
        text == &self.line[target] && !text.contains(['*', '`', '\\'])
    }

    /// Whether the `text` of a link that starts at `open` may stand for its `target`.
    fn hides(&self, open: usize, text: &str, target: Range<usize>) -> bool {
        let address = &self.line[target.clone()];
        let web = address.starts_with("https://") || address.starts_with("http://");
        self.may_hide
            && web
            && text.chars().all(plain)
            && self.line.matches("://").nth(1).is_none()
            && !self.line.contains("www.")
            && around_link(&self.line[..open], &self.line[target.end + 1..])
    }

    /// The line's pieces, in order: bytes of the line, what is drawn instead of them if not
    /// they, and the style.
    pub(super) fn pieces(
        &self,
    ) -> impl Iterator<Item = (Range<usize>, Option<&'static str>, Option<Style>)> + use<'_, 'a>
    {
        let mut at = self.from;
        let mut next = 0;
        let mut depth = [0usize; 3];
        let mut pending: Option<(Range<usize>, &'static str, Option<Style>)> = None;
        std::iter::from_fn(move || {
            if let Some((range, put, style)) = pending.take() {
                return Some((range, Some(put), style));
            }
            let style = style_of(&depth, self.style);
            let Some(cut) = self.cuts.get(next) else {
                let rest = at..self.line.len();
                at = self.line.len();
                return (!rest.is_empty()).then_some((rest, None, style));
            };
            let piece = at..cut.range.start;
            at = cut.range.end;
            next += 1;
            if !cut.put.is_empty() {
                pending = Some((cut.range.clone(), cut.put, self.style));
            }
            if let Some((span, starts)) = cut.span {
                let depth = &mut depth[span as usize];
                *depth = if starts {
                    *depth + 1
                } else {
                    depth.saturating_sub(1)
                };
            }
            Some((piece, None, style))
        })
    }
}

/// The style of text inside `depth` spans of each kind, `outside` any.
fn style_of(depth: &[usize; 3], outside: Option<Style>) -> Option<Style> {
    let inside = |span: Span| depth[span as usize] > 0;
    if inside(Span::Code) {
        Some(Style::Code)
    } else if inside(Span::Link) {
        Some(Style::Link)
    } else if outside.is_some() {
        outside
    } else {
        inside(Span::Strong).then_some(Style::Strong)
    }
}

/// Where the address, or what is written as a link's target, at byte `at` ends: at a space, a
/// backtick, a `](` or a Japanese stop or bracket.
fn address_end(line: &str, at: usize) -> usize {
    let rest = &line[at..];
    let end = rest.char_indices().find(|(index, c)| {
        c.is_whitespace()
            || matches!(c, '`' | '、' | '。' | '「' | '」' | '（' | '）')
            || rest[*index..].starts_with("](")
    });
    at + end.map_or(rest.len(), |(index, _)| index)
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'')
}

/// How many of the byte at `at` are there in a row.
fn run(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|b| **b == bytes[at]).count()
}
