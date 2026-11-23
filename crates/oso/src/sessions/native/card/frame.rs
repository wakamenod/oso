//! A permission card's frame (ADR 0165, step 2e-2; ADR 0180): its top, its bottom and the rows
//! between them as text and faces, and its keys as chips. The frame and the words on it are in
//! [`Face::CardTitle`] and the chips in [`Face::CardKey`], which Claude's text is never in.
//!
//! Must not depend on: the app or the UI; plain functions over text.

use std::ops::Range;

use oso_core::minibuffer::{cells_of, cut_to};

use super::rows::Row;
use crate::sessions::native::render::Face;

/// The fence before each row of Claude's text: the frame's left side.
pub const FENCE: &str = "│ ";
/// The fence before a row that goes on with the row above, broken at the card's width.
pub const FENCE_ON: &str = "┊ ";
/// The frame's right side, after a row of Claude's text.
pub(super) const SIDE: &str = " │";
/// The cells a row of the frame's top or bottom takes besides what it says: `╭─ `, a space and
/// `╮`.
const EDGE: usize = 5;
/// What ends what is cut.
const CUT: &str = "…";
/// The narrowest card drawn, in cells.
pub(super) const MIN_WIDTH: usize = 20;

/// A card as text, with faces over byte ranges of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shown {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
    /// How many rows it has.
    pub rows: usize,
}

impl Shown {
    fn put(&mut self, text: &str, face: Face) {
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        match self.faces.last_mut() {
            Some((last, same)) if *same == face && last.end == start => last.end = end,
            _ if start < end => self.faces.push((start..end, face)),
            _ => {}
        }
    }

    fn start_row(&mut self) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.rows += 1;
    }

    /// A row of the frame's top or bottom, `width` cells wide, between `corners`: what oso says
    /// there, `pieces`, if any, cut to the row with `…`.
    pub(super) fn edge(&mut self, corners: [&str; 2], pieces: &[(String, Face)], width: usize) {
        let width = width.max(MIN_WIDTH);
        let mut room = width - EDGE;
        let fits = cells_in(pieces) <= room;
        self.start_row();
        self.put(corners[0], Face::CardTitle);
        self.put(
            if pieces.is_empty() {
                "───"
            } else {
                "─ "
            },
            Face::CardTitle,
        );
        for (text, face) in pieces {
            let cells = cells_of(text);
            if fits || cells < room {
                self.put(text, *face);
                room -= cells.min(room);
                continue;
            }
            let (kept, _) = cut_to(text, room.saturating_sub(1));
            self.put(kept, *face);
            self.put(CUT, Face::CardTitle);
            room = room.saturating_sub(cells_of(kept) + 1);
            break;
        }
        self.put(if pieces.is_empty() { "" } else { " " }, Face::CardTitle);
        self.put(&"─".repeat(room), Face::CardTitle);
        self.put(corners[1], Face::CardTitle);
    }

    /// One fenced row in a card `width` cells wide: the fence, `│ ` or `┊ ` for one that goes on
    /// with the row above, then what oso says before the row's text, the text, and the frame's
    /// right side.
    pub(super) fn fenced_row(&mut self, row: &Row, face: Face, width: usize) {
        self.start_row();
        self.put(if row.on { FENCE_ON } else { FENCE }, Face::CardTitle);
        let mut cells = row.cells;
        for (text, face) in &row.lead {
            cells += cells_of(text);
            self.put(text, *face);
        }
        self.put(&row.text, face);
        let side = room_in(width).saturating_sub(cells);
        self.put(&" ".repeat(side), Face::Dim);
        self.put(SIDE, Face::CardTitle);
    }
}

/// The cells of Claude's text in a row of a card `width` cells wide, between the frame's sides.
pub(super) fn room_in(width: usize) -> usize {
    (width.max(MIN_WIDTH)).saturating_sub(cells_of(FENCE) + cells_of(SIDE))
}

/// The corners of the frame's top, of its bottom, and of a row of oso's between them.
pub(super) const TOP: [&str; 2] = ["╭", "╮"];
pub(super) const BOTTOM: [&str; 2] = ["╰", "╯"];
pub(super) const ACROSS: [&str; 2] = ["├", "┤"];

/// A key, what it does, and that in fewer cells.
pub(super) type Key = (&'static str, &'static str, &'static str);

/// What oso says, in pieces each in its face.
pub(super) type Pieces = Vec<(String, Face)>;

/// `keys` as rows of chips, each before what it does (ADR 0075's look), after `said`, in a card
/// `width` cells wide: one row where they fit it, in fewer words if they must, else two or more,
/// so that none is left out (ADR 0180).
pub(super) fn chips(said: &str, keys: &[Key], width: usize) -> Vec<Pieces> {
    let room = width.max(MIN_WIDTH) - EDGE;
    let rows = |short: bool| {
        let mut rows = vec![Pieces::new()];
        if !said.is_empty() {
            rows[0].push((said.to_owned(), Face::CardTitle));
        }
        for (key, does, shortly) in keys {
            let does = if short { shortly } else { does };
            let chip = [
                (format!(" {key} "), Face::CardKey),
                (format!(" {does}"), Face::CardTitle),
            ];
            let cells = cells_in(&chip);
            if let Some(row) = rows.last_mut() {
                let gap = usize::from(!row.is_empty());
                if row.is_empty() || cells_in(row) + gap + cells <= room {
                    if gap > 0 {
                        row.push((" ".to_owned(), Face::CardTitle));
                    }
                    row.extend(chip);
                    continue;
                }
            }
            rows.push(chip.to_vec());
        }
        rows
    };
    let long = rows(false);
    if long.len() == 1 {
        return long;
    }
    rows(true)
}

impl Shown {
    /// The card's last rows, `keys` as [`chips`] has them: the frame's bottom, and above it, where
    /// they take more than one, rows of oso's across it.
    pub(super) fn bottom(&mut self, keys: &[Pieces], width: usize) {
        for (at, row) in keys.iter().enumerate() {
            let last = at + 1 == keys.len();
            self.edge(if last { BOTTOM } else { ACROSS }, row, width);
        }
    }

    /// The card's first rows, `keys` as [`chips`] has them, as its bottom's.
    pub(super) fn top(&mut self, keys: &[Pieces], width: usize) {
        for (at, row) in keys.iter().enumerate() {
            self.edge(if at == 0 { TOP } else { ACROSS }, row, width);
        }
    }
}

/// The cells `pieces` take.
pub(super) fn cells_in(pieces: &[(String, Face)]) -> usize {
    pieces.iter().map(|(text, _)| cells_of(text)).sum()
}
