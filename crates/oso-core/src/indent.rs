//! Where a line's indentation goes, by the rules of its mode (ADR 0115): a buffer's
//! indentation setting, and for each mode a pure function over the rope that answers a
//! column, or "keep", for each line it is asked about. It reads no tree, so the answer is
//! the same whenever it is asked, in any buffer.
//!
//! Must not depend on: `buffer`, `motion`, `search`, `editor` or `command`; it reads a rope
//! and does no I/O. The editor chooses the rules by the buffer's mode.
//!
//! Each answer is bounded. Rust and TOML lex forward from an anchor, a line known to start
//! outside any string or comment, at most [`LINE_LOOKBACK`] back from the line; the lines of one
//! command share one pass, in order, and at most [`COMMAND_READ`] across all of them. Past
//! either, a new line takes the indentation of the nearest non-blank line before it, and an
//! existing line keeps its own.

mod fundamental;
mod markdown;
mod origin;
mod pass;
mod rust;
mod toml;

use crate::display_map::TabWidth;
use crate::rope::Rope;

pub use origin::{Origin, Origins, with_editorconfig};

/// How far back from a line its rules may start reading, in bytes: an anchor farther away
/// is not looked for (ADR 0115, as built).
pub const LINE_LOOKBACK: usize = 64 << 10;

/// What every line read costs at least, in bytes of the budget: reading a line costs about
/// as much as lexing this many bytes, so a run of short lines is bounded in time too. A
/// region of `lines` lines and `bytes` bytes needs at most `bytes + lines * LINE_COST`, and
/// what its first line's anchor is back.
pub const LINE_COST: usize = 32;

/// What one command may read across all its lines, in bytes: anchor searches, lexing and
/// the start of each line asked about (ADR 0115, as built).
pub const COMMAND_READ: usize = 1 << 20;

/// What the fallbacks of one command may read back, apart from the budget: past it, a new
/// line gets no indentation.
const FALLBACK_RESERVE: usize = 64 << 10;

/// Whether indentation is made of spaces, or of tabs then spaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    /// Spaces only, as ADR 0030 has it.
    #[default]
    Spaces,
    /// As many tabs as fit, then spaces (EditorConfig's `indent_style = tab`).
    Tabs,
}

/// A buffer's indentation setting: how deep a level is, what it is made of, and how wide
/// a tab is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Settings {
    indent_size: u8,
    style: Style,
    tab_width: TabWidth,
}

impl Settings {
    /// The largest `indent_size` and `tab_width` taken; larger values are refused, as
    /// EditorConfig's are (ADR 0115).
    pub const MAX: u32 = 32;

    /// A setting of `indent_size` columns a level and tabs `tab_width` columns wide, or
    /// none if either is not from 1 to [`Settings::MAX`].
    pub fn new(indent_size: u32, style: Style, tab_width: u32) -> Option<Self> {
        let valid = 1..=Self::MAX;
        if !valid.contains(&indent_size) || !valid.contains(&tab_width) {
            return None;
        }
        Some(Self {
            indent_size: u8::try_from(indent_size).ok()?,
            style,
            tab_width: TabWidth::new(tab_width),
        })
    }

    /// The columns of one level.
    pub fn indent_size(self) -> usize {
        usize::from(self.indent_size)
    }

    /// What indentation is made of.
    pub fn style(self) -> Style {
        self.style
    }

    /// The width of a tab.
    pub fn tab_width(self) -> TabWidth {
        self.tab_width
    }

    /// The text that indents a line to `column`: spaces, or with [`Style::Tabs`] as many
    /// tabs as fit, then spaces.
    pub fn indentation(self, column: usize) -> String {
        match self.style {
            Style::Spaces => " ".repeat(column),
            Style::Tabs => {
                let width = self.tab_width.get() as usize;
                let mut text = "\t".repeat(column / width);
                text.push_str(&" ".repeat(column % width));
                text
            }
        }
    }
}

impl Default for Settings {
    /// Four spaces a level, and tabs eight columns wide.
    fn default() -> Self {
        Self {
            indent_size: 4,
            style: Style::Spaces,
            tab_width: TabWidth::default(),
        }
    }
}

/// A line the rules are asked about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Line {
    /// An existing line, by its row, as `TAB` and `indent-region` reindent it. Its
    /// indentation as it stands is not counted.
    Row(usize),
    /// The new line that a line break inserted at this offset would start, as `RET`
    /// makes: the text before the offset is the line it leaves, the text after it,
    /// past spaces and tabs, the new line's.
    New(usize),
    /// The empty line a break at this offset would leave between the text before it and a
    /// new line holding the text after it, as `RET` between brackets opens (`{|}`): the text
    /// before the offset is the line it leaves, and nothing follows on it. Asked before
    /// `New` at the same offset, it is the line that `New`'s line leaves.
    Empty(usize),
}

/// Where a line's indentation goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Answer {
    /// Indent the line to this display column.
    Column(usize),
    /// Leave the line as it is: it starts inside a multi-line string, where spaces would
    /// change the string's value, or an existing line is past the budget. For a new line,
    /// insert no indentation and delete no space around the break.
    Keep,
}

