//! A native session's to-do list (ADR 0165, part 3e; native-sessions-ux.md's proposal 14):
//! the items its main thread's `TaskCreate`, `TaskUpdate` and `TodoWrite` calls left, and the
//! strip that shows them, folded to one line or opened to [`OPEN_ROWS`].
//!
//! Must not depend on: threads, the stream or the UI; the caller gives each event and the
//! strip's width, and docks the text.
//!
//! - Only what the tools gave back counts: a call's result, matched with its call by id, that
//!   is no error. A subagent's calls are not the session's list.
//! - At most [`MAX_ITEMS`] items, each text cut to `wire::todo`'s 256 bytes as read; a task
//!   past the limit is not kept.
//! - The strip shows while any item is not completed, and once all are, until the turn ends:
//!   then the list goes. Nothing is read of the CLI's own task files: a session resumed shows
//!   no list until its next to-do call (the author's Q-E2 (a)).
//! - oso's words (`▸ Tasks (2/5)`, `… 4 more`, `C-c C-t list`) are in faces no item has; each
//!   item's text, which is Claude's, is after its mark (`[x]`, `[/]`, `[ ]`), in the mark's face,
//!   on cells of its own, its control characters as spaces, and cut to the strip's width with `…`,
//!   so none of it runs past the strip's edge, wraps, or stands where oso's keys show (ADR 0170).

use std::ops::Range;

use oso_core::display_map::text_clusters;
use oso_core::minibuffer::{cells_of, cut_to};

use super::render::Face;
use super::wire::todo::{Done, Update, Written};
use super::wire::{Block, Event};

pub use super::wire::todo::MAX_ITEMS;

/// The most rows the strip opens to, its head among them (ADR 0170's strip).
pub const OPEN_ROWS: usize = 6;
/// What the folded strip says opens it, and the opened one folds it.
const KEYS_LIST: &str = "C-c C-t list";
const KEYS_FOLD: &str = "C-c C-t fold";
/// The cells between the strip's head, what follows it and its keys.
const APART: usize = 2;
/// What an opened item's mark is after.
const INDENT: &str = "    ";
/// The fewest cells a folded item is cut to, its `…` among them, for the keys after it.
const MIN_ITEM: usize = 8;
/// The most calls waiting for their result that are kept.
const MAX_CALLS: usize = 64;
/// What a character that would join oso's text around an item's goes on, as a lone mark is
/// shown (`◌`), as the card does.
const BASE: char = '\u{25cc}';

/// Where an item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    InProgress,
    Completed,
}

impl Status {
    fn of(status: &str) -> Option<Self> {
        match status {
            "deleted" => None,
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => Some(Self::Pending),
        }
    }

    fn mark(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[/]",
            Self::Completed => "[x]",
        }
    }

    /// The face of an item's mark and text, which no word of oso's in the strip has.
    fn face(self) -> Face {
        match self {
            Self::Pending => Face::Target,
            Self::InProgress => Face::Waiting,
            Self::Completed => Face::Dim,
        }
    }
}

/// An item of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The task's id, or a `TodoWrite` item's place.
    pub id: String,
    pub text: String,
    pub status: Status,
}

/// A to-do call whose result has not come.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Create,
    Update(Option<Update>),
    Write,
}

/// A session's to-do list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Todo {
    items: Vec<Item>,
    calls: Vec<(String, Call)>,
}

/// The strip as text, with faces over byte ranges of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Strip {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
}

impl Strip {
    fn put(&mut self, text: &str, face: Option<Face>) {
        let start = self.text.len();
        self.text.push_str(text);
        if let Some(face) = face
            && !text.is_empty()
        {
            self.faces.push((start..self.text.len(), face));
        }
    }

    /// `text` of oso's, as plain text, or in `face`, cut to `room` cells.
    fn own(&mut self, text: &str, face: Option<Face>, room: usize) {
        if room == 0 {
            return;
        }
        let (kept, cut) = cut_to(text, room);
        self.put(kept, face);
        self.put(cut, Some(Face::Prompt));
    }

    /// An item's mark and its `text`, fenced, cut to `room` cells, which is not 0.
    fn item(&mut self, status: Status, text: &str, room: usize) {
        let face = Some(status.face());
        self.put(status.mark(), face);
        self.put(" ", None);
        let (kept, cut) = cut(text, room);
        self.put(kept, face);
        self.put(cut, Some(Face::Prompt));
    }

    /// `keys` at the right edge of a row of `width` cells, `used` of them taken, which leave it
    /// the room.
    fn keys(&mut self, keys: &str, used: usize, width: usize) {
        let gap = width.saturating_sub(used + cells_of(keys));
        self.put(&" ".repeat(gap), None);
        self.put(keys, Some(Face::Prompt));
    }
}

