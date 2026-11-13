//! A review's view thread, `oso-review-view` (ADR 0122, "The views"): it holds each file's
//! current reading, lays out each file's section, and builds the review buffer's whole text,
//! its rows and its highlights, compared with the text before, for the UI to put in at O(1).
//!
//! Must not depend on: `app`, `dispatch`, winit, git or the file system; the caller hands in
//! what the review's own thread read, and takes texts out.
//!
//! - One thread per review, started and dropped with it. It takes everything waiting before it
//!   builds, so a burst of readings costs one text.
//! - Each text is whole: every section is kept as bytes and laid end to end through
//!   `load::Builder`, which keeps invalid bytes for `\xNN` as a loaded file's, and compared
//!   with the text before so the UI swaps in only the span that changed.
//! - The text is at most [`MAX_TEXT`]; files past it show their header and a note.
//! - Comments are carried here when a file has none to place, which costs hashing alone; a file
//!   with comments is carried on the review's own thread, which reads old sides.
//! - Nothing here logs a file's text.

mod section;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, Reloaded};
use oso_core::review::{Carried, Carry, Rows};
use oso_core::rope::Rope;
use oso_core::syntax::Highlights;

use super::{Entry, Reading};

/// The most bytes a review's text holds: past it, files show their header alone.
pub(crate) const MAX_TEXT: usize = 64 * 1024 * 1024;

/// What the view thread is told.
pub(crate) enum Tell {
    /// The files the review lists now, in order, and how many more it left out.
    Listed { entries: Arc<[Entry]>, more: usize },
    /// A file's new reading, and its comments to carry to it, if it is text.
    File {
        reading: Arc<Reading>,
        carry: Option<Carry>,
    },
    /// Comments to carry again to `reading`, as applying the last carry asked: dropped unless
    /// it is still the file's current reading, whose own carry is then on its way.
    Carry { reading: Arc<Reading>, carry: Carry },
    /// Every file of the listing that changed has been read.
    Read,
    /// What the review's title says under it, each a line: a failure, a lost trust.
    Said(Vec<String>),
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

/// A review's view thread. Dropping it ends the thread.
pub(crate) struct View {
    tell: Sender<Tell>,
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
        self.tell.send(what).is_ok()
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
    }
}

/// One file's place in the view.
struct File {
    entry: Entry,
    reading: Option<Arc<Reading>>,
    /// Its section as last laid out, and whether it was whole.
    rendered: Option<(Arc<section::Rendered>, bool)>,
}

/// What the view thread keeps.
struct State {
    title: String,
    said: Vec<String>,
    order: Vec<Box<[u8]>>,
    files: HashMap<Box<[u8]>, File>,
    more: usize,
    reading: bool,
    carried: Vec<(Arc<Reading>, Carried)>,
    text: Rope,
    /// Whether the text before held no invalid byte.
    valid: bool,
}

impl State {
    fn new(title: String) -> State {
        State {
            title,
            said: Vec::new(),
            order: Vec::new(),
            files: HashMap::new(),
            more: 0,
            reading: false,
            carried: Vec::new(),
            text: Rope::new(),
            valid: true,
        }
    }

    fn take(&mut self, told: Tell) {
        match told {
            Tell::Listed { entries, more } => {
                self.more = more;
                self.reading = true;
                let mut files = std::mem::take(&mut self.files);
                self.order = entries.iter().map(|entry| entry.path.clone()).collect();
                for entry in entries.iter() {
                    let file = match files.remove(&entry.path) {
                        Some(file) => file,
                        None => File {
                            entry: entry.clone(),
                            reading: None,
                            rendered: None,
                        },
                    };
                    self.files.insert(entry.path.clone(), file);
                }
            }
            Tell::File { reading, carry } => {
                if let Some(carry) = carry {
                    self.carry(&reading, carry);
                }
                let path = reading.entry.path.clone();
                let file = self.files.entry(path.clone()).or_insert_with(|| File {
                    entry: reading.entry.clone(),
                    reading: None,
                    rendered: None,
                });
                file.entry = reading.entry.clone();
                file.reading = Some(reading);
                file.rendered = None;
                if !self.order.contains(&path) {
                    self.order.push(path);
                }
            }
            Tell::Carry { reading, carry } => {
                let current = self
                    .files
                    .get(&reading.entry.path)
                    .and_then(|file| file.reading.as_ref())
                    .is_some_and(|now| Arc::ptr_eq(now, &reading));
                if current {
                    self.carry(&reading, carry);
                }
            }
            Tell::Read => self.reading = false,
            Tell::Said(said) => self.said = said,
        }
    }

    /// Carries a file's comments to `reading`. A file with no comment to place: nothing is
    /// matched, only hunks hashed.
    fn carry(&mut self, reading: &Arc<Reading>, carry: Carry) {
        let carried = carry.run(|_, _| None, |_, _| Vec::new());
        self.carried.push((Arc::clone(reading), carried));
    }

    /// The whole text, laid out from each file's section, those changed laid out again.
    fn build(&mut self) -> Text {
        let mut sections: Vec<Arc<section::Rendered>> = Vec::with_capacity(self.order.len() + 2);
        let mut said = self.said.clone();
        if self.more > 0 {
            said.push(format!("{} more files are not listed", self.more));
        }
        sections.push(Arc::new(section::title(&self.title, &said)));
        let mut size = sections[0].bytes.len();
        let mut read = 0;
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            read += usize::from(file.reading.is_some());
            // A file past the limit shows its header alone; one read meanwhile is laid out anew.
            let whole = size < MAX_TEXT;
            let rendered = match (&file.rendered, &file.reading) {
                (Some((rendered, was)), _) if *was == whole => Arc::clone(rendered),
                (_, Some(reading)) => Arc::new(section::render(reading, whole)),
                (_, None) => Arc::new(section::unread(&file.entry)),
            };
            file.rendered = Some((Arc::clone(&rendered), whole));
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
        Text {
            text,
            rows,
            highlights,
            carried: std::mem::take(&mut self.carried),
            reading: self.reading.then_some((read, self.order.len())),
        }
    }
}

/// Takes what is told, everything waiting at once, and hands out a text after each batch,
/// until the view is dropped or the event loop has gone.
fn serve(told: &Receiver<Tell>, mut send: SendText, title: String, live: &AtomicBool) {
    let mut state = State::new(title);
    while let Ok(first) = told.recv() {
        state.take(first);
        while let Ok(more) = told.try_recv() {
            state.take(more);
        }
        if !live.load(Ordering::SeqCst) {
            return;
        }
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| state.build()));
        match built {
            Ok(text) => {
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
mod tests;
