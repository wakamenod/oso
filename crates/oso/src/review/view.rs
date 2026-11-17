//! A review's view thread, `oso-review-view` (ADR 0122, "The views"): it holds each file's
//! current reading, lays out each file's section, and builds the review buffer's whole text,
//! its rows and its highlights, compared with the text before, for the UI to put in at O(1).
//!
//! Must not depend on: `app`, `dispatch`, winit, git's runner or the file system; the caller
//! hands in what the review's own thread read, and takes texts out. It uses git's ids and name
//! check only, to know a side's text and to judge the paths a reading names (step 9).
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
//! - Comments are carried here when a file has none to place, which costs hashing and matching
//!   the lines both readings put in, within [`HERE_DEADLINE`], for the map the windows keep
//!   their places by (step 4); old sides are not read, so a file whose old side moved keeps no
//!   line there, and its windows go to the nearest row shown. A file with comments is carried
//!   on the review's own thread, which reads old sides, and keeps the section it showed until
//!   that carry comes. A carry is applied by the UI only when the text holding it goes in, so
//!   the model's reading of a file is the one its text shows.
//! - Comments are cards among a file's lines, from the model's latest snapshot of them; a change
//!   to a snapshot, or to whether agent cards fold, lays out again only the files it touches. A
//!   file that leaves the listing with comments stays, after the others, until it has none.
//! - Nothing here logs a file's text or a comment's.

mod assemble;
mod build;
mod cards;
mod file;
mod highlight;
mod panes;
mod project;
mod section;
mod withheld;

use std::collections::hash_map::Entry as Slot;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::load::Reloaded;
use oso_core::review::{Carried, Carry, Comment, Rows, Shape};
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::{Entry, Reading};
use cards::Cards;
pub(crate) use cards::{HunkMark, Marks};
use file::{File, Gone, Laid};
use highlight::Highlighting;
pub(crate) use highlight::{BufferSide, Highlighter, Progress};
pub(crate) use panes::{PanePaint, PaneText};

/// The most bytes a review's text holds: past it, files show their header alone.
pub(crate) const MAX_TEXT: usize = 64 * 1024 * 1024;

/// The most lines of one range windows show that are looked at for the sides on screen: more
/// than any screen holds.
const MOST_SHOWN_LINES: usize = 1024;

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
    Carried {
        reading: Arc<Reading>,
        carried: Carried,
        cut_short: bool,
    },
    /// The model's comments on the file at `path` now, oldest first, its reading of the file,
    /// and the person's decisions on that reading's hunks.
    Cards {
        path: Box<[u8]>,
        comments: Vec<Comment>,
        reading: Option<Arc<Shape>>,
        marks: Marks,
    },
    /// Whether agent cards fold to their first line.
    Folded(bool),
    /// Every file of the listing that changed has been read.
    Read,
    /// What the review's title says under it, each a line: a failure, a lost trust.
    Said(Vec<String>),
    /// Whether to build the two panes (ADR 0122, step 11), as some area shows them or is about
    /// to.
    Panes(bool),
    /// What highlights the sides (step 9), with the work tree's top that every path a reading
    /// or a gone file names is judged under (told at start too); none to stop, as off.
    Highlighter {
        highlighter: Option<Box<dyn Highlighter>>,
        top: PathBuf,
    },
    /// A report of a side's document this view opened.
    Syntax(oso_syntax::Report<u64>),
    /// The byte ranges of the review buffer's text its windows show, for the sides on screen to
    /// go first.
    Shown(Vec<Range<usize>>),
    /// An open buffer whose text a file's new side may be.
    Buffer(Box<BufferSide>),
}

