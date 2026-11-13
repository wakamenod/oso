//! The engine's grid: text, wide characters and clusters, styles, which rows a snapshot
//! sends, sizes, scrollback and modes, and the count of rows copied without their styling.
//!
//! Must not depend on: the policy or the thread.

use oso_platform::process::pty::Size;

use super::Ghostty;
use crate::engine::Engine;
use crate::snapshot::{Attrs, Color, CursorShape, Mouse, Palette, Rgb, Row, Snapshot, Underline};

/// What `XTVERSION` answers in these tests; the policy's is the thread's.
pub(crate) const XTVERSION: &str = "oso test";

pub(crate) fn size(columns: u16, rows: u16) -> Size {
    Size {
        columns,
        rows,
        width: columns.saturating_mul(8),
        height: rows.saturating_mul(16),
    }
}

pub(crate) fn palette() -> Palette {
    let gray = |v| Rgb { r: v, g: v, b: v };
    let mut ansi = [gray(0); 16];
    for (i, color) in ansi.iter_mut().enumerate() {
        *color = Rgb {
            r: i as u8 * 10,
            g: 1,
            b: 2,
        };
    }
    Palette {
        foreground: gray(0xdd),
        background: gray(0x11),
        cursor: Some(Rgb {
            r: 0xff,
            g: 0,
            b: 0,
        }),
        ansi,
    }
}

pub(crate) fn engine(columns: u16, rows: u16) -> Ghostty {
    let mut engine = Ghostty::new(size(columns, rows), 1000, XTVERSION).expect("an engine");
    engine.set_palette(&palette());
    engine
}

/// Takes one snapshot, returning its rows.
pub(crate) fn snapshot(engine: &mut Ghostty) -> (Snapshot, bool) {
    let mut out = Snapshot::new(size(1, 1));
    let changed = engine.snapshot(&mut out, &mut Vec::new());
    (out, changed)
}

fn text(row: &Row) -> String {
    let mut line: String = row
        .cells
        .iter()
        .filter(|c| !c.attrs.contains(Attrs::WIDE_SPACER))
        .map(|c| c.ch)
        .collect();
    line.truncate(line.trim_end().len());
    line
}

#[test]
fn a_first_snapshot_sends_every_row_and_the_next_only_what_changed() {
    let mut e = engine(10, 4);
    e.feed(b"one\r\ntwo");
    let (first, changed) = snapshot(&mut e);
    assert!(changed);
    assert_eq!(first.rows.len(), 4);
    assert_eq!(text(&first.rows[0]), "one");
    assert_eq!(text(&first.rows[1]), "two");
    assert_eq!((first.cursor.row, first.cursor.column), (1, 3));
    let (idle, changed) = snapshot(&mut e);
    assert!(!changed);
    assert!(idle.rows.is_empty());
    e.feed(b"\r\nthree");
    let (next, _) = snapshot(&mut e);
    let indices: Vec<u16> = next.rows.iter().map(|r| r.index).collect();
    assert!(indices.contains(&2) && !indices.contains(&0), "{indices:?}");
}

#[test]
fn wide_characters_take_two_cells_and_clusters_keep_their_text() {
    let mut e = engine(10, 2);
    // Mode 2027 joins a flag's two regional indicators into one cluster.
    e.feed("\x1b[?2027h漢a\u{1F1EF}\u{1F1F5}e\u{301}".as_bytes());
    let (s, _) = snapshot(&mut e);
    let row = &s.rows[0];
    assert_eq!(row.cells[0].ch, '漢');
    assert!(row.cells[0].attrs.contains(Attrs::WIDE));
    assert!(row.cells[1].attrs.contains(Attrs::WIDE_SPACER));
    assert_eq!(row.cells[2].ch, 'a');
    assert!(row.cells[3].attrs.contains(Attrs::GRAPHEME));
    let texts: Vec<&str> = row
        .graphemes
        .iter()
        .map(|(_, at)| &row.text[at.start as usize..at.end as usize])
        .collect();
    assert!(texts.contains(&"\u{1F1EF}\u{1F1F5}"), "{texts:?}");
    assert!(texts.contains(&"e\u{301}"), "{texts:?}");
}

