//! A graph's lanes (ADR 0127, step 4, Q9): which lane each commit takes and the box-drawing
//! cells of its row, from the commits in `git log --topo-order`'s order and their parents.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; [`Lanes::row`] is a pure
//! function of the rows before it, over untrusted ids: one that repeats or a parent never
//! listed draws no wrong line, and nothing here panics.
//!
//! - Each lane expects one commit. A commit takes the first lane that expects it, or the first
//!   free one; other lanes that expected it join it on its row (`╯`, `╰`). Its first parent
//!   goes on in its lane; each other parent joins a lane that already expects it, or forks into
//!   the first lane free before the row (`╮`, `╭`), so that no lane is reused on the row it
//!   ends in.
//! - A cell knows its lines up, down, left and right, and takes the box-drawing character of
//!   them, so that lines that cross draw `┼` and those that meet `├`, `┤`, `┬` or `┴`.
//! - At most [`MAX_LANES`] are drawn; past them one `┄` cell says there are more (Q5), a `●`
//!   there if the commit's own lane is past them.

/// The most lanes a row draws (Q5).
pub(crate) const MAX_LANES: usize = 16;
/// The most parents of one commit given lanes: an octopus merge past them joins its first ones.
pub(crate) const MAX_PARENTS: usize = 64;

/// The commit's own cell.
pub(crate) const MARKER: char = '●';
/// The cell past [`MAX_LANES`] lanes.
pub(crate) const MORE: char = '┄';

const UP: u8 = 1;
const DOWN: u8 = 2;
const RIGHT: u8 = 4;
const LEFT: u8 = 8;

/// One drawn cell: its character and the lane whose colour it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cell {
    pub(crate) ch: char,
    pub(crate) lane: usize,
}

/// The lanes open after the rows so far: each the commit it expects, or free.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lanes {
    expect: Vec<Option<String>>,
}

/// The lines of one row as it is worked out: each lane's cell, and each gap after a lane,
/// with the lane whose colour each takes.
struct Lines {
    cells: Vec<(u8, usize)>,
    gaps: Vec<Option<usize>>,
}

impl Lines {
    fn new(width: usize) -> Lines {
        Lines {
            cells: (0..width).map(|lane| (0, lane)).collect(),
            gaps: vec![None; width],
        }
    }

    fn grow(&mut self, width: usize) {
        while self.cells.len() < width {
            self.cells.push((0, self.cells.len()));
            self.gaps.push(None);
        }
    }

    /// A line across from the lane `from` to the lane `to`, in `colour`: it leaves `from`
    /// towards `to`, and ends at `to` coming from `from`'s side, with `vertical` there too.
    fn across(&mut self, from: usize, to: usize, vertical: u8, colour: usize) {
        if from == to {
            return;
        }
        let (low, high) = (from.min(to), from.max(to));
        self.grow(high + 1);
        for lane in low + 1..high {
            let (lines, own) = self.cells[lane];
            // A lane that passes keeps its colour; one that is free takes the line's.
            let own = if lines & (UP | DOWN) == 0 {
                colour
            } else {
                own
            };
            self.cells[lane] = (lines | LEFT | RIGHT, own);
        }
        for gap in low..high {
            self.gaps[gap] = Some(colour);
        }
        let (leaves, comes) = if to > from {
            (RIGHT, LEFT)
        } else {
            (LEFT, RIGHT)
        };
        self.cells[from].0 |= leaves;
        self.cells[to].0 |= comes | vertical;
        self.cells[to].1 = colour;
    }
}

