//! One buffer's parse state on the worker: its parser, its tree, the text the tree is for and
//! its highlights, followed through the buffer's edits, parsed and highlighted in bounded
//! slices (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - A parse reads the rope's chunks through a callback, with no copy of the text.
//! - A slice stops it through tree-sitter's progress callback, and the next call resumes it
//!   where it stopped. Edits that come meanwhile wait, and do not cancel it: cancelling on every
//!   key would never finish a large file while someone types.
//! - A first parse may take [`FIRST_LIMIT`] of the worker's time, and a later one
//!   [`LATER_LIMIT`]. Past that, or past [`MAX_BYTES`] (the inline cap for Markdown's inline
//!   grammar), the document stops for good.
//! - The highlights follow each edit at once, and the lines it touched are highlighted again,
//!   with what the next parse changes (`highlight`).

use std::cell::Cell;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::{Highlights, Splice, compose};
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

use crate::edit::input_edits;
use crate::language::Language;
use crate::pending::Pending;

/// The largest text parsed: a larger buffer stays plain. A tree costs 35 to 60 times its text
/// (ADR 0111, as built). The buffer keeps no log of edits past it either.
pub const MAX_BYTES: usize = oso_core::syntax::MAX_BYTES;

/// How much of the worker's time a parse from nothing may take.
const FIRST_LIMIT: Duration = Duration::from_secs(5);

/// How much a parse from an earlier tree may take.
const LATER_LIMIT: Duration = Duration::from_secs(1);

/// The most splices that wait for a stopped parse as one batch. Past it, the next parse starts
/// from nothing, which costs a parse and keeps no more.
pub(crate) const MAX_WAITING: usize = 4096;

/// What a call to [`Document::parse`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// The tree is for the newest text.
    Done,
    /// The slice ran out; call again to go on.
    Paused,
    /// The document stopped for good.
    Failed(Failure),
}

/// Why a document stopped for good.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The text grew past [`MAX_BYTES`], or past the inline cap in a document of Markdown's
    /// inline grammar.
    TooLarge,
    /// A parse, or highlighting, passed its time limits (ADR 0111).
    TooSlow,
    /// The grammar, or oso's query for it, does not fit the tree-sitter runtime oso was built
    /// with.
    Incompatible,
}

/// One buffer's parse state. It lives on the worker, never on the UI thread.
pub struct Document {
    language: Language,
    parser: Parser,
    /// The newest tree, edited to match `text`, or none before the first parse ends.
    tree: Option<Tree>,
    /// The text the tree is for once it is current, and the text a parse reads.
    text: Rope,
    /// Whether the tree is for `text` as it is.
    current: bool,
    /// Edits that came while a parse was stopped part way, joined into one batch, with the text
    /// after them; no batch if they could not be joined, so that the next parse starts afresh.
    waiting: Option<(Option<Vec<Splice>>, Rope)>,
    /// A parse that a slice stopped part way: what it may take in all, and what it took.
    running: Option<(Duration, Duration)>,
    failed: Option<Failure>,
    /// [`FIRST_LIMIT`], [`LATER_LIMIT`] and [`highlight::HIGHLIGHT_LIMIT`], shorter in tests.
    limits: (Duration, Duration, Duration),
    /// The worker's time highlighting took since it was last done.
    spent: Duration,
    /// How many pieces fitted in a row since pieces last shrank or grew.
    fits: u32,
    /// How many times the highlights changed, so that the worker reports only a change.
    changes: u64,
    /// What the newest text is, as far as it is highlighted: it covers the text after every
    /// edit, waiting ones too.
    highlights: Highlights,
    /// What of the newest text is still to highlight.
    pending: Pending,
    /// Markdown's parsers of nested text, each made at its grammar's first text: the inline
    /// grammar's and fenced code's.
    nested: Vec<(Language, Parser)>,
    /// The blocks of the fences a fence's grammar painted last, mapped through edits since;
    /// fences that follow each other stay apart.
    fences: Pending,
    /// How many bytes a piece of highlighting may take; less after a piece outlasted a slice.
    max_piece: usize,
}

