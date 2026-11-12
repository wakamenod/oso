//! What an isearch holds (ADR 0082, ADR 0116): the string, literal or a regexp, where it has
//! got to, the count, the steps `DEL` goes back through, and the echo line it says.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! `Debug` prints none of the string, which can say something of a buffer that may hold
//! secrets (ADR 0044).

use std::collections::VecDeque;
use std::fmt::{self, Write};
use std::ops::Range;

use crate::buffer::BufferId;
use crate::search::{
    self, Find, Invalid, KEY_SIZE_LIMIT, MAX_QUERY_CHARS, Needle, Prepared, Query,
};
use crate::selection::SelectionSet;
use crate::window::Scroll;

/// The most steps `DEL` can go back. The oldest go first.
const MAX_STEPS: usize = 4096;

/// What isearch knows of the count of matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor) enum Tally {
    /// A worker is counting. `moves` says how many matches the current one has moved by
    /// since the match it counts for.
    Counting { moves: isize },
    /// The current match is the `index`th of `total`, from 1, or 0 with none.
    Known { index: usize, total: usize },
}

/// How a search was asked for, which says how the count follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Asked {
    /// The string, the case or the text changed: the count starts again.
    Typed,
    /// A repeat, to the next match or, wrapping, round to the first.
    Repeat { wrapped: bool },
}

/// Why a regexp search cannot go on, which the line says after the string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Problem {
    /// The regexp is not complete yet, as a `[` being typed: `incomplete input`.
    Incomplete,
    /// What is wrong with the regexp, on one line.
    Invalid(String),
    /// A line too long to match a regexp in stopped the search, at this row from 1.
    LineTooLong(usize),
}

/// What `DEL` goes back to.
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub(super) len: usize,
    /// The whole string, when the step replaced it rather than added to it.
    pub(super) replaced: Option<String>,
    pub(super) backward: bool,
    pub(super) flipped: bool,
    pub(super) regexp: bool,
    pub(super) current: Option<Range<usize>>,
    pub(super) failing: bool,
    /// The regexp did not compile, so `C-g` goes back past it as past a failing step.
    pub(super) invalid: bool,
    pub(super) wrapped: bool,
    pub(super) cursor: usize,
    pub(super) tally: Option<Tally>,
}

/// An isearch under way in the selected window. It ends when that window shows another
/// buffer or a prompt opens over it.
pub struct Isearch {
    pub(super) string: String,
    pub(super) backward: bool,
    /// `M-c` turned smart case around.
    pub(super) flipped: bool,
    /// The string is a regexp, as `C-M-s` and `M-r` make it (ADR 0116).
    pub(super) regexp: bool,
    /// What the string looks for: `None` while it is empty, its regexp does not compile, or
    /// the worker compiles it.
    pub(super) needle: Option<Needle>,
    /// The regexp is too large to compile on the UI thread, and the worker compiles it.
    pub(super) compiling: bool,
    /// Why the regexp cannot search, for the line.
    pub(super) problem: Option<Problem>,
    /// The match the cursor is at. While failing, the last that matched, of a shorter string.
    pub(super) current: Option<Range<usize>>,
    pub(super) failing: bool,
    pub(super) wrapped: bool,
    /// A worker goes on with the search, which was asked as this and began as this find, so
    /// that a change to the text can ask it again.
    pub(super) searching: Option<(Asked, Find)>,
    pub(super) tally: Option<Tally>,
    /// The buffer it searches, and the cursors, marks and view before it began, for `C-g`.
    pub(super) buffer: BufferId,
    pub(super) begin: (SelectionSet, Scroll),
    /// The buffer's revision when it last searched.
    pub(super) revision: u64,
    pub(super) steps: VecDeque<Step>,
    /// It began, or went on, while a buffer that may hold secrets was shown.
    pub(super) around_secret: bool,
    /// Where `M-p` and `M-n` are in the history: 0 the newest.
    pub(super) history: Option<usize>,
    /// The kill the last `C-y` or `M-y` added, and how many steps the search had then, for
    /// `M-y` right after it (ADR 0091).
    pub(super) yanked: Option<(u64, usize)>,
}

impl fmt::Debug for Isearch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The string stays out of it (ADR 0044).
        f.debug_struct("Isearch")
            .field("regexp", &self.regexp)
            .field("backward", &self.backward)
            .field("failing", &self.failing)
            .field("wrapped", &self.wrapped)
            .finish_non_exhaustive()
    }
}

impl Isearch {
    /// A search of `buffer` from the place `begin` keeps, forward or `backward`.
    pub(super) fn new(
        buffer: BufferId,
        begin: (SelectionSet, Scroll),
        revision: u64,
        (backward, regexp): (bool, bool),
        around_secret: bool,
    ) -> Self {
        Self {
            string: String::new(),
            backward,
            flipped: false,
            regexp,
            needle: None,
            compiling: false,
            problem: None,
            current: None,
            failing: false,
            wrapped: false,
            searching: None,
            tally: None,
            buffer,
            begin,
            revision,
            steps: VecDeque::new(),
            around_secret,
            history: None,
            yanked: None,
        }
    }

    /// The search string.
    pub fn string(&self) -> &str {
        &self.string
    }

    /// Whether it searches backward.
    pub fn backward(&self) -> bool {
        self.backward
    }

    /// Whether the string is a regexp (ADR 0116).
    pub fn regexp(&self) -> bool {
        self.regexp
    }

    /// Whether the string has no match where it searched.
    pub fn failing(&self) -> bool {
        self.failing
    }

    /// Whether it went round the buffer's end.
    pub fn wrapped(&self) -> bool {
        self.wrapped
    }

