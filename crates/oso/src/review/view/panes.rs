//! A review's two panes on its view thread (ADR 0122, step 11): each file's section laid out
//! as an old pane and a new one in step, so that row `n` of one is beside row `n` of the other,
//! and both panes' whole texts, built only while the UI shows them or is about to.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this lays out what the view
//! thread holds.
//!
//! - Headers, notes and hunks' headers go in both panes. Each pane shows its own side's lines,
//!   with the sign the unified text has, and a hunk's removed and added lines are paired in
//!   order, as refinement pairs them.
//! - A card goes in the pane of its side, under its line, and a hunk's or an outdated one in the
//!   new pane: the other pane holds filler rows as tall, so that the panes stay in step.
//! - The text is cut at the same file in both panes, by both panes' bytes together, so that the
//!   two hold about what the unified text does.

use std::sync::Arc;

use oso_core::load::Reloaded;
use oso_core::review::{Hunk, RowKind, Rows, Shape, Side};
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};

use super::State;
use super::assemble::assemble;
use super::cards::{Cards, Placing, Plan};
use super::file::{Laidout, section_in};
use super::section::{self, Rendered, Shown, Writer};
use crate::review::{Content, Entry, Reading};

/// A pane's whole text, with what each of its lines is and its highlights.
pub(crate) type PaneText = (Reloaded, Rows, Highlights);

/// A file's section as the two panes show it: the old pane's, then the new one's.
pub(crate) struct Sides {
    pub(crate) old: Rendered,
    pub(crate) new: Rendered,
}

impl Sides {
    fn of(old: Writer, new: Writer) -> Sides {
        Sides {
            old: old.done(),
            new: new.done(),
        }
    }
}

impl Laidout for Sides {
    fn cards(&self) -> bool {
        self.old.cards || self.new.cards
    }

    fn cut(&self) -> bool {
        self.old.cut || self.new.cut
    }

    /// The bytes both panes hold, which the review's text limit counts together, so that the
    /// panes hold no more than the unified text does.
    fn len(&self) -> usize {
        self.old.bytes.len() + self.new.bytes.len()
    }
}

/// What the view keeps of the panes: whether it builds them, whether it must build them though
/// nothing else changed, and the texts it built last, which the next are compared with. A pane's
/// text always goes to the UI whole, so whether it held invalid bytes needs no keeping.
#[derive(Default)]
pub(super) struct Panes {
    on: bool,
    dirty: bool,
    texts: [Rope; 2],
}

/// Writes a filler row into whichever of `old` and `new` has fewer, until they have as many.
fn pad(old: &mut Writer, new: &mut Writer) {
    while old.rows() < new.rows() {
        old.line(RowKind::Filler, &[]);
    }
    while new.rows() < old.rows() {
        new.line(RowKind::Filler, &[]);
    }
}

/// Writes `note` into both panes.
fn note(old: &mut Writer, new: &mut Writer, note: &str) {
    old.note(note);
    new.note(note);
}

/// Writes a side's line, a run of its lines numbered from its own first line, which a card or
/// a filler row before it ends.
struct SideLines {
    side: Side,
    hunk: usize,
    first: u32,
    /// The pane's rows just after its last line of this side, if none came after it since.
    after: Option<usize>,
}

impl SideLines {
    fn write(&mut self, writer: &mut Writer, number: u32, line: &[u8]) {
        if self.after != Some(writer.rows()) {
            self.first = number;
        }
        let (sign, highlight) = match self.side {
            Side::Old => (b"-", Highlight::DiffRemoved),
            Side::New => (b"+", Highlight::DiffAdded),
        };
        let (text, _) = section::without_ending(line);
        let kind = RowKind::Lines {
            hunk: self.hunk,
            side: self.side,
            first: self.first,
        };
        writer.side_line(kind, (self.side, number), sign, text, highlight);
        self.after = Some(writer.rows());
    }
}