impl Document {
    /// A document for `text`, which [`Self::parse`] parses from nothing.
    pub fn new(language: Language, text: Rope) -> Self {
        let mut parser = Parser::new();
        let failed = parser
            .set_language(&language.grammar())
            .err()
            .map(|_| Failure::Incompatible);
        Self {
            language,
            parser,
            tree: None,
            highlights: Highlights::new(text.len()),
            text,
            current: false,
            waiting: None,
            running: None,
            failed,
            limits: (FIRST_LIMIT, LATER_LIMIT, highlight::HIGHLIGHT_LIMIT),
            spent: Duration::ZERO,
            fits: 0,
            changes: 0,
            pending: Pending::default(),
            nested: Vec::new(),
            fences: Pending::apart(),
            max_piece: highlight::PIECE,
        }
    }

    /// The grammar it parses with.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Follows one batch of the buffer's edits: `splices`, as [`Splice`] says, which made
    /// `after`. Splices that do not take the document's text to `after` are not trusted: the
    /// next parse starts from nothing.
    ///
    /// The highlights follow at once, and the lines the splices touched are to highlight again.
    pub fn edit(&mut self, splices: &[Splice], after: &Rope) {
        self.changes += 1;
        if self.failed.is_some() {
            // Plain, but still the newest text's length.
            self.highlights = Highlights::new(after.len());
            return;
        }
        self.highlights.splice(splices);
        if self.highlights.len() == after.len() {
            self.pending.map(splices);
            self.fences.map(splices);
            self.pending
                .add(highlight::edited(after, splices, self.max_piece));
        } else {
            self.highlights = Highlights::new(after.len());
            self.pending.all(after.len());
            self.fences.clear();
        }
        self.follow(splices, after);
    }

    /// Follows a batch in the tree: at once, or after the parse a slice stopped.
    fn follow(&mut self, splices: &[Splice], after: &Rope) {
        if self.running.is_some() {
            let joined = match self.waiting.take() {
                None => Some(splices.to_vec()),
                Some((batch, _)) => batch.and_then(|batch| compose(&batch, splices)),
            }
            .filter(|batch| batch.len() <= MAX_WAITING);
            self.waiting = Some((joined, after.clone()));
            return;
        }
        if let Some(tree) = &mut self.tree {
            match input_edits(&self.text, after, splices) {
                Some(edits) => edits.iter().for_each(|edit| tree.edit(edit)),
                None => self.tree = None,
            }
        }
        self.text = after.clone();
        self.current = false;
    }

    /// Parses for at most about `slice`, going on from where the last call stopped.
    pub fn parse(&mut self, slice: Duration) -> Parsed {
        // A slice past the first parse's limit gains nothing, and an `Instant` cannot overflow.
        let deadline = Instant::now() + slice.min(self.limits.0);
        loop {
            if let Some(failure) = self.failed {
                return Parsed::Failed(failure);
            }
            if self.current {
                let Some((batch, after)) = self.waiting.take() else {
                    return Parsed::Done;
                };
                match batch {
                    Some(batch) => self.follow(&batch, &after),
                    None => {
                        self.tree = None;
                        self.text = after;
                        self.current = false;
                    }
                }
                continue;
            }
            if self.text.len() > self.most_bytes() {
                return self.fail(Failure::TooLarge);
            }
            let now = Instant::now();
            if now >= deadline {
                return Parsed::Paused;
            }
            if let Some(parsed) = self.parse_until(deadline, now) {
                return parsed;
            }
        }
    }

