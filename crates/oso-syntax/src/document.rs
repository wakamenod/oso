//! One buffer's parse state on the worker: its parser, its tree and the text the tree is
//! for, followed through the buffer's edits and parsed in bounded slices (ADR 0111).
//!
//! Must not depend on: the UI thread; a parse runs only on the worker.
//!
//! - A parse reads the rope's chunks through a callback, with no copy of the text.
//! - A slice stops it through tree-sitter's progress callback, and the next call resumes it
//!   where it stopped. Edits that come meanwhile wait, and do not cancel it: cancelling on every
//!   key would never finish a large file while someone types.
//! - A first parse may take [`FIRST_LIMIT`] of the worker's time, and a later one
//!   [`LATER_LIMIT`]. Past that, or past [`MAX_BYTES`], the document stops for good.

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::Splice;
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

use crate::edit::input_edits;
use crate::language::Language;

/// The largest text parsed: a larger buffer stays plain (ADR 0111).
pub const MAX_BYTES: usize = 8 << 20;

/// How much of the worker's time a parse from nothing may take.
const FIRST_LIMIT: Duration = Duration::from_secs(5);

/// How much a parse from an earlier tree may take.
const LATER_LIMIT: Duration = Duration::from_secs(1);

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
    /// The text grew past [`MAX_BYTES`].
    TooLarge,
    /// A parse passed its time limit.
    TooSlow,
    /// The grammar does not fit the tree-sitter runtime oso was built with.
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
    /// Edits that came while a parse was stopped part way, with the text after each batch.
    waiting: Vec<(Vec<Splice>, Rope)>,
    /// A parse that a slice stopped part way: what it may take in all, and what it took.
    running: Option<(Duration, Duration)>,
    failed: Option<Failure>,
    /// [`FIRST_LIMIT`] and [`LATER_LIMIT`], shorter in tests.
    limits: (Duration, Duration),
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
            text,
            current: false,
            waiting: Vec::new(),
            running: None,
            failed,
            limits: (FIRST_LIMIT, LATER_LIMIT),
        }
    }

    /// The grammar it parses with.
    pub fn language(&self) -> Language {
        self.language
    }

    /// Follows one batch of the buffer's edits: `splices`, as [`Splice`] says, which made
    /// `after`. Splices that do not take the document's text to `after` are not trusted: the
    /// next parse starts from nothing.
    pub fn edit(&mut self, splices: &[Splice], after: &Rope) {
        if self.failed.is_some() {
            return;
        }
        if self.running.is_some() {
            self.waiting.push((splices.to_vec(), after.clone()));
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
        let deadline = Instant::now() + slice;
        loop {
            if let Some(failure) = self.failed {
                return Parsed::Failed(failure);
            }
            if self.current {
                if self.waiting.is_empty() {
                    return Parsed::Done;
                }
                for (splices, after) in std::mem::take(&mut self.waiting) {
                    self.edit(&splices, &after);
                }
                continue;
            }
            if self.text.len() > MAX_BYTES {
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
        let mut progress = |_: &ParseState| {
            if Instant::now() >= stop {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let text = &self.text;
        // Never panics: a panic here would cross tree-sitter's C.
        let mut read = |byte: usize, _: tree_sitter::Point| {
            text.chunks_in_range(byte..text.len())
                .next()
                .unwrap_or("")
                .as_bytes()
        };
        let options = ParseOptions::new().progress_callback(&mut progress);
        let tree = self
            .parser
            .parse_with_options(&mut read, self.tree.as_ref(), Some(options));
        let spent = spent + now.elapsed();
        match tree {
            Some(tree) => {
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

    /// Stops for good, and frees the tree.
    fn fail(&mut self, failure: Failure) -> Parsed {
        self.failed = Some(failure);
        self.tree = None;
        self.waiting.clear();
        self.running = None;
        self.parser.reset();
        Parsed::Failed(failure)
    }

    /// The tree, once the newest parse is done.
    #[cfg(test)]
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref().filter(|_| self.current)
    }
}

#[cfg(test)]
mod tests;
