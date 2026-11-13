//! A review's view thread, `oso-review-view` (ADR 0122, "The views"): it holds each file's
//! current reading, lays out each file's section, and builds the review buffer's whole text,
//! its rows and its highlights, compared with the text before, for the UI to put in at O(1).
//!
//! Must not depend on: `app`, `dispatch`, winit, git or the file system; the caller hands in
//! what the review's own thread read, and takes texts out.
//!
//! - One thread per review, started and dropped with it. It takes everything waiting before it
//!   builds, so a burst of readings costs one text.
//! - One text is in flight at a time: the next is built only once the UI took the last
//!   ([`View::taken`]), and what is told meanwhile is gathered for it. So a review whose texts
//!   wait under a prompt holds one text, not one per batch, and each text is compared with the
//!   one the UI put in last.
//! - Each text is whole: every section is kept as bytes and laid end to end through
//!   `load::Builder`, which keeps invalid bytes for `\xNN` as a loaded file's, and compared
//!   with the text before so the UI swaps in only the span that changed. When no section, line
//!   under the title or listing changed, nothing is laid out: the text is `None`, with the last
//!   rows and highlights, which cost O(1) to share.
//! - The text is cut near [`MAX_TEXT`]: the limit is looked at before each section, so the one
//!   that passes it is whole, and the files after it show their header and a note.
//! - Comments are carried here when a file has none to place, which costs hashing alone; a file
//!   with comments is carried on the review's own thread, which reads old sides, and keeps the
//!   section it showed until that carry comes. A carry is applied by the UI only when the text
//!   holding it goes in, so the model's reading of a file is the one its text shows.
//! - Comments are cards among a file's lines, from the model's latest snapshot of them; a change
//!   to a snapshot, or to whether agent cards fold, lays out again only the files it touches. A
//!   file that leaves the listing with comments stays, after the others, until it has none.
//! - Nothing here logs a file's text or a comment's.

mod cards;
mod file;
mod section;

use std::collections::hash_map::Entry as Slot;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, Reloaded};
use oso_core::review::{Carried, Carry, Comment, Rows, Shape};
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::{Entry, Reading};
use cards::Cards;
use file::{File, Gone, section_in};

/// The most bytes a review's text holds: past it, files show their header alone.
pub(crate) const MAX_TEXT: usize = 64 * 1024 * 1024;

/// What the view thread is told.
pub(crate) enum Tell {
    /// The files the review lists now, in order, how many more it left out, and whether git's
    /// listing was cut short, so that more files may be left out unnamed.
    Listed {
        entries: Arc<[Entry]>,
        more: usize,
        cut: bool,
    },
    /// A file's new reading, and how its comments are carried to it.
    File {
        reading: Arc<Reading>,
        carry: Carrying,
    },
    /// Comments to carry here to `reading`, as applying the last carry asked, or as the
    /// review's thread could not: dropped unless `reading` is still the file's current one,
    /// the one waiting for its carry included.
    Carry { reading: Arc<Reading>, carry: Carry },
    /// What the review's thread carried to `reading`; `cut_short` if matching ran out of time.
    /// Dropped unless `reading` is still the file's current one, the one waiting for its carry
    /// included.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the UI sends these from ADR 0122's step 3b")
    )]
    Carried {
        reading: Arc<Reading>,
        carried: Carried,
        cut_short: bool,
    },
    /// The model's comments on the file at `path` now, oldest first, and its reading of the
    /// file.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the UI sends these from ADR 0122's step 3b")
    )]
    Cards {
        path: Box<[u8]>,
        comments: Vec<Comment>,
        reading: Option<Arc<Shape>>,
    },
    /// Whether agent cards fold to their first line.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the UI sends these from ADR 0122's step 3b")
    )]
    Folded(bool),
    /// Every file of the listing that changed has been read.
    Read,
    /// What the review's title says under it, each a line: a failure, a lost trust.
    Said(Vec<String>),
}

/// How a file's comments are carried to its new reading.
pub(crate) enum Carrying {
    /// Here, as the file has no comment to place.
    Here(Carry),
    /// On the review's thread, whose result comes as [`Tell::Carried`]: until then the file
    /// shows its reading before.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the UI sends these from ADR 0122's step 3b")
    )]
    Elsewhere,
    /// Not at all: the reading is not text.
    Nothing,
}