impl Todo {
    /// The items, in their order.
    #[cfg(test)]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Whether the strip shows: the list has an item.
    pub fn shows(&self) -> bool {
        !self.items.is_empty()
    }

    /// The list goes, as a resumed session's does. Whether it had any.
    pub fn clear(&mut self) -> bool {
        self.calls.clear();
        !std::mem::take(&mut self.items).is_empty()
    }

    /// Folds `event` into the list. Whether the list changed.
    pub fn apply(&mut self, event: &Event) -> bool {
        match event {
            Event::Assistant(assistant) if assistant.parent.is_none() => {
                for block in &assistant.blocks {
                    if let Block::ToolUse { id, name, input } = block {
                        self.called(id, name, input.task.clone());
                    }
                }
                false
            }
            Event::User(user) if user.parent.is_none() => match (&user.results[..], &user.todo) {
                ([result], Some(done)) if !result.error => self.done(&result.tool_use_id, done),
                _ => false,
            },
            // A turn's end: a list all done goes.
            Event::Result(_) => {
                self.calls.clear();
                let done = self
                    .items
                    .iter()
                    .all(|item| item.status == Status::Completed);
                done && self.clear()
            }
            _ => false,
        }
    }

    fn called(&mut self, id: &str, name: &str, update: Option<Update>) {
        let call = match name {
            "TaskCreate" => Call::Create,
            "TaskUpdate" => Call::Update(update),
            "TodoWrite" => Call::Write,
            _ => return,
        };
        if id.is_empty() || self.calls.iter().any(|(called, _)| called == id) {
            return;
        }
        if self.calls.len() >= MAX_CALLS {
            self.calls.remove(0);
        }
        self.calls.push((id.to_owned(), call));
    }

    /// Call `id` gave back `done`. Whether the list changed.
    fn done(&mut self, id: &str, done: &Done) -> bool {
        let Some(at) = self.calls.iter().position(|(called, _)| called == id) else {
            return false;
        };
        let (_, call) = self.calls.remove(at);
        match (call, done) {
            (Call::Create, Done::Created { id, subject }) => self.created(id, subject),
            (Call::Update(Some(update)), Done::Updated { id, subject, to })
                if *id == update.task =>
            {
                self.updated(
                    id,
                    subject.then_some(update.subject).flatten(),
                    to.as_deref(),
                )
            }
            (Call::Write, Done::Written(written)) => self.written(written),
            _ => false,
        }
    }