    /// Whether case matters: smart case, turned around by `M-c`.
    pub fn case_sensitive(&self) -> bool {
        let smart = if self.regexp {
            search::regexp_smart_case(&self.string)
        } else {
            search::smart_case(&self.string)
        };
        smart != self.flipped
    }

    /// Whether a worker goes on with the search.
    pub fn searching(&self) -> bool {
        self.searching.is_some()
    }

    /// Where the current match is among them and how many matches there are, once a worker
    /// has counted them: from 1, or 0 with no current match.
    pub fn count(&self) -> Option<(usize, usize)> {
        match self.tally {
            Some(Tally::Known { index, total }) => Some((index, total)),
            _ => None,
        }
    }

    /// What is known of the count, `None` with no string.
    #[cfg(test)]
    pub(in crate::editor) fn tally(&self) -> Option<Tally> {
        self.tally
    }

    /// Whether it began, or went on, while a buffer that may hold secrets was shown, so
    /// that the control socket leaves it out and its string enters no history.
    pub fn around_secret(&self) -> bool {
        self.around_secret
    }

    /// Marks it as around a buffer that may hold secrets.
    pub(in crate::editor) fn note_secret(&mut self) {
        self.around_secret = true;
    }

    /// The echo line: `I-search: foo [3/17]`, with Emacs's words before it.
    pub(super) fn write_line(&self, out: &mut String) {
        let start = out.len();
        let mut word = |word: &str| {
            if out.len() > start {
                out.push(' ');
            }
            out.push_str(word);
        };
        if self.failing {
            word("failing");
        }
        if self.wrapped {
            word("wrapped");
        }
        if self.flipped {
            word(if self.case_sensitive() {
                "case-sensitive"
            } else {
                "case-insensitive"
            });
        }
        if self.regexp {
            word("regexp");
        }
        word("I-search");
        if self.backward {
            word("backward");
        }
        // Every word is ASCII.
        out[start..=start].make_ascii_uppercase();
        out.push_str(": ");
        out.push_str(&self.string);
        if let Some(problem) = &self.problem {
            let _ = match problem {
                Problem::Incomplete => write!(out, " [incomplete input]"),
                Problem::Invalid(why) => write!(out, " [{why}]"),
                Problem::LineTooLong(row) => {
                    let limit = search::MAX_LINE / (1024 * 1024);
                    write!(
                        out,
                        " [Regexp search stops at line {row}: it is longer than {limit} MiB]"
                    )
                }
            };
            return;
        }
        if self.searching.is_some() {
            out.push_str(" [searching…]");
            return;
        }
        let _ = match self.tally {
            None => Ok(()),
            Some(Tally::Counting { .. }) => write!(out, " [?/…]"),
            Some(Tally::Known { index, total }) => write!(out, " [{index}/{total}]"),
        };
    }

    /// Remembers where it is now, with the primary cursor at `cursor`, for `DEL`.
    pub(super) fn push_step(&mut self, cursor: usize) {
        self.push(cursor, None);
    }

    /// As [`Isearch::push_step`], before the string is replaced, so `DEL` brings it back.
    pub(super) fn push_replacing_step(&mut self, cursor: usize) {
        self.push(cursor, Some(self.string.clone()));
    }

    fn push(&mut self, cursor: usize, replaced: Option<String>) {
        if self.steps.len() == MAX_STEPS {
            self.steps.pop_front();
        }
        self.steps.push_back(Step {
            replaced,
            len: self.string.len(),
            backward: self.backward,
            flipped: self.flipped,
            regexp: self.regexp,
            current: self.current.clone(),
            failing: self.failing,
            invalid: matches!(
                self.problem,
                Some(Problem::Incomplete | Problem::Invalid(_))
            ),
            wrapped: self.wrapped,
            cursor,
            tally: self.tally,
        });
    }

    /// Makes what the string looks for anew: a literal query, or a regexp compiled within
    /// [`KEY_SIZE_LIMIT`], one larger left for the worker to compile, or why it cannot.
    pub(super) fn rebuild_query(&mut self) {
        self.compiling = false;
        self.problem = None;
        self.needle = None;
        if self.string.is_empty() {
            return;
        }
        if !self.regexp {
            self.needle = Query::new(&self.string, self.case_sensitive()).map(Needle::Literal);
            return;
        }
        let insensitive = !self.case_sensitive();
        match search::compile_isearch(&self.string, insensitive, (KEY_SIZE_LIMIT, false)) {
            Ok(regexp) => self.needle = Some(Needle::Regexp(regexp)),
            Err(Invalid::TooBig) => self.compiling = true,
            Err(Invalid::Incomplete) => self.problem = Some(Problem::Incomplete),
            Err(Invalid::Wrong(why)) => self.problem = Some(Problem::Invalid(why)),
        }
    }

    /// Whether the string has something to search for: a needle, or a regexp the worker
    /// compiles.
    pub(super) fn has_needle(&self) -> bool {
        self.needle.is_some() || self.compiling
    }

    /// What a worker's job looks for: the needle, or the pattern it is to compile.
    pub(super) fn prepared(&self) -> Option<Prepared> {
        if let Some(needle) = &self.needle {
            return Some(Prepared::Ready(needle.clone()));
        }
        self.compiling.then(|| Prepared::Compile {
            pattern: self.string.clone(),
            insensitive: !self.case_sensitive(),
        })
    }

    /// How many characters the string still has room for.
    pub(super) fn room(&self) -> usize {
        MAX_QUERY_CHARS.saturating_sub(self.string.chars().count())
    }

    /// Adds `text` up to [`MAX_QUERY_CHARS`]. Returns whether any went in.
    pub(super) fn add(&mut self, text: &str) -> bool {
        let end = text
            .char_indices()
            .nth(self.room())
            .map_or(text.len(), |(at, _)| at);
        self.string.push_str(&text[..end]);
        end > 0
    }
}