/// What one command may still read across its lines. A command makes one and passes it
/// to each call; a line it runs out on gets the fallback answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    left: usize,
    ran_out: bool,
    /// What fallbacks may still read, apart from `left`.
    reserve: usize,
}

impl Budget {
    /// The budget of one command: [`COMMAND_READ`] bytes.
    pub const fn new() -> Self {
        Self::with_limit(COMMAND_READ)
    }

    /// A budget of `bytes`, as `indent-region` takes for a region it has checked the size of.
    pub const fn with_limit(bytes: usize) -> Self {
        Self {
            left: bytes,
            ran_out: false,
            reserve: FALLBACK_RESERVE,
        }
    }

    /// Takes `bytes`, or empties the budget and returns false if it holds fewer.
    fn spend(&mut self, bytes: usize) -> bool {
        let enough = bytes <= self.left;
        self.left = if enough { self.left - bytes } else { 0 };
        self.ran_out |= !enough;
        enough
    }

    /// Takes `bytes` from the fallbacks' reserve, or returns false if it holds fewer.
    fn spend_reserve(&mut self, bytes: usize) -> bool {
        let enough = bytes <= self.reserve;
        if enough {
            self.reserve -= bytes;
        }
        enough
    }

    /// Whether a line got the fallback answer because the budget ran out.
    pub const fn is_spent(self) -> bool {
        self.ran_out
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

/// The answers of Rust's rules for `lines`, in order (ADR 0115): by the innermost bracket
/// open at the line's start, block indent as rustfmt writes, and one level more for a line
/// that goes on with a statement. `lines` are in the order of where they start; one out
/// of order gets the fallback answer.
pub fn rust(rope: &Rope, settings: Settings, lines: &[Line], budget: &mut Budget) -> Vec<Answer> {
    pass::answers::<rust::Rust>(rope, settings, lines, budget)
}

/// The answers of TOML's rules for `lines`, as [`rust`]: inside a multi-line array or
/// inline table one level past its opener's line, and elsewhere the indentation of the
/// last line outside any bracket.
pub fn toml(rope: &Rope, settings: Settings, lines: &[Line], budget: &mut Budget) -> Vec<Answer> {
    pass::answers::<toml::Toml>(rope, settings, lines, budget)
}

/// The answers of Markdown's rules for `lines`, as [`rust`]: a new line goes to the content
/// column of the list item it leaves, or copies the indentation of the line it leaves. An
/// existing line keeps its indentation: `TAB` cycles through [`markdown_stops`] instead.
pub fn markdown(
    rope: &Rope,
    settings: Settings,
    lines: &[Line],
    budget: &mut Budget,
) -> Vec<Answer> {
    pass::answers::<markdown::Markdown>(rope, settings, lines, budget)
}

/// The columns `TAB` cycles line `row` of a Markdown buffer through: the content columns of
/// the list items it could belong to, from the innermost out, then 0. Past the budget,
/// only 0.
pub fn markdown_stops(
    rope: &Rope,
    settings: Settings,
    row: usize,
    budget: &mut Budget,
) -> Vec<usize> {
    markdown::stops(rope, settings, row, budget)
}

/// The answers of Fundamental mode's rules for `lines`: the indentation of the line before,
/// or 0 if that line is blank.
pub fn fundamental(
    rope: &Rope,
    settings: Settings,
    lines: &[Line],
    budget: &mut Budget,
) -> Vec<Answer> {
    pass::answers::<fundamental::Fundamental>(rope, settings, lines, budget)
}

#[cfg(test)]
mod fixed_point;
#[cfg(test)]
mod rust_tests;
pub(crate) use pass::MAX_COLUMN;

/// Whether each of `offsets`, ascending, of a Markdown buffer is inside a fenced code block,
/// within the budget: false past it.
pub(crate) fn markdown_fenced(rope: &Rope, settings: Settings, offsets: &[usize]) -> Vec<bool> {
    markdown::fenced(rope, settings, offsets, &mut Budget::new())
}

pub(crate) use markdown::{Item, Lists, Marker};

/// The list items open at each of `offsets`, ascending line starts of a Markdown buffer, and
/// whether each is inside a fenced code block, in one pass; none past the budget.
pub(crate) fn markdown_lists(
    rope: &Rope,
    settings: Settings,
    offsets: &[usize],
    budget: &mut Budget,
) -> Vec<Option<Lists>> {
    markdown::lists(rope, settings, offsets, budget)
}

/// The indentation that `pieces`, a line's text from its start in order, starts with: its
/// length in bytes and the display column it reaches. Spaces and tabs only, never U+3000, a
/// tab reaching the next stop of `tab_width`; the rules count indentation the same way.
pub(crate) fn indentation<'a>(
    pieces: impl IntoIterator<Item = &'a str>,
    tab_width: TabWidth,
) -> (usize, usize) {
    pass::lead(pieces, tab_width)
}

#[cfg(test)]
mod tests;
