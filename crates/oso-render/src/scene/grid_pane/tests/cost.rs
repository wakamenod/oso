//! What a frame of a full terminal grid costs to build, with every row changed and with none
//! (ADR 0120). Run with `cargo test --release -p oso-render
//! -- --ignored a_full_grids_frame --nocapture`: it writes the times to stderr. The three grids
//! take turns frame by frame, so a busy machine slows all three alike, and their order is
//! judged by each one's fastest frame: being preempted only adds time.
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

/// A grid that shows every row of a new generation and other text each frame if `changed`, or
/// the same rows.
struct Grid {
    scene: Scene,
    /// Each frame's rows, and one more frame's to start from.
    lines: Vec<Vec<GridCell>>,
    changed: bool,
}

impl Grid {
    fn new((changed, per_cell): (bool, bool)) -> Grid {
        let limits = Limits {
            rects: crate::shape::MAX_RECTS,
            quads: 1 << 18,
        };
        Grid {
            scene: Scene::new(limits, theme()),
            lines: (0..u64::from(FRAMES + 2) * ROWS as u64)
                .map(|seed| line(seed, per_cell))
                .collect(),
            changed,
        }
    }

    /// Builds frame `n` and says what it took.
    fn frame(&mut self, system: &mut dyn TextSystem, n: u32) -> Duration {
        let n = if self.changed { n as usize } else { 0 };
        let rows: Vec<_> = (0..ROWS)
            .map(|row| GridRow {
                generation: n as u64,
                cells: &self.lines[n * ROWS + row],
                text: "",
                graphemes: &[],
            })
            .collect();
        let content = PaneContent::Grid(GridView {
            id: 1,
            rows: &rows,
            cursor: None,
            preedit: None,
            preedit_at: (0, 0),
        });
        let start = Instant::now();
        self.scene.build(
            &[grid_pane(rect(0, 0, ROWS, COLUMNS), content)],
            system,
            &mut Places,
        );
        start.elapsed()
    }
}

/// The median frame and the fastest.
fn median_and_fastest(mut times: Vec<Duration>) -> (Duration, Duration) {
    times.sort_unstable();
    (times[times.len() / 2], times[0])
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
    // A colour a word, a colour a cell, and none changed.
    let mut grids = [(true, false), (true, true), (false, false)].map(Grid::new);
    for grid in &mut grids {
        grid.frame(&mut *system, FRAMES + 1);
    }
    let mut times: [Vec<Duration>; 3] = Default::default();
    for n in 1..=FRAMES {
        for (grid, times) in grids.iter_mut().zip(&mut times) {
            times.push(grid.frame(&mut *system, n));
        }
    }
    let [words, cells, none] = times.map(median_and_fastest);
    let mut err = std::io::stderr();
    let _ = writeln!(
        err,
        "{COLUMNS} x {ROWS} grid, a frame's median (fastest): every row changed {:?} ({:?}) \
         (a colour a word), {:?} ({:?}) (a colour a cell); none changed {:?} ({:?})",
        words.0, words.1, cells.0, cells.1, none.0, none.1
    );
    assert!(none.1 < words.1 && words.1 < cells.1);
}
