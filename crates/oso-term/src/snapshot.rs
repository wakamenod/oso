//! What the UI reads of a terminal: the rows that changed since the last snapshot, the
//! cursor, the modes input depends on, and the events in between (ADR 0120, Q4). The grid is
//! as the engine placed it; oso does not measure widths again.
//!
//! Must not depend on: the engine, the policy or the thread.

use std::ops::Range;

use oso_platform::process::pty::Size;

use crate::event::Event;

/// One handoff from a terminal's thread to the UI. Only the rows that changed are in it; the
/// UI keeps the rest from earlier snapshots. A size change sends every row.
#[derive(Debug)]
pub struct Snapshot {
    pub size: Size,
    /// The changed rows, by index from the top of the screen, each a full row.
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    pub modes: Modes,
    /// The PTY does not echo: a password prompt (ADR 0120, Q17).
    pub echo_off: bool,
    /// Lines in the scrollback, above the screen.
    pub history: usize,
    /// Lines the view is scrolled back from the bottom, 0 at the bottom. While it is not 0,
    /// `rows` and `cursor` are of the lines scrolled to, and output keeps the view on them.
    pub scrolled: usize,
    pub events: Vec<Event>,
    /// Events left out because a snapshot holds at most 256.
    pub dropped_events: u32,
}

impl Snapshot {
    pub(crate) fn new(size: Size) -> Snapshot {
        Snapshot {
            size,
            rows: Vec::new(),
            cursor: Cursor::default(),
            modes: Modes::default(),
            echo_off: false,
            history: 0,
            scrolled: 0,
            events: Vec::new(),
            dropped_events: 0,
        }
    }

    /// Empties it for reuse, keeping its rows' allocations in `spare`.
    pub(crate) fn clear(&mut self, spare: &mut Vec<Row>) {
        spare.append(&mut self.rows);
        self.events.clear();
        self.dropped_events = 0;
    }
}

/// One screen row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub index: u16,
    pub cells: Vec<Cell>,
    /// The row's line goes on in the next row: a soft wrap.
    pub wrapped: bool,
    /// The text of every cell that holds more than one code point, one after another.
    pub text: String,
    /// Each such cell's column and its text's bytes in `text`, by column.
    pub graphemes: Vec<(u16, Range<u32>)>,
}

/// A cell. The second cell of a wide character is `Attrs::WIDE_SPACER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The first code point, or `' '` for an empty cell.
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub underline_color: Color,
    pub underline: Underline,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Cell {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            underline_color: Color::Default,
            underline: Underline::None,
            attrs: Attrs::default(),
        }
    }
}

/// A cell's color as the program set it. The theme draws `Default` and the 16 named colors;
/// the rest are drawn as given (ADR 0120).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// The theme's colors a terminal answers color queries from (OSC 4, 10, 11 and 12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    /// `terminal-color-0` to `terminal-color-15`.
    pub ansi: [Rgb; 16],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// A cell's attributes, as a set of flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attrs(u16);

impl Attrs {
    pub const BOLD: Attrs = Attrs(1 << 0);
    pub const DIM: Attrs = Attrs(1 << 1);
    pub const ITALIC: Attrs = Attrs(1 << 2);
    pub const BLINK: Attrs = Attrs(1 << 3);
    pub const INVERSE: Attrs = Attrs(1 << 4);
    pub const HIDDEN: Attrs = Attrs(1 << 5);
    pub const STRIKE: Attrs = Attrs(1 << 6);
    pub const OVERLINE: Attrs = Attrs(1 << 7);
    /// The first cell of a wide character.
    pub const WIDE: Attrs = Attrs(1 << 8);
    /// The cell after a wide character's first.
    pub const WIDE_SPACER: Attrs = Attrs(1 << 9);
    /// The cell holds more than one code point; its text is in `Row::text`, at the
    /// range `Row::graphemes` gives for its column.
    pub const GRAPHEME: Attrs = Attrs(1 << 10);

    /// Whether every flag of `other` is set.
    pub fn contains(self, other: Attrs) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) fn with(self, other: Attrs) -> Attrs {
        Attrs(self.0 | other.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: u16,
    pub column: u16,
    pub visible: bool,
    pub blinking: bool,
    pub shape: CursorShape,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    HollowBlock,
    Bar,
    Underline,
}

/// The modes oso's input and drawing depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub bracketed_paste: bool,
    pub focus_reports: bool,
    pub mouse: Mouse,
    pub sgr_mouse: bool,
    /// The kitty keyboard protocol's flags the program pushed; 0 for none.
    pub kitty_keyboard: u8,
}

/// How `Terminal::scroll` moves the view over the scrollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// By this many lines: negative is up, into the scrollback.
    Lines(isize),
    /// To the bottom, the live screen.
    Bottom,
}

/// Which mouse reports the program asked for (1000, 1002, 1003).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mouse {
    #[default]
    Off,
    Click,
    Drag,
    Motion,
}
