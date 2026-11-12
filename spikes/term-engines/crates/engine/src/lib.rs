//! The `Engine` trait both terminal engines implement for ADR 0120's comparison, and the
//! snapshot and effect types they share. This is the shape `oso_term::Engine` would take:
//! feed bytes, resize, publish a snapshot of changed rows, read rows as text, report effects.

/// A cell's color as the program set it. The theme resolves `Default` and the 16 named
/// colors; 256-color and true color are drawn as given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Cell attributes. Only what both engines can report, so the comparison is fair.
pub mod flags {
    pub const BOLD: u16 = 1 << 0;
    pub const DIM: u16 = 1 << 1;
    pub const ITALIC: u16 = 1 << 2;
    pub const UNDERLINE: u16 = 1 << 3;
    pub const INVERSE: u16 = 1 << 4;
    pub const HIDDEN: u16 = 1 << 5;
    pub const STRIKE: u16 = 1 << 6;
    /// The first cell of a wide character.
    pub const WIDE: u16 = 1 << 7;
    /// The cell after a wide character's first cell.
    pub const WIDE_SPACER: u16 = 1 << 8;
    /// The cell holds more than one code point; the text is in `Snapshot::graphemes`.
    pub const GRAPHEME: u16 = 1 << 9;
    /// The cell is part of an OSC 8 hyperlink.
    pub const LINK: u16 = 1 << 10;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The first code point, or ' ' for an empty cell.
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub flags: u16,
}

/// The visible screen as the UI would draw it. Built from damaged rows only: rows not
/// marked in `dirty` keep their cells from the last snapshot.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cols: u16,
    pub rows: u16,
    /// `rows * cols` cells, row-major.
    pub cells: Vec<Cell>,
    /// Which rows changed since the last snapshot.
    pub dirty: Vec<bool>,
    /// Full text of cells flagged `GRAPHEME`, by cell index.
    pub graphemes: Vec<(u32, String)>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_visible: bool,
}

impl Snapshot {
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.cells
            .resize(usize::from(cols) * usize::from(rows), Cell::default());
        self.dirty.clear();
        self.dirty.resize(usize::from(rows), true);
        self.graphemes.clear();
    }

    pub fn row(&self, row: u16) -> &[Cell] {
        let start = usize::from(row) * usize::from(self.cols);
        &self.cells[start..start + usize::from(self.cols)]
    }
}

/// Modes oso's input and drawing depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    pub alt_screen: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    /// 0 none, 1000 click, 1002 drag, 1003 motion.
    pub mouse_tracking: u16,
    pub sgr_mouse: bool,
    pub focus_events: bool,
    pub cursor_visible: bool,
    pub kitty_keyboard_flags: u8,
}

/// Something the engine would send to the program or to oso. The policy in `oso-term`
/// decides what to do with each one, the same for either engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Bytes the engine would write back to the PTY (device reports, mode reports).
    Reply(Vec<u8>),
    Title(String),
    Bell,
    /// OSC 7.
    WorkingDirectory(String),
    /// OSC 9 or OSC 777.
    Notification {
        title: String,
        body: String,
    },
    /// OSC 9;4: state and percent.
    Progress {
        state: u8,
        percent: Option<u8>,
    },
    /// OSC 133: the mark letter (A, B, C, D) and its parameters.
    PromptMark {
        kind: char,
        params: String,
    },
    /// OSC 52 write, decoded.
    ClipboardWrite(Vec<u8>),
    /// OSC 52 read request.
    ClipboardRead,
    /// OSC 4/10/11/12 color query.
    ColorQuery(u16),
    /// An OSC the engine does not know, its whole content (`number;rest`), capped at 4 KiB.
    UnknownOsc(Vec<u8>),
    /// An APC (Kitty graphics and others), capped.
    Apc(Vec<u8>),
}

/// What the comparison can learn of an engine's memory without guessing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    /// Bytes the engine reports or that can be computed from its grid; None when unknown.
    pub grid_bytes: Option<u64>,
}

pub trait Engine {
    /// `scrollback` is in lines.
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self
    where
        Self: Sized;

    fn name(&self) -> &'static str;

    /// Parse program output.
    fn feed(&mut self, bytes: &[u8]);

    fn resize(&mut self, cols: u16, rows: u16);

    /// Update `out` from the rows that changed since the last call, set `out.dirty`, and
    /// clear the engine's damage.
    fn snapshot(&mut self, out: &mut Snapshot);

    /// The visible rows as text, trailing blanks trimmed, wide spacers skipped.
    fn screen_text(&mut self) -> Vec<String>;

    /// Lines in scrollback, above the visible screen.
    fn history_len(&mut self) -> usize;

    fn modes(&mut self) -> Modes;

    /// Effects since the last call.
    fn drain_effects(&mut self) -> Vec<Effect>;

    fn memory(&mut self) -> Memory;
}