    /// Runs the parse in progress, or starts one, until `deadline` or its limit. `None` when the
    /// tree is current, so that edits waiting can follow.
    fn parse_until(&mut self, deadline: Instant, now: Instant) -> Option<Parsed> {
        let (limit, spent) = self.running.unwrap_or(match self.tree {
            None => (self.limits.0, Duration::ZERO),
            Some(_) => (self.limits.1, Duration::ZERO),
        });
        let stop = deadline.min(now + limit.saturating_sub(spent));
        let stopped = Cell::new(false);
        let mut progress = until::<ParseState>(stop, &stopped);
        let mut read = reader(&self.text);
        let options = ParseOptions::new().progress_callback(&mut progress);
        let tree = self
            .parser
            .parse_with_options(&mut read, self.tree.as_ref(), Some(options));
        drop(read);
        let spent = spent + now.elapsed();
        match tree {
            Some(tree) => {
                self.take_changes(&tree);
                self.tree = Some(tree);
                self.current = true;
                self.running = None;
                None
            }
            None if spent >= limit => Some(self.fail(Failure::TooSlow)),
            None => {
                self.running = Some((limit, spent));
                Some(Parsed::Paused)
            }
        }
    }

    /// Marks what a finished parse changed as to highlight again: the ranges `tree` differs in
    /// from the tree it was parsed from, or all of the text after a parse from nothing. Edits
    /// waiting for the parse came after it, and move those ranges.
    fn take_changes(&mut self, tree: &Tree) {
        let (Some(old), Some((Some(_), _)) | None) = (&self.tree, &self.waiting) else {
            self.pending.all(self.highlights.len());
            return;
        };
        let mut changed = Pending::default();
        changed.add(old.changed_ranges(tree).map(|range| {
            highlight::lines(&self.text, range.start_byte..range.end_byte, self.max_piece)
        }));
        if let Some((Some(batch), _)) = &self.waiting {
            changed.map(batch);
        }
        self.pending.add(changed.ranges().iter().cloned());
    }

    /// The largest text it parses: [`MAX_BYTES`], or for a document of Markdown's inline grammar
    /// the inline cap, as that grammar overflows a stack on deeply nested inline links.
    fn most_bytes(&self) -> usize {
        match self.language {
            Language::MarkdownInline => inline::MAX_INLINE_BYTES,
            Language::Rust | Language::Markdown | Language::Toml => MAX_BYTES,
        }
    }

    /// Stops for good, and frees the tree; the text is plain.
    fn fail(&mut self, failure: Failure) -> Parsed {
        self.failed = Some(failure);
        self.tree = None;
        self.waiting = None;
        self.running = None;
        self.parser.reset();
        self.nested.clear();
        self.fences.clear();
        self.highlights = Highlights::new(self.highlights.len());
        self.changes += 1;
        self.pending.clear();
        Parsed::Failed(failure)
    }

    /// The tree, once the newest parse is done.
    #[cfg(test)]
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref().filter(|_| self.current)
    }
}

/// A progress callback that stops tree-sitter at `deadline`, and notes in `stopped` that it did.
/// It runs under tree-sitter's C, so it never panics.
pub(super) fn until<State>(
    deadline: Instant,
    stopped: &Cell<bool>,
) -> impl FnMut(&State) -> ControlFlow<()> + '_ {
    move |_: &State| {
        if Instant::now() >= deadline {
            stopped.set(true);
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}

/// A parser's read callback over `text`: the chunk from a byte to its end. It runs under
/// tree-sitter's C, so it never panics; a byte past the end reads nothing.
pub(super) fn reader<'text>(
    text: &'text Rope,
) -> impl FnMut(usize, tree_sitter::Point) -> &'text [u8] + 'text {
    move |byte, _| {
        text.chunks_in_range(byte..text.len())
            .next()
            .unwrap_or("")
            .as_bytes()
    }
}

mod fence;
mod highlight;
mod inline;

pub use highlight::Highlighted;
#[cfg(test)]
mod crafted;
#[cfg(test)]
mod fence_tests;
#[cfg(test)]
mod highlight_properties;
#[cfg(test)]
mod highlight_tests;
#[cfg(test)]
mod properties;
#[cfg(test)]
mod tests;
