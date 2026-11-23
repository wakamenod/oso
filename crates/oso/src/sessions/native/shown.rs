//! A reply's Markdown as it is drawn (ADR 0165, "The look: Markdown"): its lines with their
//! marks hidden, a line at a time, so that a reply that streams is drawn as each of its lines
//! ends.
//!
//! Must not depend on: the syntax threads, the editor or the theme; a pure walk over a reply's
//! text, whose pieces say where in it they come from.
//!
//! - Hidden: a heading's `#`s, `*` around emphasis, the backticks of inline code, a fence's two
//!   lines, a link's brackets, and its target when [`inline`] says so. `- ` and `* ` are drawn
//!   `• `, a quote's `> ` as `│ `, a rule as a line. What is hidden leaves a style on what it
//!   was around.
//! - Drawn as written: tables, HTML, a block's indented code, `+ ` items, `_` and `~` emphasis,
//!   images, reference links, backslash escapes, a setext heading's underline, a fence in a
//!   quote or after a bullet, and a fence whose info string is not one short word. A mark that
//!   could be something else (a wildcard, arithmetic) stays.
//! - Code is never read for marks, and its lines keep their spaces.
//! - What Claude writes cannot pass for a row of oso's: a line that starts with a character
//!   that could be a row's mark ([`signs`]) is drawn as written, or in a fence behind a bar; and
//!   no [`Style`] has a highlight oso's own quiet, alert, prompt and diff rows have.
//! - A line that has not ended is drawn as written. Drawing a reply from any [`LineStart`]
//!   gives the same lines as drawing it whole.
//! - O(text): a line's marks are matched with no search that repeats. Past [`MAX_MARKED`] bytes
//!   of a line, [`MAX_TEXT`] of a reply, [`MAX_RULES`] rules or [`MAX_QUOTES`] quote marks,
//!   text is drawn as written, so that a crafted reply draws little more than it holds.

mod inline;
mod signs;

use signs::{could_be_mark, past_spaces, stands};

/// What a rule draws.
const RULE: &str = "────────────────────────────────";
/// What stands for a quote's `> `, and before a code line that could pass for a row.
const BAR: &str = "│ ";
/// What stands for a bullet's `- `.
const BULLET: &str = "• ";
/// The longest line read for marks, in bytes, as the syntax threads' (ADR 0111): a longer one
/// is drawn as written.
const MAX_MARKED: usize = 16 << 10;
/// How much of a reply is read for marks, in bytes: what starts past it is drawn as written.
const MAX_TEXT: usize = 256 << 10;
/// The most rules drawn, and the most of a line's quote marks: past them, as written, so that
/// a reply draws no more than a small part over what it holds.
const MAX_RULES: u8 = 64;
const MAX_QUOTES: usize = 8;
/// What blank lines and a line's ends are made of: no other space is dropped.
const BLANK: [char; 2] = [' ', '\t'];
/// The longest info string hidden with its fence.
const MAX_INFO: usize = 16;

/// A place in a reply where a line of its text starts: how many lines are drawn before it, and
/// what the lines before it say of those after.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineStart {
    /// The byte the line starts at.
    pub from: usize,
    /// The lines drawn before it.
    pub lines: usize,
    state: State,
}

/// What the lines so far say of the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct State {
    /// The fence that is open.
    fence: Option<Fence>,
    /// The line before has text.
    text: bool,
    /// A list's item came, and nothing at the margin since.
    list: bool,
    /// The lines before are a block's indented code.
    code: bool,
    /// A hidden fence ended just before.
    ended: bool,
    /// The rules drawn so far.
    rules: u8,
}

/// An open fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fence {
    tilde: bool,
    len: usize,
    indent: usize,
    /// Its two lines are drawn.
    drawn: bool,
    /// Its lines are a quote's.
    quoted: bool,
    /// The byte its first line starts at.
    start: usize,
}

/// What a stretch of a reply is, by the marks hidden around it or drawn for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Heading,
    Strong,
    Code,
    Link,
    /// What stands for a bullet, a number of a list, a quote or a rule.
    Mark,
}

