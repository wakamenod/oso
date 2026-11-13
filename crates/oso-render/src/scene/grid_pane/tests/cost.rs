//! What a frame of a full terminal grid costs to build, with every row changed and with none
//! (ADR 0120). Run with `cargo test --release -p oso-render
//! -- --ignored a_full_grids_frame --nocapture`: it writes the times to stderr.
//!
//! Must not depend on: `capture` or `paint`. It shapes with the OS's font where there is one,
//! and with the fake text system where there is none; no GPU is touched.

use std::io::Write as _;
use std::time::{Duration, Instant};

use oso_platform::text::{self, FontRequest, GlyphFormat, GlyphKey, TextSystem};

use super::{grid_pane, plain};
use crate::atlas::AtlasGlyph;
use crate::pane::{Face, GridCell, GridColor, GridRow, GridStyle, GridView, PaneContent};
use crate::rows::tests::Recorder;
use crate::scene::tests::{rect, theme};
use crate::scene::{GlyphSource, Limits, Scene};

/// An atlas with a place for every glyph, which records nothing.
struct Places;

impl GlyphSource for Places {
    fn get(&mut self, _system: &mut dyn TextSystem, _key: GlyphKey) -> Option<AtlasGlyph> {
        Some(AtlasGlyph {
            format: GlyphFormat::Coverage,
            uv: [0, 0],
            size: [8, 12],
            left: 1,
            top: 12,
        })
    }

    fn take_full(&mut self) -> bool {
        false
    }

    fn clear(&mut self) {}
}

const COLUMNS: usize = 300;
const ROWS: usize = 100;
const FRAMES: u32 = 20;

/// A row of output of its own for `seed`: words of letters no other row repeats, one blank
/// between them. With `per_cell` a cell in three takes a colour or a style of its own, the
/// worst for shaping, and otherwise a word in four does, as a highlighter colours output.
fn line(seed: u64, per_cell: bool) -> Vec<GridCell> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let looks = [
        (GridColor::Face(Face::TerminalColor4), GridStyle::default()),
        (GridColor::Rgb([200, 120, 40]), GridStyle::default()),
        (GridColor::Default, GridStyle::BOLD),
    ];
    let mut cells = Vec::with_capacity(COLUMNS);
    while cells.len() < COLUMNS {
        let word = 2 + (next() % 9) as usize;
        let colour = next() % 12;
        for _ in 0..word {
            let random = next();
            let mut cell = plain(char::from(b'!' + (random % 90) as u8));
            let pick = if per_cell { random >> 8 } else { colour };
            if let Some(&(fg, style)) = looks.get((pick % 9) as usize) {
                (cell.fg, cell.style) = (fg, style);
            }
            cells.push(cell);
        }
        cells.push(plain(' '));
    }
    cells.truncate(COLUMNS);
    cells
}

/// The time a frame of the grid takes on average: every row of a new generation and other
/// text each frame if `changed`, or none.
fn time(system: &mut dyn TextSystem, (changed, per_cell): (bool, bool)) -> Duration {
    // Each frame's rows, and one more frame's to start from.
    let lines: Vec<Vec<_>> = (0..u64::from(FRAMES + 2) * ROWS as u64)
        .map(|seed| line(seed, per_cell))
        .collect();
    let limits = Limits {
        rects: crate::shape::MAX_RECTS,
        quads: 1 << 18,
    };
    let mut scene = Scene::new(limits, theme());
    let mut frame = |n: u32| {
        let n = if changed { n as usize } else { 0 };
        let rows: Vec<_> = (0..ROWS)
            .map(|row| GridRow {
                generation: n as u64,
                cells: &lines[n * ROWS + row],
                text: "",
                graphemes: &[],
            })
            .collect();
        let content = PaneContent::Grid(GridView {
            id: 1,
            rows: &rows,
            cursor: None,
            preedit: None,
        });
        scene.build(
            &[grid_pane(rect(0, 0, ROWS, COLUMNS), content)],
            system,
            &mut Places,
        );
    };
    frame(FRAMES + 1);
    let start = Instant::now();
    for n in 1..=FRAMES {
        frame(n);
    }
    start.elapsed() / FRAMES
}

#[test]
#[ignore = "measures; run in release with --nocapture"]
fn a_full_grids_frame() {
    let request = FontRequest {
        families: &["SF Mono", "Menlo"],
        size_pt: 13.0,
    };
    let mut system: Box<dyn TextSystem> = match text::load(&request, 2.0) {
        Ok(system) => system,
        Err(_) => Box::new(Recorder::default()),
    };
    let words = time(&mut *system, (true, false));
    let cells = time(&mut *system, (true, true));
    let none = time(&mut *system, (false, false));
    let mut err = std::io::stderr();
    let _ = writeln!(
        err,
        "{COLUMNS} x {ROWS} grid, a frame: every row changed {words:?} (a colour a word), \
         {cells:?} (a colour a cell); none changed {none:?}"
    );
    assert!(none < words && words < cells);
}