#[test]
fn the_longest_cluster_the_library_keeps_arrives_whole() {
    let mut e = engine(10, 1);
    // A 4-byte code point and 70 4-byte marks: the library keeps the first 64 marks.
    let mut text = String::from("\u{1F600}");
    text.extend(std::iter::repeat_n('\u{E0100}', 70));
    e.feed(text.as_bytes());
    let (s, _) = snapshot(&mut e);
    let row = &s.rows[0];
    assert!(
        row.cells[0].attrs.contains(Attrs::GRAPHEME),
        "{:?}",
        row.cells[0]
    );
    assert_eq!(row.graphemes.len(), 1);
    let kept: String = text.chars().take(65).collect();
    assert_eq!(row.text, kept);
    assert_eq!(row.text.len(), crate::limits::GRAPHEME_MAX);
}

#[test]
fn row_reads_out_of_their_order_read_nothing() {
    let mut vt = super::raw::Vt::new(10, 2, 100, XTVERSION).expect("a terminal");
    vt.write(b"ab");
    let mut raw = Vec::new();
    let mut buf = [0; crate::limits::GRAPHEME_MAX];
    // Before `frame`: no rows.
    assert!(!vt.next_row());
    assert!(!vt.row(&mut raw) && raw.is_empty());
    vt.update();
    let _ = vt.frame();
    assert!(vt.next_row());
    // On a row, before `row` points the cells at it: no cell.
    assert_eq!(vt.grapheme(0, &mut buf), None);
    vt.row(&mut raw);
    assert!(!raw.is_empty());
    // Writing changes the terminal: the rows end until the next `frame`.
    vt.write(b"c");
    assert!(!vt.next_row());
    assert!(!vt.row(&mut raw) && raw.is_empty());
}

/// Ordinary output never needs oso's patch to copy a row without its styling.
#[test]
fn ordinary_output_keeps_every_row_whole() {
    let mut e = engine(80, 24);
    for line in 0..500 {
        let row = format!(
            "\x1b[1;38;5;{}m{line}\x1b[0m \x1b]8;;https://example.org/{}\x1b\\link\x1b]8;;\x1b\\ e\u{301}👍\r\n",
            line % 256,
            line % 7
        );
        e.feed(row.as_bytes());
    }
    e.feed(b"\x1b[5;20r\x1b[10;1H\x1b[3L\x1b[2M\x1b[r");
    e.resize(size(40, 10));
    e.resize(size(120, 30));
    assert_eq!(e.vt.plain_copies(), Some(0));
    assert!(!e.warned_plain_copies);
}

#[test]
fn styles_reach_the_cells() {
    let mut e = engine(20, 2);
    e.feed(b"\x1b[1;3;4:3;38;5;196;48;2;1;2;3mx\x1b[0;7;9my\x1b[0;53;58;5;4mz");
    let (s, _) = snapshot(&mut e);
    let cells = &s.rows[0].cells;
    assert!(cells[0].attrs.contains(Attrs::BOLD));
    assert!(cells[0].attrs.contains(Attrs::ITALIC));
    assert_eq!(cells[0].underline, Underline::Curly);
    assert_eq!(cells[0].fg, Color::Indexed(196));
    assert_eq!(cells[0].bg, Color::Rgb(Rgb { r: 1, g: 2, b: 3 }));
    assert!(cells[1].attrs.contains(Attrs::INVERSE));
    assert!(cells[1].attrs.contains(Attrs::STRIKE));
    assert_eq!(cells[1].fg, Color::Default);
    assert!(cells[2].attrs.contains(Attrs::OVERLINE));
    assert_eq!(cells[2].underline_color, Color::Indexed(4));
}