/// A stretch of a drawn line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece<'a> {
    pub text: &'a str,
    /// Where in the reply's text it is; `None` for what oso puts.
    pub from: Option<usize>,
    pub style: Option<Style>,
    /// A fence's code, which takes its language's highlights: the byte its fence starts at.
    pub code: Option<usize>,
}

/// Draws `text` from `at` on: gives `line` each line drawn, in order, as its pieces, with
/// nothing at its end. `streams` if more text may come, so that a last line with no break is
/// drawn as written. Returns the mark the line that has not ended starts at, or the text's
/// end.
pub fn walk<'a>(
    text: &'a str,
    at: LineStart,
    streams: bool,
    mut line: impl FnMut(&[Piece<'a>]),
) -> LineStart {
    let mut mark = at;
    mark.from = text.floor_char_boundary(mark.from.min(text.len()));
    let mut out = Line::default();
    for whole in text[mark.from..].split_inclusive('\n') {
        // As `str::lines`: a line's break, and a `\r` just before it, are not its text.
        let ended = whole.strip_suffix('\n');
        let shown = ended.map_or(whole, |shown| shown.strip_suffix('\r').unwrap_or(shown));
        let from = mark.from;
        // A line that may go on changes nothing for what is drawn after it.
        let goes_on = streams && ended.is_none();
        let mut state = mark.state;
        let draws = out.read(shown, from, &mut state, goes_on);
        let mut lines = 0;
        if draws.gap {
            line(&[]);
            lines += 1;
        }
        if draws.line {
            line(&out.pieces);
            lines += 1;
        }
        if goes_on {
            return mark;
        }
        mark = LineStart {
            from: from + whole.len(),
            lines: mark.lines + lines,
            state,
        };
    }
    mark
}

/// What a line of the text draws: an empty line, then itself.
#[derive(Clone, Copy)]
struct Draws {
    gap: bool,
    line: bool,
}

/// The pieces of the line being read, kept from line to line.
#[derive(Default)]
struct Line<'a> {
    pieces: Vec<Piece<'a>>,
    inline: inline::Marks<'a>,
}

impl<'a> Line<'a> {
    fn put(
        &mut self,
        text: &'a str,
        from: Option<usize>,
        style: Option<Style>,
        code: Option<usize>,
    ) {
        if !text.is_empty() {
            self.pieces.push(Piece {
                text,
                from,
                style,
                code,
            });
        }
    }

    /// The whole line as written.
    fn raw(&mut self, line: &'a str, from: usize) {
        self.pieces.clear();
        self.put(line, Some(from), None, None);
    }

    /// Reads a line of the text into its pieces.
    fn read(&mut self, line: &'a str, from: usize, state: &mut State, goes_on: bool) -> Draws {
        self.pieces.clear();
        let blank = is_blank(line);
        // A hidden fence ended: an empty line sets the code apart from what follows.
        let gap = std::mem::take(&mut state.ended) && !blank;
        let quoted = line.trim_start().starts_with('>');
        if state.fence.is_some_and(|fence| fence.quoted && !quoted) {
            state.fence = None;
        }
        if let Some(fence) = state.fence {
            return self.in_fence(line, from, fence, state, goes_on);
        }
        if goes_on || line.len() > MAX_MARKED || from >= MAX_TEXT {
            self.raw(line, from);
            state.text = !blank;
            return Draws { gap, line: true };
        }
        let indent = line.len() - line.trim_start_matches(BLANK).len();
        let body = &line[indent..];
        let deep = deep(&line[..indent]) && !state.list;
        if let Some((fence, info)) = opens(body, indent).filter(|_| !deep) {
            let hidden = hidden_info(info);
            state.fence = Some(Fence {
                drawn: !hidden,
                start: from,
                ..fence
            });
            if hidden {
                // An empty line in its place, where none is.
                return Draws {
                    gap: gap || state.text,
                    line: false,
                };
            }
            self.raw(line, from);
            return Draws { gap, line: true };
        }
        self.prose(line, from, indent, deep, state);
        state.text = !blank;
        state.code &= blank || deep;
        Draws { gap, line: true }
    }

