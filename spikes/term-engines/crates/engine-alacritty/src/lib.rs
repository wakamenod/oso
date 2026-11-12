//! `alacritty_terminal` behind the comparison's `Engine` trait, with a second `vte` parser
//! for the sequences `Term` ignores (OSC 7, 133, 9, 777 and unknown OSCs), as ADR 0120
//! describes.

use std::cell::RefCell;
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Osc52, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{Color as AColor, NamedColor, Processor, StdSyncHandler};
use engine::{Cell, Color, Effect, Engine, Memory, Modes, Snapshot, flags};

/// Longest OSC content the second parser reports, as ADR 0120 caps it.
const OSC_CAP: usize = 4096;

#[derive(Clone, Default)]
struct Sink(Rc<RefCell<Vec<Effect>>>);

impl EventListener for Sink {
    fn send_event(&self, event: Event) {
        let effect = match event {
            Event::Title(title) => Effect::Title(title),
            Event::ResetTitle => Effect::Title(String::new()),
            Event::ClipboardStore(_, text) => Effect::ClipboardWrite(text.into_bytes()),
            Event::ClipboardLoad(..) => Effect::ClipboardRead,
            Event::ColorRequest(index, _) => Effect::ColorQuery(index as u16),
            Event::PtyWrite(text) => Effect::Reply(text.into_bytes()),
            Event::Bell => Effect::Bell,
            _ => return,
        };
        self.0.borrow_mut().push(effect);
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// The second parser's handler: reads only OSC strings.
struct OscReader(Rc<RefCell<Vec<Effect>>>);

/// OSCs `Term` handles itself (`vte::ansi`'s `osc_dispatch`).
const TERM_OSCS: &[&[u8]] = &[
    b"0", b"1", b"2", b"4", b"8", b"10", b"11", b"12", b"22", b"50", b"52", b"104", b"110", b"111",
    b"112",
];

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl alacritty_terminal::vte::Perform for OscReader {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        let Some(&number) = params.first() else {
            return;
        };
        let effect = match number {
            b"7" => Effect::WorkingDirectory(text(params.get(1).copied().unwrap_or_default())),
            b"133" => {
                let mark = params.get(1).copied().unwrap_or_default();
                let kind = mark.first().map_or('?', |&b| char::from(b));
                Effect::PromptMark {
                    kind,
                    params: params[2..]
                        .iter()
                        .map(|p| text(p))
                        .collect::<Vec<_>>()
                        .join(";"),
                }
            }
            b"9" if params.get(1).copied() == Some(b"4") => {
                let state = params
                    .get(2)
                    .and_then(|p| text(p).parse().ok())
                    .unwrap_or(0);
                let percent = params.get(3).and_then(|p| text(p).parse().ok());
                Effect::Progress { state, percent }
            }
            b"9" => Effect::Notification {
                title: String::new(),
                body: params[1..]
                    .iter()
                    .map(|p| text(p))
                    .collect::<Vec<_>>()
                    .join(";"),
            },
            b"777" if params.get(1).copied() == Some(b"notify") => Effect::Notification {
                title: text(params.get(2).copied().unwrap_or_default()),
                body: params
                    .get(3..)
                    .unwrap_or_default()
                    .iter()
                    .map(|p| text(p))
                    .collect::<Vec<_>>()
                    .join(";"),
            },
            n if TERM_OSCS.contains(&n) => return,
            _ => {
                let mut content = params.join(&b';');
                content.truncate(OSC_CAP);
                Effect::UnknownOsc(content)
            }
        };
        self.0.borrow_mut().push(effect);
    }
}

pub struct AlacrittyEngine {
    term: Term<Sink>,
    processor: Processor<StdSyncHandler>,
    osc_parser: alacritty_terminal::vte::Parser,
    osc_reader: OscReader,
    effects: Rc<RefCell<Vec<Effect>>>,
    no_second_parser: bool,
}

fn color(color: AColor) -> Color {
    match color {
        AColor::Named(named) if (named as usize) < 16 => Color::Indexed(named as u8),
        AColor::Named(NamedColor::Foreground | NamedColor::Background) => Color::Default,
        // Dim and cursor colors: resolved by the renderer from the theme.
        AColor::Named(named) => Color::Indexed((named as usize).min(255) as u8),
        AColor::Indexed(index) if index < 16 => Color::Indexed(index),
        AColor::Indexed(index) => Color::Indexed(index),
        AColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

impl AlacrittyEngine {
    fn fill_row(&self, out: &mut Snapshot, row: usize) {
        let cols = usize::from(out.cols);
        let line = &self.term.grid()[Line(row as i32)];
        let base = row * cols;
        for col in 0..cols {
            let cell = &line[Column(col)];
            let mut f = 0u16;
            let cf = cell.flags;
            if cf.contains(Flags::BOLD) {
                f |= flags::BOLD;
            }
            if cf.contains(Flags::DIM) {
                f |= flags::DIM;
            }
            if cf.contains(Flags::ITALIC) {
                f |= flags::ITALIC;
            }
            if cf.intersects(Flags::ALL_UNDERLINES) {
                f |= flags::UNDERLINE;
            }
            if cf.contains(Flags::INVERSE) {
                f |= flags::INVERSE;
            }
            if cf.contains(Flags::HIDDEN) {
                f |= flags::HIDDEN;
            }
            if cf.contains(Flags::STRIKEOUT) {
                f |= flags::STRIKE;
            }
            if cf.contains(Flags::WIDE_CHAR) {
                f |= flags::WIDE;
            }
            if cf.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                f |= flags::WIDE_SPACER;
            }
            if let Some(extra) = cell.zerowidth() {
                f |= flags::GRAPHEME;
                let mut s = String::with_capacity(8);
                s.push(cell.c);
                s.extend(extra);
                out.graphemes.push(((base + col) as u32, s));
            }
            if cell.hyperlink().is_some() {
                f |= flags::LINK;
            }
            out.cells[base + col] = Cell {
                ch: cell.c,
                fg: color(cell.fg),
                bg: color(cell.bg),
                flags: f,
            };
        }
    }
}

impl Engine for AlacrittyEngine {
    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let effects = Rc::new(RefCell::new(Vec::new()));
        let config = Config {
            scrolling_history: scrollback,
            kitty_keyboard: true,
            osc52: Osc52::CopyPaste,
            ..Config::default()
        };
        let size = Size {
            cols: usize::from(cols),
            rows: usize::from(rows),
        };
        let term = Term::new(config, &size, Sink(effects.clone()));
        Self {
            term,
            processor: Processor::new(),
            osc_parser: alacritty_terminal::vte::Parser::new(),
            osc_reader: OscReader(effects.clone()),
            effects,
            no_second_parser: std::env::var_os("NO_SECOND_PARSER").is_some(),
        }
    }

    fn name(&self) -> &'static str {
        "alacritty_terminal"
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        // `NO_SECOND_PARSER` measures what the second parser costs.
        if !self.no_second_parser {
            self.osc_parser.advance(&mut self.osc_reader, bytes);
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(Size {
            cols: usize::from(cols),
            rows: usize::from(rows),
        });
    }

    fn snapshot(&mut self, out: &mut Snapshot) {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;
        let full = out.cols != cols || out.rows != rows;
        if full {
            out.resize(cols, rows);
        }
        out.dirty.iter_mut().for_each(|d| *d = false);
        let damaged: Vec<usize> = match self.term.damage() {
            TermDamage::Full => (0..usize::from(rows)).collect(),
            TermDamage::Partial(lines) => {
                if full {
                    (0..usize::from(rows)).collect()
                } else {
                    lines.map(|l| l.line).collect()
                }
            }
        };
        if !damaged.is_empty() {
            let cols_usize = usize::from(cols);
            out.graphemes
                .retain(|(index, _)| !damaged.contains(&(*index as usize / cols_usize)));
        }
        for &row in &damaged {
            if row < usize::from(rows) {
                self.fill_row(out, row);
                out.dirty[row] = true;
            }
        }
        self.term.reset_damage();
        let point = self.term.grid().cursor.point;
        out.cursor_row = point.line.0.max(0) as u16;
        out.cursor_col = point.column.0 as u16;
        out.cursor_visible = self.term.mode().contains(TermMode::SHOW_CURSOR);
    }

    fn screen_text(&mut self) -> Vec<String> {
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .map(|row| {
                let line = &grid[Line(row as i32)];
                let mut s = String::new();
                for col in 0..grid.columns() {
                    let cell = &line[Column(col)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    s.push(cell.c);
                    if let Some(extra) = cell.zerowidth() {
                        s.extend(extra);
                    }
                }
                s.truncate(s.trim_end().len());
                s
            })
            .collect()
    }

    fn history_len(&mut self) -> usize {
        self.term.grid().history_size()
    }

    fn modes(&mut self) -> Modes {
        let mode = *self.term.mode();
        let mouse_tracking = if mode.contains(TermMode::MOUSE_MOTION) {
            1003
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            1002
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            1000
        } else {
            0
        };
        Modes {
            alt_screen: mode.contains(TermMode::ALT_SCREEN),
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            mouse_tracking,
            sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
            focus_events: mode.contains(TermMode::FOCUS_IN_OUT),
            cursor_visible: mode.contains(TermMode::SHOW_CURSOR),
            kitty_keyboard_flags: ((mode & TermMode::KITTY_KEYBOARD_PROTOCOL).bits() >> 18) as u8,
        }
    }

    fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut *self.effects.borrow_mut())
    }