#[test]
fn a_cleared_cell_keeps_only_its_background() {
    let mut e = engine(10, 2);
    e.feed(b"\x1b[44m\x1b[K\x1b[48;2;9;8;7m\x1b[5X");
    let (s, _) = snapshot(&mut e);
    let cells = &s.rows[0].cells;
    assert_eq!(cells[0].ch, ' ');
    assert_eq!(cells[0].bg, Color::Rgb(Rgb { r: 9, g: 8, b: 7 }));
    assert_eq!(cells[9].bg, Color::Indexed(4));
}

#[test]
fn soft_wraps_are_marked() {
    let mut e = engine(4, 3);
    e.feed(b"abcdef\r\ngh");
    let (s, _) = snapshot(&mut e);
    assert!(s.rows[0].wrapped);
    assert!(!s.rows[1].wrapped);
    assert_eq!(text(&s.rows[1]), "ef");
}

#[test]
fn a_resize_sends_every_row_at_the_new_size() {
    let mut e = engine(10, 3);
    e.feed(b"x");
    snapshot(&mut e);
    e.resize(size(6, 5));
    let (s, changed) = snapshot(&mut e);
    assert!(changed);
    assert_eq!(s.rows.len(), 5);
    assert!(s.rows.iter().all(|r| r.cells.len() == 6));
    assert_eq!((s.size.columns, s.size.rows), (6, 5));
}

#[test]
fn scrollback_keeps_the_lines_asked_for_and_not_a_small_default() {
    let mut e = Ghostty::new(size(80, 24), 10_000, XTVERSION).expect("an engine");
    for i in 0..5000 {
        e.feed(format!("line {i}\r\n").as_bytes());
    }
    let (s, _) = snapshot(&mut e);
    assert!(s.history >= 4900, "history {}", s.history);
    let mut small = Ghostty::new(size(80, 24), 100, XTVERSION).expect("an engine");
    for i in 0..5000 {
        small.feed(format!("line {i}\r\n").as_bytes());
    }
    let (s, _) = snapshot(&mut small);
    assert!(s.history < 1000, "history {}", s.history);
}

#[test]
fn scrollback_memory_is_bounded_and_plain_lines_all_fit() {
    let mut e = Ghostty::new(size(200, 24), 10_000, XTVERSION).expect("an engine");
    assert_eq!(e.memory_settings().0, crate::limits::SCROLLBACK_BYTES_MAX);
    for i in 0..10_000 {
        e.feed(format!("{i:x<199}\r\n").as_bytes());
    }
    let (s, _) = snapshot(&mut e);
    assert!(s.history >= 9_400, "history {}", s.history);
}

#[test]
fn modes_and_the_cursor_follow_the_program() {
    let mut e = engine(10, 3);
    e.feed(b"\x1b[?1049h\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[>1u\x1b[6 q");
    let (s, _) = snapshot(&mut e);
    assert!(s.modes.alternate_screen);
    assert!(s.modes.application_cursor);
    assert!(s.modes.bracketed_paste);
    assert!(s.modes.focus_reports);
    assert_eq!(s.modes.mouse, Mouse::Drag);
    assert!(s.modes.sgr_mouse);
    assert_eq!(s.modes.kitty_keyboard, 1);
    assert_eq!(s.cursor.shape, CursorShape::Bar);
    e.feed(b"\x1b[?25l\x1b[?1049l");
    let (s, _) = snapshot(&mut e);
    assert!(!s.cursor.visible);
    assert!(!s.modes.alternate_screen);
}

#[test]
fn spare_rows_are_reused() {
    let mut e = engine(10, 3);
    let mut out = Snapshot::new(size(1, 1));
    let mut spare = Vec::new();
    e.snapshot(&mut out, &mut spare);
    out.clear(&mut spare);
    assert_eq!(spare.len(), 3);
    e.feed(b"\x1b[2J");
    e.snapshot(&mut out, &mut spare);
    assert_eq!(out.rows.len(), 3);
    assert!(spare.is_empty());
}