    /// Reads a line inside `fence`.
    fn in_fence(
        &mut self,
        line: &'a str,
        from: usize,
        fence: Fence,
        state: &mut State,
        goes_on: bool,
    ) -> Draws {
        let body = if fence.quoted {
            line.trim_start().trim_start_matches(['>', ' '])
        } else {
            line
        };
        let closes = !goes_on && closes(body, fence);
        if closes {
            state.fence = None;
            state.text = false;
            state.ended = !fence.drawn;
            if !fence.drawn {
                return Draws {
                    gap: false,
                    line: false,
                };
            }
        }
        if fence.drawn {
            // Its marks show: nothing of it needs a face.
            self.put(line, Some(from), None, None);
            return Draws {
                gap: false,
                line: true,
            };
        }
        // `>` too: it looks like a prompt's.
        if could_be_mark(line) || line.trim_start().starts_with('>') {
            self.put(BAR, None, Some(Style::Mark), None);
        }
        self.put(line, Some(from), Some(Style::Code), Some(fence.start));
        Draws {
            gap: false,
            line: true,
        }
    }

    /// Reads a line that is no fence's.
    fn prose(&mut self, line: &'a str, from: usize, indent: usize, deep: bool, state: &mut State) {
        let body = &line[indent..];
        if is_blank(body) {
            // An empty line is drawn empty, as no row ends in spaces.
            return;
        }
        if deep && (state.code || !state.text) {
            state.code = true;
            if could_be_mark(line) {
                self.put(BAR, None, Some(Style::Mark), None);
            }
            return self.put(line, Some(from), Some(Style::Code), None);
        }
        if indent == 0 && !state.text {
            state.list = false;
        }
        // A setext heading's underline, below text, is drawn as written.
        if !state.text && state.rules < MAX_RULES && is_rule(body) {
            state.rules += 1;
            self.put(&line[..indent], Some(from), None, None);
            return self.put(RULE, None, Some(Style::Mark), None);
        }
        self.put(&line[..indent], Some(from), None, None);
        let at = self.quote(line, indent);
        if is_blank(&line[at..]) {
            // A quote's empty line.
            return;
        }
        let quoted = at > indent;
        if let Some(level) = heading(&line[at..]) {
            self.inline.read(line, at + level, Some(Style::Heading));
            return self.inline_pieces(line, from);
        }
        let text = self.item(line, at, from);
        state.list |= text > at;
        let body = line[text..].trim_start_matches(' ');
        if let Some((fence, _)) = opens(body, indent).filter(|_| text > at || quoted) {
            // A fence in a quote, or after a bullet, is drawn.
            state.fence = Some(Fence {
                drawn: true,
                quoted,
                ..fence
            });
            return self.raw(line, from);
        }
        if body.starts_with('|') || html(body) {
            return self.put(&line[text..], Some(from + text), None, None);
        }
        self.inline.read(line, text, None);
        self.inline_pieces(line, from);
    }

    /// Draws a quote's marks from byte `at` of the line; returns where what it quotes starts.
    fn quote(&mut self, line: &'a str, mut at: usize) -> usize {
        // Where the line's text ends: spaces are looked through once.
        let end = line.trim_end_matches(BLANK).len();
        for _ in 0..MAX_QUOTES {
            let Some(rest) = line[at..].strip_prefix('>') else {
                break;
            };
            let bar = if at + 1 >= end { BAR.trim_end() } else { BAR };
            self.put(bar, None, Some(Style::Mark), None);
            at += 1 + usize::from(rest.starts_with(' '));
        }
        at
    }

    /// Draws a list item's mark at byte `at` of the line, if one is there, and the spaces
    /// around it; returns where its text starts, `at` if none is.
    fn item(&mut self, line: &'a str, at: usize, from: usize) -> usize {
        let body = line[at..].trim_start_matches(' ');
        let len = item(body);
        if len == 0 {
            return at;
        }
        let mark = line.len() - body.len();
        self.put(&line[at..mark], Some(from + at), None, None);
        let text = mark + len;
        if body.starts_with(['-', '*']) {
            // Its spaces past the first are drawn.
            self.put(BULLET, None, Some(Style::Mark), None);
            self.put(&line[mark + 2..text], Some(from + mark + 2), None, None);
        } else {
            self.put(
                &line[mark..text],
                Some(from + mark),
                Some(Style::Mark),
                None,
            );
        }
        text
    }

