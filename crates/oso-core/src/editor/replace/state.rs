//! query-replace's state (ADR 0113): its strings, its scope, the match it asks about, where
//! it has been, its count, and its echo line.
//!
//! Must not depend on: `command` or `keymap`, or the host. `Debug` prints neither string, as
//! either can say something of a buffer that may hold secrets.

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;

use crate::buffer::{BufferId, ContentId, Tracking};
use crate::minibuffer;
use crate::search::{Counting, Needle};
use crate::selection::SelectionSet;

/// The most cells each string takes on the echo line.
const STRING_CELLS: usize = 40;

/// An answer to query-replace's question at a match, as Emacs's keys give it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplaceAnswer {
    /// `y`, `SPC`: replace this match, go to the next.
    Replace,
    /// `n`, `DEL`: skip it, go to the next.
    Skip,
    /// `!`: replace this and every match left, without asking.
    All,
    /// `.`: replace this one and end.
    ReplaceAndExit,
    /// `,`: replace this one and stay; the next answer moves on.
    ReplaceAndStay,
    /// `^`: go back to the previous match, without undoing it.
    Back,
    /// `u`: undo the last replacement, and go back to it.
    Undo,
    /// `q`, `RET`: end.
    Exit,
    /// `?`: show the keys.
    Help,
}

/// A match the session has been at, for `^` and `u`.
#[derive(Clone, Debug)]
pub(super) struct Visit {
    /// The match as it was found.
    pub(super) found: Range<usize>,
    /// What it holds now, if it was replaced: the replacement.
    pub(super) replaced: Option<Range<usize>>,
}

/// A query-replace under way in the selected window (ADR 0113).
pub struct Replace {
    pub(super) from: String,
    pub(super) to: String,
    pub(super) regexp: bool,
    pub(super) needle: Needle,
    /// The case of the replacement follows the match's: the search folds case.
    pub(super) preserve_case: bool,
    pub(super) buffer: BufferId,
    /// The scope's ranges, tracked in the buffer under this id, so that they follow edits.
    pub(super) scope: Tracking,
    /// The match asked about.
    pub(super) current: Option<Range<usize>>,
    /// After `,` or `^`: the text the cursor stays at; the next answer moves on from its end.
    pub(super) stay: Option<Range<usize>>,
    /// The match the session stays at was empty.
    pub(super) stay_empty: bool,
    /// A worker looks for the next match from this offset.
    pub(super) searching: Option<usize>,
    /// Answers given while a worker looks, run in order once it finds.
    pub(super) pending: VecDeque<ReplaceAnswer>,
    /// The matches in the scope when the session began, once counted.
    pub(super) total: Option<usize>,
    pub(super) replaced: usize,
    pub(super) skipped: usize,
    pub(super) visits: Vec<Visit>,
    /// Where the primary cursor was, and the buffer's state, when the session began.
    pub(super) began: (usize, (ContentId, u64)),
    /// The selected window's cursors when it began, which undoing the session goes back to.
    pub(super) before: Option<SelectionSet>,
    /// The buffer's revision after the session's last look at it.
    pub(super) revision: u64,
    pub(super) around_secret: bool,
    /// `!` is under way: replacing from this offset on, a slice a turn.
    pub(super) all_from: Option<usize>,
    /// `?` was the last answer: the line shows the keys.
    pub(super) help: bool,
    /// The scope as the session began, for the count, until it has come.
    pub(super) counting: Option<Counting>,
    /// How many matches `^` has gone back over, from the last.
    pub(super) back: usize,
}

impl fmt::Debug for Replace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No string and no offset (ADR 0044).
        f.debug_struct("Replace")
            .field("regexp", &self.regexp)
            .field("replaced", &self.replaced)
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

impl Replace {
    /// What it puts in its place.
    #[cfg(test)]
    pub(crate) fn to(&self) -> &str {
        &self.to
    }

    /// Whether it replaces a regexp's matches.
    pub fn regexp(&self) -> bool {
        self.regexp
    }

    /// Whether a worker looks for the next match.
    pub fn searching(&self) -> bool {
        self.searching.is_some()
    }

    /// Whether `!` is replacing every match left.
    pub(crate) fn replacing_all(&self) -> bool {
        self.all_from.is_some()
    }

    /// The matches passed, plus the one asked about, and how many there were in the scope
    /// when it began, once counted.
    pub fn count(&self) -> Option<(usize, usize)> {
        let total = self.total?;
        let passed = self.replaced + self.skipped + usize::from(self.current.is_some());
        Some((passed.min(total), total))
    }

    /// How many it replaced.
    pub fn replaced(&self) -> usize {
        self.replaced
    }

    /// Whether it began, or went on, while a buffer that may hold secrets was shown: its
    /// strings then enter no history and stay off the control socket.
    pub fn around_secret(&self) -> bool {
        self.around_secret
    }

    /// The echo line: `Query replacing foo with bar: [3/17] (? for help)`.
    pub(super) fn write_line(&self, out: &mut String) {
        if self.all_from.is_some() {
            out.push_str("Replacing…");
            if let Some(total) = self.total {
                out.push_str(&format!(" {}/{total}", self.replaced));
            }
            return;
        }
        if self.help {
            out.push_str(
                "y/SPC replace, n/DEL skip, ! all, . this and end, , this and stay, ^ back, u undo, q/RET end",
            );
            return;
        }
        out.push_str(if self.regexp {
            "Query replacing regexp "
        } else {
            "Query replacing "
        });
        let (from, cut) = minibuffer::cut_to(&self.from, STRING_CELLS);
        out.push_str(from);
        out.push_str(cut);
        out.push_str(" with ");
        let (to, cut) = minibuffer::cut_to(&self.to, STRING_CELLS);
        out.push_str(to);
        out.push_str(cut);
        out.push(':');
        if self.searching.is_some() {
            out.push_str(" [searching…]");
        } else {
            match self.count() {
                Some((index, total)) => out.push_str(&format!(" [{index}/{total}]")),
                None => out.push_str(" [?/…]"),
            }
        }
        out.push_str(" (? for help)");
    }
}