impl Lanes {
    /// The row of the commit `id` with `parents`, first parent first: its cells, and the lane
    /// it takes.
    pub(crate) fn row(&mut self, id: &str, parents: &[String]) -> (Vec<Cell>, usize) {
        let hits: Vec<usize> = (self.expect.iter().enumerate())
            .filter(|(_, expected)| expected.as_deref() == Some(id))
            .map(|(lane, _)| lane)
            .collect();
        let lane = match hits.first() {
            Some(&lane) => lane,
            None => self.free_lane(&[]),
        };
        let mut lines = Lines::new(self.expect.len().max(lane + 1));
        for (other, expected) in self.expect.iter().enumerate() {
            if expected.is_some() && other != lane && !hits.contains(&other) {
                lines.cells[other].0 |= UP | DOWN;
            }
        }
        // The other lanes that expected it end in it.
        for &other in hits.iter().skip(1) {
            lines.across(other, lane, 0, other);
            lines.cells[other].0 |= UP;
            self.expect[other] = None;
        }
        let mut unique: Vec<&String> = Vec::new();
        for parent in parents.iter().take(MAX_PARENTS) {
            if !unique.contains(&parent) && parent != id {
                unique.push(parent);
            }
        }
        self.expect[lane] = unique.first().map(|parent| (*parent).clone());
        for parent in unique.iter().skip(1) {
            let joined = (self.expect.iter().enumerate())
                .find(|(other, expected)| *other != lane && expected.as_ref() == Some(*parent))
                .map(|(other, _)| other);
            match joined {
                Some(other) => lines.across(lane, other, 0, other),
                None => {
                    let fork = self.free_lane(&hits);
                    self.expect[fork] = Some((*parent).clone());
                    lines.across(lane, fork, DOWN, fork);
                }
            }
        }
        while self.expect.last().is_some_and(Option::is_none) {
            self.expect.pop();
        }
        (draw(&lines, lane), lane)
    }

    /// Whether some lane expects `id`.
    #[cfg(test)]
    pub(crate) fn expects(&self, id: &str) -> bool {
        self.expect
            .iter()
            .any(|expected| expected.as_deref() == Some(id))
    }

    /// The commits the open lanes expect.
    #[cfg(test)]
    pub(crate) fn expected(&self) -> Vec<&str> {
        self.expect.iter().flatten().map(String::as_str).collect()
    }

    /// The first free lane but those in `ended`, made if there is none.
    fn free_lane(&mut self, ended: &[usize]) -> usize {
        let free = (self.expect.iter().enumerate())
            .find(|(lane, expected)| expected.is_none() && !ended.contains(lane))
            .map(|(lane, _)| lane);
        free.unwrap_or_else(|| {
            self.expect.push(None);
            self.expect.len() - 1
        })
    }
}

/// The cells of `lines` with the commit's marker in `lane`, at most [`MAX_LANES`] lanes.
fn draw(lines: &Lines, lane: usize) -> Vec<Cell> {
    let mut width = lines.cells.len();
    while width > lane + 1 && lines.cells[width - 1].0 == 0 {
        width -= 1;
    }
    let shown = width.min(MAX_LANES);
    let mut out = Vec::with_capacity(2 * shown + 1);
    for index in 0..shown {
        let (bits, colour) = lines.cells[index];
        let ch = if index == lane {
            MARKER
        } else {
            box_char(bits)
        };
        // The commit's own cell keeps its lane's colour, whatever joins it.
        let colour = if index == lane { lane } else { colour };
        out.push(Cell { ch, lane: colour });
        let gap = lines.gaps.get(index).copied().flatten();
        out.push(match gap {
            Some(colour) if index + 1 < width => Cell {
                ch: '─',
                lane: colour,
            },
            _ => Cell {
                ch: ' ',
                lane: index,
            },
        });
    }
    if width > MAX_LANES {
        let ch = if lane >= MAX_LANES { MARKER } else { MORE };
        out.push(Cell {
            ch,
            lane: MAX_LANES,
        });
        out.push(Cell {
            ch: ' ',
            lane: MAX_LANES,
        });
    }
    out
}

/// The box-drawing character of a cell with `bits`' lines.
fn box_char(bits: u8) -> char {
    match bits {
        0 => ' ',
        b if b == UP | DOWN => '│',
        b if b == LEFT | RIGHT => '─',
        b if b == UP | LEFT => '╯',
        b if b == UP | RIGHT => '╰',
        b if b == DOWN | LEFT => '╮',
        b if b == DOWN | RIGHT => '╭',
        b if b == UP | DOWN | LEFT => '┤',
        b if b == UP | DOWN | RIGHT => '├',
        b if b == DOWN | LEFT | RIGHT => '┬',
        b if b == UP | LEFT | RIGHT => '┴',
        b if b == UP | DOWN | LEFT | RIGHT => '┼',
        b if b & (UP | DOWN) != 0 => '│',
        _ => '─',
    }
}

#[cfg(test)]
mod tests;