    /// Draws what the inline marks left; as written, a line that would start as a row does.
    fn inline_pieces(&mut self, line: &'a str, from: usize) {
        for (range, text, style) in self.inline.pieces() {
            let piece = Piece {
                text: text.unwrap_or(&line[range.clone()]),
                from: text.is_none().then_some(from + range.start),
                style,
                code: None,
            };
            if !piece.text.is_empty() {
                self.pieces.push(piece);
            }
        }
        // What oso put there, a bullet or a quote's bar, is no row's mark.
        let start = (self.pieces.iter())
            .find(|piece| piece.from.is_none() || !past_spaces(piece.text).is_empty());
        let passes = start.is_none_or(|piece| piece.from.is_some() && could_be_mark(piece.text));
        // As is one whose hidden marks would join what they were between.
        let joins = |text: Option<char>| text.is_some_and(|c| !stands(c));
        let joined = (self.pieces.windows(2)).any(|pair| {
            joins(pair[0].text.chars().next_back()) || joins(pair[1].text.chars().next())
        });
        if joined || passes {
            self.raw(line, from);
        }
    }
}

/// Whether `text` has nothing but spaces and tabs.
fn is_blank(text: &str) -> bool {
    text.trim_start_matches(BLANK).is_empty()
}

/// Whether `spaces`, a line's indent, makes it indented code outside a list.
fn deep(spaces: &str) -> bool {
    spaces.contains('\t') || spaces.len() >= 4
}

/// The fence `body`, a line past its indent, opens, and its info string.
fn opens(body: &str, indent: usize) -> Option<(Fence, &str)> {
    let mark = body.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let info = body.trim_start_matches(mark);
    let len = body.len() - info.len();
    // Three backticks on one line with their end are inline code.
    if len < 3 || (mark == '`' && info.contains('`')) {
        return None;
    }
    let fence = Fence {
        tilde: mark == '~',
        len,
        indent,
        drawn: false,
        quoted: false,
        start: 0,
    };
    Some((fence, info.trim_matches(BLANK)))
}

/// Whether `line` ends `fence`.
fn closes(line: &str, fence: Fence) -> bool {
    let body = line.trim_start_matches(' ');
    let indent = line.len() - body.len();
    let rest = body.trim_start_matches(if fence.tilde { '~' } else { '`' });
    body.len() - rest.len() >= fence.len && is_blank(rest) && indent <= fence.indent + 3
}

/// Whether a fence's info string goes with its line: none, or one short word, a language.
fn hidden_info(info: &str) -> bool {
    info.len() <= MAX_INFO
        && (info.bytes())
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'#' | b'-' | b'_'))
}

/// The bytes of an ATX heading's `#`s and spaces, `None` for no heading or an empty one.
fn heading(body: &str) -> Option<usize> {
    let rest = body.trim_start_matches('#');
    let level = body.len() - rest.len();
    let text = rest.trim_start_matches(' ');
    ((1..=6).contains(&level) && text.len() < rest.len() && !text.trim().is_empty())
        .then(|| body.len() - text.len())
}

/// The bytes of a list item's mark and spaces, 0 if `body` starts none or is an empty one.
fn item(body: &str) -> usize {
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let rest = match digits {
        0 => body.strip_prefix(['-', '*', '+']),
        1..=9 => body[digits..].strip_prefix(['.', ')']),
        _ => None,
    };
    let Some(rest) = rest else {
        return 0;
    };
    let text = rest.trim_start_matches(' ');
    if text.len() == rest.len() || text.trim().is_empty() {
        return 0;
    }
    body.len() - text.len()
}

/// Whether `body` is a rule: three or more of one of `-`, `*` and `_`, and spaces.
fn is_rule(body: &str) -> bool {
    let Some(mark) = body.chars().next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    body.chars().all(|c| c == mark || c == ' ') && body.chars().filter(|c| *c == mark).count() >= 3
}

/// Whether `body` starts an HTML tag or a comment.
fn html(body: &str) -> bool {
    let mut chars = body.chars();
    chars.next() == Some('<')
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'))
        && !body.starts_with("<http")
}

#[cfg(test)]
mod tests;