    fn created(&mut self, id: &str, subject: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            let changed = item.text != subject;
            subject.clone_into(&mut item.text);
            return changed;
        }
        if self.items.len() >= MAX_ITEMS {
            return false;
        }
        self.items.push(Item {
            id: id.to_owned(),
            text: subject.to_owned(),
            status: Status::Pending,
        });
        true
    }

    fn updated(&mut self, id: &str, subject: Option<String>, to: Option<&str>) -> bool {
        let Some(at) = self.items.iter().position(|item| item.id == id) else {
            return false;
        };
        let status = match to.map(Status::of) {
            Some(None) => {
                self.items.remove(at);
                return true;
            }
            Some(Some(status)) => Some(status),
            None => None,
        };
        let item = &mut self.items[at];
        let before = item.clone();
        if let Some(status) = status {
            item.status = status;
        }
        if let Some(subject) = subject {
            item.text = subject;
        }
        *item != before
    }

    fn written(&mut self, written: &[Written]) -> bool {
        // The same list again, as a `TodoWrite` gives it whole each time: nothing is made.
        let same = written.len() == self.items.len()
            && (written.iter().zip(&self.items)).all(|(written, item)| {
                written.content == item.text && Status::of(&written.status) == Some(item.status)
            });
        if same {
            return false;
        }
        let items: Vec<Item> = (written.iter().enumerate())
            .filter_map(|(at, written)| {
                Some(Item {
                    id: at.to_string(),
                    text: written.content.clone(),
                    status: Status::of(&written.status)?,
                })
            })
            .collect();
        let changed = items != self.items;
        self.items = items;
        changed
    }

    /// How many items are completed, of how many.
    fn counts(&self) -> (usize, usize) {
        let done = (self.items.iter())
            .filter(|item| item.status == Status::Completed)
            .count();
        (done, self.items.len())
    }

    /// The strip's head: `mark`, and how many tasks are completed of how many.
    fn head(&self, mark: char) -> String {
        let (done, total) = self.counts();
        format!("{mark} Tasks ({done}/{total})")
    }

    /// The strip folded, `width` cells wide: `▸ Tasks (2/5)  [/] ITEM`, the item in progress or
    /// else the first not started, and `C-c C-t list` at its right edge. With less room the keys
    /// are left out first, then the item.
    pub fn folded(&self, width: usize) -> Strip {
        let mut strip = Strip::default();
        let head = self.head('▸');
        let used = cells_of(&head);
        strip.own(&head, None, width);
        let keys = APART + cells_of(KEYS_LIST);
        let current = (self.items.iter())
            .find(|item| item.status == Status::InProgress)
            .or_else(|| (self.items.iter()).find(|item| item.status == Status::Pending));
        let Some(item) = current else {
            if used + keys <= width {
                strip.keys(KEYS_LIST, used, width);
            }
            return strip;
        };
        let before = used + APART + cells_of(item.status.mark()) + 1;
        let room = width.saturating_sub(before);
        // For the keys: the item whole, or enough of it.
        let text = fenced(&item.text);
        let least = MIN_ITEM.min(cells_of(&text));
        let keyed = room >= keys + least;
        let room = if keyed { room - keys } else { room };
        if room > 0 {
            strip.put(&" ".repeat(APART), None);
            strip.item(item.status, &text, room);
        }
        if keyed {
            let used = cells_of(&strip.text);
            strip.keys(KEYS_LIST, used, width);
        }
        strip
    }

    /// The strip opened, `width` cells wide: its head, with `C-c C-t fold` at its right edge, then a
    /// row of each item, its mark then its text, at most [`OPEN_ROWS`] in all; with more, the rows
    /// around the first not completed, and on the head how many more there are. With less room the
    /// keys are left out first, then how many more.
    pub fn opened(&self, width: usize) -> Strip {
        let mut strip = Strip::default();
        let items = &self.items;
        let shown = items.len().min(OPEN_ROWS - 1);
        let first = (items.iter())
            .position(|item| item.status != Status::Completed)
            .unwrap_or(0);
        let from = first.saturating_sub(1).min(items.len() - shown);
        let head = self.head('▾');
        let mut used = cells_of(&head);
        strip.own(&head, None, width);
        if shown < items.len() {
            let more = format!("… {} more", items.len() - shown);
            let cells = APART + cells_of(&more);
            if used + cells <= width {
                strip.put(&" ".repeat(APART), None);
                strip.put(&more, Some(Face::Prompt));
                used += cells;
            }
        }
        if used + APART + cells_of(KEYS_FOLD) <= width {
            strip.keys(KEYS_FOLD, used, width);
        }
        for item in &items[from..from + shown] {
            strip.put("\n", None);
            let before = cells_of(INDENT) + cells_of(item.status.mark()) + 1;
            match width.saturating_sub(before) {
                // Its mark alone, as far as it shows.
                0 => strip.own(
                    &format!("{INDENT}{}", item.status.mark()),
                    Some(item.status.face()),
                    width,
                ),
                room => {
                    strip.put(INDENT, None);
                    strip.item(item.status, &fenced(&item.text), room);
                }
            }
        }
        strip
    }
}

/// `text`, fenced, cut to `room` cells with `…` where it was cut, so that the `…` stays oso's
/// and one cell: a prepend the cut leaves last, which would take the `…` into its cluster, or
/// past a cluster's limit box it, goes too.
fn cut(text: &str, room: usize) -> (&str, &'static str) {
    let (mut kept, mut cut) = cut_to(text, room);
    while !cut.is_empty() && !kept.is_empty() {
        let joined = format!("{kept}{cut}");
        let last = (text_clusters(&joined, 0).last()).map_or(0, |last| last.offset);
        if last == kept.len() && cells_of(&joined) == cells_of(kept) + cells_of(cut) {
            break;
        }
        // The cluster the `…` went in, or with a limit between them, the last character.
        let before = kept.floor_char_boundary(kept.len() - 1);
        kept = &kept[..last.min(before)];
        cut = "…";
    }
    (kept, cut)
}

/// An item's text on one line, its control characters as spaces, as rows show Claude's text
/// (`render::one_line`, which also cuts it shorter), and as clusters of its own: a mark or a
/// joiner it starts with, which would join oso's space before it, and a character that joins
/// what follows (a prepend) it ends with, which would take oso's next cell, each go on `◌`.
/// So it is measured alone as the display draws it after oso's text.
fn fenced(text: &str) -> String {
    let mut text: String = (text.chars())
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let joins_before = text_clusters(&format!(" {text}"), 0)
        .next()
        .is_some_and(|cluster| cluster.text.len() > 1);
    if joins_before {
        text.insert(0, BASE);
    }
    let after = format!("{text} ");
    let joins_after = text_clusters(&after, 0)
        .last()
        .is_some_and(|cluster| cluster.offset < text.len());
    if joins_after {
        text.push(BASE);
    }
    text
}

#[cfg(test)]
mod tests;