/// A text for the review buffer, with what each of its lines is, its highlights, the comments
/// carried for it, each with the reading it was carried to, and how far the reading is.
pub(crate) struct Text {
    /// The new text, or `None` when it is the text before, with no invalid byte in either:
    /// then only what each line is and its highlights may have changed.
    pub(crate) text: Option<Reloaded>,
    pub(crate) rows: Rows,
    pub(crate) highlights: Highlights,
    pub(crate) carried: Vec<(Arc<Reading>, Carried)>,
    pub(crate) reading: Option<(usize, usize)>,
}

/// Hands a text to the event loop; `false` once it is gone.
type SendText = Box<dyn FnMut(Text) -> bool + Send>;

/// What the thread takes: what it is told, or that the UI took its last text.
enum Message {
    Tell(Tell),
    Taken,
}

/// A review's view thread. Dropping it ends the thread.
pub(crate) struct View {
    tell: Sender<Message>,
    live: Arc<AtomicBool>,
}

impl View {
    /// Starts the thread for a review titled `title`, handing each text to `send`.
    pub(crate) fn start(
        title: String,
        send: impl FnMut(Text) -> bool + Send + 'static,
    ) -> std::io::Result<View> {
        let (tell, told) = mpsc::channel();
        let live = Arc::new(AtomicBool::new(true));
        let thread_live = Arc::clone(&live);
        let send: SendText = Box::new(send);
        thread::Builder::new()
            .name("oso-review-view".into())
            .spawn(move || serve(&told, send, title, &thread_live))?;
        Ok(View { tell, live })
    }

    /// Tells the thread `what`; `false` if it has ended.
    pub(crate) fn tell(&self, what: Tell) -> bool {
        self.tell.send(Message::Tell(what)).is_ok()
    }