#[test]
fn a_reused_row_keeps_no_cluster_text_of_its_last_use() {
    let mut e = engine(10, 1);
    let mut out = Snapshot::new(size(1, 1));
    let mut spare = Vec::new();
    e.feed("e\u{301}a\u{1F1EF}\u{1F1F5}".as_bytes());
    e.snapshot(&mut out, &mut spare);
    assert_eq!(out.rows[0].graphemes.len(), 1, "{:?}", out.rows[0]);
    out.clear(&mut spare);
    e.feed("\ru\u{308}".as_bytes());
    e.snapshot(&mut out, &mut spare);
    let row = &out.rows[0];
    assert_eq!(row.text, "u\u{308}");
    assert_eq!(row.graphemes, [(0, 0..3)]);
    assert!(spare.is_empty());
}

/// The snapshot and parse budgets of ADR 0120's Cost table, measured on this machine:
/// `cargo test --release -p oso-term --lib measure -- --ignored --nocapture`. Each case fills a
/// 300 × 100 screen, makes every row dirty, and times a full snapshot (p50 and p99 of 200).
#[test]
#[ignore = "a measurement, not a check"]
fn measure() {
    use std::io::Write;
    use std::time::Instant;
    type Line = Box<dyn Fn(usize) -> String>;
    let screens: [(&str, Line); 4] = [
        ("plain", Box::new(|i| format!("{:0>300}", i))),
        (
            "sgr per cell",
            Box::new(|i| {
                (0..300)
                    .map(|c| format!("\x1b[38;5;{}m{}", (c + i) % 256, 'x'))
                    .collect()
            }),
        ),
        (
            "a b a b styles",
            Box::new(|_| {
                (0..300)
                    .map(|c| if c % 2 == 0 { "\x1b[31mx" } else { "\x1b[32my" })
                    .collect()
            }),
        ),
        ("graphemes", Box::new(|_| "e\u{301}".repeat(300))),
    ];
    let mut err = std::io::stderr();
    for (name, line) in &screens {
        let mut e = Ghostty::new(size(300, 100), 1000, XTVERSION).expect("an engine");
        let mut times = Vec::new();
        let mut out = Snapshot::new(size(1, 1));
        let mut spare = Vec::new();
        for round in 0..200 {
            let mut screen = String::from("\x1b[H");
            for row in 0..100 {
                screen.push_str(&line(row + round));
                screen.push_str("\x1b[0m\r\n");
            }
            e.feed(screen.trim_end().as_bytes());
            e.resize(size(300, 100));
            let started = Instant::now();
            e.snapshot(&mut out, &mut spare);
            times.push(started.elapsed());
            out.clear(&mut spare);
        }
        times.sort();
        let _ = writeln!(
            err,
            "snapshot 300x100 {name}: p50 {:?} p99 {:?}",
            times[100], times[198]
        );
    }
    let plain: Vec<u8> = (0..1_000_000)
        .flat_map(|i: u32| format!("line {i} of plain text output here\r\n").into_bytes())
        .collect();
    let sgr: Vec<u8> = (0..400_000)
        .flat_map(|i: u32| {
            format!(
                "\x1b[1;3{}mab\x1b[0mcd \x1b[38;2;1;2;{}mef\x1b[0m\r\n",
                i % 8,
                i % 256
            )
            .into_bytes()
        })
        .collect();
    for (name, bytes) in [("plain", plain), ("sgr-heavy", sgr)] {
        let mut e = Ghostty::new(size(200, 50), 10_000, XTVERSION).expect("an engine");
        let started = Instant::now();
        for chunk in bytes.chunks(64 << 10) {
            e.feed(chunk);
        }
        let took = started.elapsed();
        let mb = bytes.len() as f64 / 1e6;
        let _ = writeln!(
            err,
            "parse {name}: {:.0} MB/s ({mb:.0} MB in {took:?})",
            mb / took.as_secs_f64()
        );
    }
}
