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
//! - oso's marks (`☐ 2/5 ▶`, `☑`, `C-c C-t list`) are in faces of oso's own; each item's text,
//!   which is Claude's, is after them in a face of its own, its control characters as spaces,
//!   and cut to the strip's width with `…`, so none of it runs past the strip's edge, wraps,
//!   or stands where oso's keys are (ADR 0170).

use std::ops::Range;

use oso_core::display_map::text_clusters;
use oso_core::minibuffer::{cells_of, cut_to};

use super::render::Face;
use super::wire::todo::{Done, Update, Written};
use super::wire::{Block, Event};

pub use super::wire::todo::MAX_ITEMS;

/// The most rows the strip opens to (ADR 0170's strip).
pub const OPEN_ROWS: usize = 6;
/// What the folded strip says opens it.
pub const KEYS: &str = "C-c C-t list";
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
            Self::Pending => "☐",
            Self::InProgress => "▶",
            Self::Completed => "☑",
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
    fn put(&mut self, text: &str, face: Face) {
        let start = self.text.len();
        self.text.push_str(text);
        if !text.is_empty() {
            self.faces.push((start..self.text.len(), face));
        }
    }

    fn row(&mut self) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
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

    /// The strip folded, `width` cells wide: `☐ 2/5 ▶ ITEM`, the item in progress or else the
    /// first not started, and `C-c C-t list` at its right edge, where there is room for it.
    pub fn folded(&self, width: usize) -> Strip {
        let mut strip = Strip::default();
        let (done, total) = self.counts();
        let mark = if done == total { "☑" } else { "☐" };
        let count = format!("{mark} {done}/{total}");
        let current = (self.items.iter())
            .find(|item| item.status == Status::InProgress)
            .or_else(|| (self.items.iter()).find(|item| item.status == Status::Pending));
        let used = cells_of(&count) + current.map_or(0, |item| cells_of(item.status.mark()) + 2);
        let keys = cells_of(KEYS) + 2;
        let (room, keyed) = match width.checked_sub(used + keys) {
            Some(room) if room > 0 => (room, true),
            _ => (width.saturating_sub(used), false),
        };
        strip.put(&count, Face::Dim);
        let mut shown = 0;
        if let Some(item) = current.filter(|_| room > 0) {
            strip.put(&format!(" {} ", item.status.mark()), Face::Dim);
            let text = fenced(&item.text);
            let (kept, cut) = cut(&text, room);
            strip.put(kept, Face::Target);
            strip.put(cut, Face::Dim);
            shown = cells_of(kept) + cells_of(cut);
        }
        if keyed {
            strip.put(&" ".repeat(room - shown.min(room) + 2), Face::Dim);
            strip.put(KEYS, Face::Prompt);
        }
        strip
    }

    /// The strip opened, `width` cells wide: a row of each item, its mark then its text, at
    /// most [`OPEN_ROWS`]; with more, the rows around the first not completed, and how many
    /// more there are.
    pub fn opened(&self, width: usize) -> Strip {
        let mut strip = Strip::default();
        let items = &self.items;
        let (from, shown) = if items.len() <= OPEN_ROWS {
            (0, items.len())
        } else {
            let first = (items.iter())
                .position(|item| item.status != Status::Completed)
                .unwrap_or(0);
            let shown = OPEN_ROWS - 1;
            (first.saturating_sub(1).min(items.len() - shown), shown)
        };
        for item in &items[from..from + shown] {
            let room = width.saturating_sub(cells_of(item.status.mark()) + 1);
            strip.row();
            strip.put(item.status.mark(), Face::Dim);
            if room == 0 {
                continue;
            }
            strip.put(" ", Face::Dim);
            let text = fenced(&item.text);
            let (kept, cut) = cut(&text, room);
            strip.put(kept, Face::Target);
            strip.put(cut, Face::Dim);
        }
        if shown < items.len() {
            strip.row();
            let more = format!("… {} more", items.len() - shown);
            strip.put(cut_to(&more, width).0, Face::Dim);
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
        let apart = cells_of(&joined) == cells_of(kept) + cells_of(cut)
            && (text_clusters(&joined, 0).last()).is_some_and(|last| last.offset == kept.len());
        if apart {
            break;
        }
        kept = &kept[..kept.floor_char_boundary(kept.len() - 1)];
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