/// A hunk's header in both panes, then its removed and added lines beside each other, each
/// line's cards under it in its own pane, and the hunk's cards in the new pane.
fn hunk(
    (old, new): (&mut Writer, &mut Writer),
    plan: &mut Plan,
    (index, hunk): (usize, &Hunk),
    said: &str,
) {
    let header = crate::review::hunk_header(hunk);
    for writer in [&mut *old, &mut *new] {
        writer.line(
            RowKind::Hunk { hunk: index },
            &[
                (header.as_bytes(), Some(Highlight::DiffHunkHeader)),
                (said.as_bytes(), Some(Highlight::Decided)),
            ],
        );
    }
    let mut sides = [Side::Old, Side::New].map(|side| SideLines {
        side,
        hunk: index,
        first: 0,
        after: None,
    });
    let [old_lines, new_lines] = &mut sides;
    for at in 0..hunk.removed.len().max(hunk.added.len()) {
        let number = |start: u32| start.saturating_add(u32::try_from(at).unwrap_or(u32::MAX));
        if let Some(line) = hunk.removed.get(at) {
            old_lines.write(old, number(hunk.old_start), line);
        }
        if let Some(line) = hunk.added.get(at) {
            new_lines.write(new, number(hunk.new_start), line);
        }
        pad(old, new);
        if at < hunk.removed.len() {
            plan.after_line(old, Side::Old, number(hunk.old_start));
        }
        if at < hunk.added.len() {
            plan.after_line(new, Side::New, number(hunk.new_start));
        }
        pad(old, new);
    }
    plan.after_hunk(new, index);
    pad(old, new);
}

/// The shape of a text reading.
fn shape_of(reading: &Reading) -> Option<&Arc<Shape>> {
    match &reading.content {
        Content::Text { shape, .. } => Some(shape),
        _ => None,
    }
}

/// The panes of `reading` with its comments, given the bytes the review's text has `room` for,
/// as [`section::render`] lays out its section.
pub(crate) fn render(reading: &Reading, room: usize, shown: &Shown) -> Sides {
    let entry = &reading.entry;
    let cards = shown.cards;
    let (mut old, mut new) = (Writer::new(&entry.path), Writer::new(&entry.path));
    for writer in [&mut old, &mut new] {
        section::header(writer, entry, reading.counts(), cards, shape_of(reading));
    }
    let shape = shape_of(reading);
    let placing = Placing::of(shape, cards, shown.placed);
    let shape = shape.map(|shape| &**shape);
    let mut plan = Plan::new(&cards.comments, shape, placing, shown.folded, room);
    if room == 0 {
        note(
            &mut old,
            &mut new,
            "not shown: the review's text is past its limit",
        );
        if !matches!(reading.content, Content::Withheld) {
            plan.rest(&mut new);
        }
        pad(&mut old, &mut new);
        return Sides::of(old, new);
    }
    match (&reading.content, shape_of(reading)) {
        (Content::Text { .. }, Some(shape)) => {
            for writer in [&mut old, &mut new] {
                section::ending_notes(writer, shape.hunks());
            }
            if let Some(dropped) = section::dropped_note(cards) {
                note(&mut old, &mut new, &dropped);
            }
            if shown.cut_short {
                note(
                    &mut old,
                    &mut new,
                    "comments not carried in time: some show as outdated",
                );
            }
            plan.outdated(&mut new);
            pad(&mut old, &mut new);
            for (index, at) in shape.hunks().iter().enumerate() {
                let said = cards.decided(shape, index);
                hunk((&mut old, &mut new), &mut plan, (index, at), &said);
            }
            plan.rest(&mut new);
        }
        (content, _) => {
            if let Some(text) = section::content_note(entry, content) {
                note(&mut old, &mut new, &text);
            }
            // A file that may hold secrets shows only how many comments it has.
            if !matches!(content, Content::Withheld) {
                plan.rest(&mut new);
            }
        }
    }
    pad(&mut old, &mut new);
    Sides::of(old, new)
}