    fn memory(&mut self) -> Memory {
        // Rows are allocated as the grid grows; each holds `columns` cells.
        let grid = self.term.grid();
        let lines = grid.total_lines() as u64;
        let cell = std::mem::size_of::<alacritty_terminal::term::cell::Cell>() as u64;
        Memory {
            grid_bytes: Some(lines * grid.columns() as u64 * cell),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> AlacrittyEngine {
        AlacrittyEngine::new(80, 24, 1000)
    }

    #[test]
    fn text_and_color() {
        let mut e = engine();
        e.feed(b"hello \x1b[31mX");
        assert_eq!(e.screen_text()[0], "hello X");
        let mut s = Snapshot::default();
        e.snapshot(&mut s);
        assert_eq!(s.row(0)[6].fg, Color::Indexed(1));
    }

    #[test]
    fn wide_and_grapheme() {
        let mut e = engine();
        e.feed("日e\u{301}".as_bytes());
        let mut s = Snapshot::default();
        e.snapshot(&mut s);
        assert_ne!(s.row(0)[0].flags & flags::WIDE, 0);
        assert_ne!(s.row(0)[1].flags & flags::WIDE_SPACER, 0);
        assert_ne!(s.row(0)[2].flags & flags::GRAPHEME, 0);
        assert_eq!(e.screen_text()[0], "日e\u{301}");
    }

    #[test]
    fn effects() {
        let mut e = engine();
        e.feed(b"\x1b]2;t\x07\x1b]7;file://h/tmp\x07\x1b]133;A\x07\x1b]9;4;1;50\x07");
        e.feed(b"\x1b]777;notify;T;B\x07\x1b[6n\x1b]1234;hi\x07");
        let fx = e.drain_effects();
        assert!(fx.contains(&Effect::Title("t".into())));
        assert!(fx.contains(&Effect::WorkingDirectory("file://h/tmp".into())));
        assert!(fx.contains(&Effect::PromptMark {
            kind: 'A',
            params: String::new()
        }));
        assert!(fx.contains(&Effect::Progress {
            state: 1,
            percent: Some(50)
        }));
        assert!(fx.contains(&Effect::Notification {
            title: "T".into(),
            body: "B".into()
        }));
        assert!(fx.contains(&Effect::Reply(b"\x1b[1;1R".to_vec())));
        assert!(fx.contains(&Effect::UnknownOsc(b"1234;hi".to_vec())));
    }

    #[test]
    fn modes_and_history() {
        let mut e = AlacrittyEngine::new(80, 10, 1000);
        for i in 0..100 {
            e.feed(format!("{i}\r\n").as_bytes());
        }
        assert_eq!(e.history_len(), 91);
        e.feed(b"\x1b[?1049h");
        assert!(e.modes().alt_screen);
    }
}