/// How a file's comments are carried to its new reading.
pub(crate) enum Carrying {
    /// Here, as the file has no comment to place.
    Here(Carry),
    /// On the review's thread, whose result comes as [`Tell::Carried`]: until then the file
    /// shows its reading before.
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
    /// The two panes' texts, old then new, while they are built and changed since the last.
    pub(crate) panes: Option<Box<[PaneText; 2]>>,
    /// Or, when their texts did not change, their rows and highlights, as the sides' runs
    /// changed (step 9).
    pub(crate) pane_paint: Option<Box<PanePaint>>,
    pub(crate) carried: Vec<(Arc<Reading>, Carried)>,
    pub(crate) reading: Option<(usize, usize)>,
    /// Whether its agent cards show folded to their first line.
    pub(crate) folded: bool,
    /// How far its sides' highlighting is, while it is on (step 9).
    #[cfg_attr(
        not(any(test, feature = "dev")),
        expect(dead_code, reason = "dev builds time it for the review benchmark")
    )]
    pub(crate) highlighted: Option<Progress>,
}

/// Hands a text to the event loop; `false` once it is gone.
type SendText = Box<dyn FnMut(Text) -> bool + Send>;

/// What the thread takes: what it is told, or that the UI took its last text.
enum Message {
    /// Boxed, as a comment's snapshot makes it large.
    Tell(Box<Tell>),
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
        self.tell.send(Message::Tell(Box::new(what))).is_ok()
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
    held: withheld::Held,
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
    /// Each file's section in the last text, with where it starts there in decoded bytes, for
    /// its sides' runs to be laid over it.
    placed: Vec<(Box<[u8]>, usize, Arc<section::Rendered>)>,
    panes: panes::Panes,
    highlighting: Highlighting,
    /// The byte ranges of the text the windows show, as last told.
    shown: Vec<Range<usize>>,
}

impl State {
    fn new(title: String) -> State {
        State {
            title,
            said: Vec::new(),
            order: Vec::new(),
            files: HashMap::new(),
            gone: BTreeMap::new(),
            held: withheld::Held::default(),
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
            placed: Vec::new(),
            panes: panes::Panes::default(),
            highlighting: Highlighting::default(),
            shown: Vec::new(),
        }
    }