    /// Says the UI put in the last text, so that the thread may build the next.
    pub(crate) fn taken(&self) {
        // A thread that has ended needs no word.
        let _ = self.tell.send(Message::Taken);
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// What the view thread keeps.
struct State {
    title: String,
    said: Vec<String>,
    order: Vec<Box<[u8]>>,
    files: HashMap<Box<[u8]>, File>,
    gone: BTreeMap<Box<[u8]>, Gone>,
    /// Whether agent cards fold to their first line.
    folded: bool,
    /// The most bytes the text holds: [`MAX_TEXT`], less in tests.
    max_text: usize,
    more: usize,
    /// Whether the listing was cut short.
    cut: bool,
    reading: bool,
    carried: Vec<(Arc<Reading>, Carried)>,
    text: Rope,
    /// Whether the text before held no invalid byte.
    valid: bool,
    /// Whether the text would change from the last one built: a file, the listing or what the
    /// title says changed since.
    dirty: bool,
    /// The last text's rows and highlights, for a text that did not change.
    last: Option<(Rows, Highlights)>,
}

impl State {
    fn new(title: String) -> State {
        State {
            title,
            said: Vec::new(),
            order: Vec::new(),
            files: HashMap::new(),
            gone: BTreeMap::new(),
            folded: false,
            max_text: MAX_TEXT,
            more: 0,
            cut: false,
            reading: false,
            carried: Vec::new(),
            text: Rope::new(),
            valid: true,
            dirty: true,
            last: None,
        }
    }

    fn take(&mut self, told: Tell) {
        match told {
            Tell::Listed { entries, more, cut } => self.listed(&entries, more, cut),
            Tell::File { reading, carry } => self.file(reading, carry),
            Tell::Carry { reading, carry } => {
                if self.is_current(&reading) {
                    let carried = carry_here(carry);
                    self.carried(reading, carried, false);
                }
            }
            Tell::Carried {
                reading,
                carried,
                cut_short,
            } => {
                if self.is_current(&reading) {
                    self.carried(reading, carried, cut_short);
                }
            }
            Tell::Cards {
                path,
                comments,
                reading,
            } => self.cards(path, Cards { comments, reading }),
            Tell::Folded(folded) => self.fold(folded),
            Tell::Read => self.reading = false,
            Tell::Said(said) => {
                self.dirty |= said != self.said;
                self.said = said;
            }
        }
    }

    /// Takes a new listing. A file that left it with comments stays, after the others.
    fn listed(&mut self, entries: &[Entry], more: usize, cut: bool) {
        let same = self.more == more
            && self.cut == cut
            && self.order.len() == entries.len()
            && self
                .order
                .iter()
                .zip(entries.iter())
                .all(|(a, b)| *a == b.path);
        self.dirty |= !same;
        self.more = more;
        self.cut = cut;
        self.reading = true;
        let mut files = std::mem::take(&mut self.files);
        self.order = entries.iter().map(|entry| entry.path.clone()).collect();
        for entry in entries {
            let file = match files.remove(&entry.path) {
                Some(file) => file,
                None => {
                    let cards = self.gone.remove(&entry.path).map(|gone| gone.cards);
                    File::new(entry.clone(), cards.unwrap_or_default())
                }
            };
            self.files.insert(entry.path.clone(), file);
        }
        for (path, file) in files {
            if !file.cards.comments.is_empty() {
                let gone = Gone {
                    cards: file.cards,
                    rendered: None,
                };
                self.gone.insert(path, gone);
                self.dirty = true;
            }
        }
    }

    /// Takes a file's new reading: shown now, or once its carry comes.
    fn file(&mut self, reading: Arc<Reading>, carry: Carrying) {
        let path = reading.entry.path.clone();
        let file = match self.files.entry(path) {
            Slot::Occupied(file) => file.into_mut(),
            Slot::Vacant(vacant) => {
                // Not listed yet: it goes last.
                self.order.push(vacant.key().clone());
                self.dirty = true;
                let cards = self.gone.remove(vacant.key()).map(|gone| gone.cards);
                let entry = reading.entry.clone();
                vacant.insert(File::new(entry, cards.unwrap_or_default()))
            }
        };
        match carry {
            Carrying::Here(carry) => {
                let carried = carry_here(carry);
                file.show(Arc::clone(&reading), Some(carried.placed().into()), false);
                self.carried.push((reading, carried));
                self.dirty = true;
            }
            // The file shows its reading before until its carry comes.
            Carrying::Elsewhere => file.pending = Some(reading),
            Carrying::Nothing => {
                file.show(reading, None, false);
                self.dirty = true;
            }
        }
    }

    /// Folds agent cards to their first line, or unfolds them, laying out again only the files
    /// an agent commented on.
    fn fold(&mut self, folded: bool) {
        if folded == self.folded {
            return;
        }
        self.folded = folded;
        let files = self.files.values_mut();
        let files = files.map(|file| (&file.cards, &mut file.rendered));
        let gone = self.gone.values_mut();
        let gone = gone.map(|gone| (&gone.cards, &mut gone.rendered));
        for (cards, rendered) in files.chain(gone) {
            if cards.any_agent() {
                *rendered = None;
                self.dirty = true;
            }
        }
    }

    /// Whether `reading` is its file's current reading, the one waiting for its carry included.
    fn is_current(&self, reading: &Arc<Reading>) -> bool {
        self.files
            .get(&reading.entry.path)
            .and_then(File::current)
            .is_some_and(|now| Arc::ptr_eq(now, reading))
    }

    /// Shows `reading`, the file's current one, its comments placed by `carried`, which goes in
    /// the next text for the UI to apply.
    fn carried(&mut self, reading: Arc<Reading>, carried: Carried, cut_short: bool) {
        if let Some(file) = self.files.get_mut(&reading.entry.path) {
            file.show(
                Arc::clone(&reading),
                Some(carried.placed().into()),
                cut_short,
            );
            self.carried.push((reading, carried));
            self.dirty = true;
        }
    }

    /// Takes the model's snapshot of a file's comments. A file not listed keeps a section while
    /// it has any.
    fn cards(&mut self, path: Box<[u8]>, cards: Cards) {
        if let Some(file) = self.files.get_mut(&path) {
            file.cards = cards;
            file.rendered = None;
            self.dirty = true;
        } else if cards.comments.is_empty() {
            self.dirty |= self.gone.remove(&path).is_some();
        } else {
            let gone = Gone {
                cards,
                rendered: None,
            };
            self.gone.insert(path, gone);
            self.dirty = true;
        }
    }

    /// The whole text, laid out from each file's section, those changed laid out again; or,
    /// when nothing in it changed, `None` with the last rows and highlights.
    fn build(&mut self) -> Text {
        let reading = self.reading_now();
        if !self.dirty
            && let Some((rows, highlights)) = &self.last
        {
            return Text {
                text: None,
                rows: rows.clone(),
                highlights: highlights.clone(),
                carried: std::mem::take(&mut self.carried),
                reading,
            };
        }
        let mut sections: Vec<Arc<section::Rendered>> = Vec::with_capacity(self.order.len() + 2);
        let mut said = self.said.clone();
        if self.more > 0 {
            said.push(format!("{} more files are not listed", self.more));
        }
        if self.cut {
            said.push("more files may not be listed: git's listing was cut short".to_owned());
        }
        sections.push(Arc::new(section::title(&self.title, &said)));
        let mut size = sections[0].bytes.len();
        let folded = self.folded;
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            // A file past the limit shows its header alone; one read meanwhile is laid out anew.
            let room = self.max_text.saturating_sub(size);
            let mut kept = file.rendered.take();
            let rendered = section_in(&mut kept, room, || file.render(room, folded));
            file.rendered = kept;
            size += rendered.bytes.len();
            sections.push(rendered);
        }
        for (path, gone) in &mut self.gone {
            let room = self.max_text.saturating_sub(size);
            let cards = &gone.cards;
            let rendered = section_in(&mut gone.rendered, room, || {
                section::gone(path, cards, room, folded)
            });
            size += rendered.bytes.len();
            sections.push(rendered);
        }
        let mut builder = Builder::new(Some(size as u64), TabWidth::default());
        let mut decoded = 0;
        for rendered in &sections {
            builder.push(&rendered.bytes);
            decoded += rendered.runs.iter().map(|(len, _)| len).sum::<usize>();
        }
        builder.finish();
        let snapshot = builder.snapshot();
        let mut highlights = Highlights::new(snapshot.text().len());
        let runs = sections
            .iter()
            .flat_map(|rendered| rendered.runs.iter().copied());
        highlights.replace(0..decoded, runs);
        let rows = Rows::new(
            sections
                .iter()
                .map(|rendered| Arc::clone(&rendered.section))
                .collect(),
        );
        let valid = sections.iter().all(|rendered| rendered.valid);
        let new_text = snapshot.text().clone();
        let text = snapshot.compared_with(&self.text);
        // The same text needs no swap, which would only free it again on the UI thread.
        let text = (text.is_changed() || !valid || !self.valid).then_some(text);
        self.text = new_text;
        self.valid = valid;
        self.dirty = false;
        self.last = Some((rows.clone(), highlights.clone()));
        Text {
            text,
            rows,
            highlights,
            carried: std::mem::take(&mut self.carried),
            reading,
        }
    }

    /// How many of the files listed are read, while a reading runs.
    fn reading_now(&self) -> Option<(usize, usize)> {
        self.reading.then(|| {
            let read = self
                .files
                .values()
                .filter(|file| file.reading.is_some())
                .count();
            (read, self.order.len())
        })
    }
}

/// Carries the comments of a file with none to place: nothing is matched, only hunks hashed.
fn carry_here(carry: Carry) -> Carried {
    carry.run(|_, _| None, |_, _| Vec::new())
}

/// Takes what is told, everything waiting at once, and hands out a text after each batch once
/// the UI took the last, until the view is dropped or the event loop has gone.
fn serve(told: &Receiver<Message>, mut send: SendText, title: String, live: &AtomicBool) {
    let mut state = State::new(title);
    // Whether a text was sent that the UI has not taken yet, and whether anything was told
    // since the last text.
    let mut in_flight = false;
    let mut told_since = false;
    while let Ok(first) = told.recv() {
        let mut take = |message| match message {
            Message::Tell(tell) => {
                state.take(tell);
                told_since = true;
            }
            Message::Taken => in_flight = false,
        };
        take(first);
        while let Ok(more) = told.try_recv() {
            take(more);
        }
        if !live.load(Ordering::SeqCst) {
            return;
        }
        if in_flight || !told_since {
            continue;
        }
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.build()));
        match built {
            Ok(text) => {
                in_flight = true;
                told_since = false;
                if !send(text) {
                    return;
                }
            }
            Err(_) => {
                log::warn!("a review's view failed to lay out its text");
                return;
            }
        }
    }
}

/// The text a view titled `title` builds after taking each of `told` in turn, for tests
/// outside this module.
#[cfg(test)]
pub(crate) fn built(title: &str, told: Vec<Tell>) -> Text {
    let mut state = State::new(title.to_owned());
    for tell in told {
        state.take(tell);
    }
    state.build()
}

#[cfg(test)]
mod carry_tests;
#[cfg(test)]
mod tests;