/// The panes of a file listed and not read yet: its header and a note in both.
pub(crate) fn unread(entry: &Entry, cards: &Cards) -> Sides {
    Sides {
        old: section::unread(entry, cards),
        new: section::unread(entry, cards),
    }
}

/// The panes of a file that left the review with comments on it: its header in both, and its
/// cards, outdated, in the new pane, as [`section::gone`] lays it out.
pub(crate) fn gone(path: &[u8], cards: &Cards, room: usize, folded: bool, withheld: bool) -> Sides {
    let new = section::gone(path, cards, room, folded, withheld);
    let mut old = Writer::new(path);
    let header = (new.bytes.split(|byte| *byte == b'\n').next()).unwrap_or_default();
    old.line(RowKind::File, &[(header, Some(Highlight::DiffFileHeader))]);
    let lines = new.section.lines();
    for _ in 1..lines {
        old.line(RowKind::Filler, &[]);
    }
    Sides {
        old: old.done(),
        new,
    }
}

impl State {
    /// Builds the panes from now on, or no more: the first text after it is told to build them,
    /// the first time or again, holds them whatever else changed.
    pub(super) fn show_panes(&mut self, on: bool) {
        // Asked for again, as the UI dropped them: the next text holds them too.
        if on {
            self.panes.dirty = true;
        }
        if !on && self.panes.on {
            // What they were laid out from goes too: the panes are kept only while built.
            self.panes = Panes::default();
            let files = self.files.values_mut().map(|file| &mut file.rendered.panes);
            let gone = self.gone.values_mut().map(|gone| &mut gone.rendered.panes);
            for kept in files.chain(gone) {
                *kept = None;
            }
        }
        self.panes.on = on;
    }

    /// Both panes' whole texts, laid out from each file's panes, those changed laid out again:
    /// `None` while the panes are not built, or when nothing in them changed.
    pub(super) fn panes_text(&mut self) -> Option<Box<[PaneText; 2]>> {
        if !self.panes.on || !(self.dirty || self.panes.dirty) {
            return None;
        }
        self.panes.dirty = false;
        let said = self.said_now();
        let title = Arc::new(Sides {
            old: section::title(&self.title, &said),
            new: section::title(&self.title, &said),
        });
        let mut size = title.len();
        let mut pairs = vec![title];
        let folded = self.folded;
        let filtered = self.filtering.on();
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            if filtered && file.judged.hidden() {
                continue;
            }
            let room = self.max_text.saturating_sub(size);
            let mut kept = file.rendered.panes.take();
            let pair = section_in(&mut kept, room, || file.render_panes(room, folded));
            file.rendered.panes = kept;
            size += pair.len();
            pairs.push(pair);
        }
        for (path, gone) in &mut self.gone {
            if filtered && gone.judged.hidden() {
                continue;
            }
            let room = self.max_text.saturating_sub(size);
            let (cards, withheld) = (&gone.cards, gone.withheld);
            let pair = section_in(&mut gone.rendered.panes, room, || {
                self::gone(path, cards, room, folded, withheld)
            });
            size += pair.len();
            pairs.push(pair);
        }
        let olds: Vec<&Rendered> = pairs.iter().map(|pair| &pair.old).collect();
        let news: Vec<&Rendered> = pairs.iter().map(|pair| &pair.new).collect();
        let mut texts = [olds, news]
            .into_iter()
            .enumerate()
            .map(|(side, sections)| {
                let built = assemble(&sections, &[], &self.panes.texts[side]);
                self.panes.texts[side] = built.rope;
                (built.text, built.rows, built.highlights)
            });
        let (Some(old), Some(new)) = (texts.next(), texts.next()) else {
            return None;
        };
        Some(Box::new([old, new]))
    }
}

#[cfg(test)]
#[path = "panes_tests.rs"]
mod tests;