    /// Takes what is told; whether it may change the next text. Reports wait to be taken with
    /// the others of their batch, and what windows show orders the sides only.
    fn take(&mut self, told: Tell) -> bool {
        match told {
            Tell::Syntax(report) => {
                self.highlighting.told(report);
                return false;
            }
            Tell::Shown(ranges) => {
                self.shown = ranges;
                self.shown_now();
                return false;
            }
            Tell::Buffer(buffer) => {
                self.highlighting.offer_buffer(*buffer);
                return false;
            }
            _ => {}
        }
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
                marks,
            } => self.cards(
                path,
                Cards {
                    comments,
                    reading,
                    marks,
                },
            ),
            Tell::Folded(folded) => self.fold(folded),
            Tell::Read => self.reading = false,
            Tell::Said(said) => {
                self.dirty |= said != self.said;
                self.said = said;
            }
            Tell::Panes(on) => self.show_panes(on),
            Tell::Highlighter { highlighter, top } => {
                self.highlighting.set(highlighter, top);
                self.shown_now();
            }
            Tell::Syntax(_) | Tell::Shown(_) | Tell::Buffer(_) => {}
        }
        true
    }

    /// Tells the highlighting the lines of each side the windows show, from the ranges of the
    /// last text they showed: at most [`MOST_SHOWN_LINES`] lines a range are looked at. The
    /// ranges are read against the last text built, which may be one the UI has not put in
    /// yet: they only order the sides.
    fn shown_now(&mut self) {
        if !self.highlighting.is_on() {
            return;
        }
        let mut shown = highlight::ShownLines::new();
        if let Some((rows, _)) = &self.last {
            for range in &self.shown {
                let first = self.text.offset_to_point(range.start).row;
                let last = self.text.offset_to_point(range.end).row;
                for line in first..=last.min(first + MOST_SHOWN_LINES) {
                    let Some(row) = rows.at(line) else {
                        break;
                    };
                    if !shown.contains_key(row.path) {
                        shown.insert(row.path.into(), highlight::SideRuns::default());
                    }
                    if let (Some((side, number)), Some(sides)) =
                        (row.line(), shown.get_mut(row.path))
                    {
                        highlight::add_shown(&mut sides[highlight::index(side)], number);
                    }
                }
            }
        }
        self.highlighting.show(shown);
    }

    /// Takes the reports that came, and opens what the sides want, after each batch told and
    /// each text built; whether the reports changed what the next text shows.
    fn schedule(&mut self) -> bool {
        let changed = self.highlighting.take_reports(&self.order);
        self.highlighting.schedule(&self.order);
        changed
    }

    /// Takes a new listing. A file that left a whole listing with comments stays, after the
    /// others, as no longer changed. A cut listing tells nothing of the files it left out, as
    /// the model's files keep them: one with comments stays shown as it was, after the listed
    /// ones.
    fn listed(&mut self, entries: &[Entry], more: usize, cut: bool) {
        let whole = more == 0 && !cut;
        let before = std::mem::take(&mut self.order);
        let mut files = std::mem::take(&mut self.files);
        self.order = self.held.listed(self.highlighting.top(), entries);
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
        // In the order they were shown.
        for path in &before {
            let Some(file) = files.remove(path) else {
                continue;
            };
            if file.cards.comments.is_empty() {
                self.highlighting.forget(path);
                continue;
            }
            if whole {
                self.highlighting.forget(path);
                let gone = Gone {
                    withheld: self.held.left(self.highlighting.top(), &file),
                    cards: file.cards,
                    rendered: Laid::default(),
                };
                self.gone.insert(path.clone(), gone);
                self.dirty = true;
            } else {
                self.order.push(path.clone());
                self.files.insert(path.clone(), file);
            }
        }
        self.highlighting.keep_listed_buffers();
        let same = self.more == more && self.cut == cut && before == self.order;
        self.dirty |= !same;
        self.more = more;
        self.cut = cut;
        self.reading = true;
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
                self.highlighting.reading(&reading.entry.path, &reading);
                self.carried.push((reading, carried));
                self.dirty = true;
            }
            // The file shows its reading before until its carry comes.
            Carrying::Elsewhere => file.pending = Some(reading),
            Carrying::Nothing => {
                file.show(Arc::clone(&reading), None, false);
                self.highlighting.reading(&reading.entry.path, &reading);
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
                rendered.clear();
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
            self.highlighting.reading(&reading.entry.path, &reading);
            self.carried.push((reading, carried));
            self.dirty = true;
        }
    }

    /// Takes the model's snapshot of a file's comments. A file not listed keeps a section while
    /// it has any.
    fn cards(&mut self, path: Box<[u8]>, cards: Cards) {
        if let Some(file) = self.files.get_mut(&path) {
            file.cards = cards;
            file.rendered.clear();
            self.dirty = true;
        } else if cards.comments.is_empty() {
            self.dirty |= self.gone.remove(&path).is_some();
        } else {
            let withheld = self.held.told(self.highlighting.top(), &path);
            let gone = Gone {
                cards,
                withheld,
                rendered: Laid::default(),
            };
            self.gone.insert(path, gone);
            self.dirty = true;
        }
    }
}

/// How long one carry made here may match lines for.
const HERE_DEADLINE: Duration = Duration::from_millis(100);

/// Carries the comments of a file with none to place: hunks hashed, and the lines both readings
/// put in matched within [`HERE_DEADLINE`], with no old side read.
fn carry_here(carry: Carry) -> Carried {
    let deadline = Instant::now() + HERE_DEADLINE;
    carry.run(|_, _| None, |a, b| super::diff::matches(a, b, deadline))
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
            Message::Tell(tell) => told_since |= state.take(*tell),
            Message::Taken => in_flight = false,
        };
        take(first);
        while let Ok(more) = told.try_recv() {
            take(more);
        }
        if !live.load(Ordering::SeqCst) {
            return;
        }
        let scheduled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.schedule()));
        match scheduled {
            Ok(changed) => told_since |= changed,
            Err(_) => {
                log::warn!("a review's view failed to highlight its sides");
                return;
            }
        }
        if in_flight || !told_since {
            continue;
        }
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let text = state.build();
            // The text says which files show lines, which are the ones whose sides open.
            told_since = state.schedule();
            text
        }));
        match built {
            Ok(text) => {
                in_flight = true;
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
mod highlight_limits_tests;
#[cfg(test)]
mod highlight_tests;
#[cfg(test)]
mod project_tests;
#[cfg(test)]
mod tests;
