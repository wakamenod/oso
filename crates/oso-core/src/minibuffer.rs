//! Prompts: reading an answer, or asking a question, in a float at the top center of
//! the frame, and the histories of answers (ADR 0072).
//!
//! Must not depend on: `editor`, `command` or `keymap`, or the host's window.
//!
//! A prompt is a window of its own, over the tiled ones, showing an internal buffer that
//! takes typing. The [`Editor`](crate::editor::Editor) keeps a stack of them, and only the
//! innermost shows and takes keys. A command never waits for an answer: the dispatcher
//! reads a command's arguments before running it, and a command that has to ask leaves a
//! question whose answer runs a command of its own. What a prompt ends with is an
//! [`Outcome`], which the dispatcher takes.
//!
//! What is typed here may be a passphrase, so nothing here prints it through `Debug`, and
//! a passphrase enters no history.

use std::collections::VecDeque;
use std::fmt;
use std::ops::Range;

use crate::buffer::BufferId;
use crate::display_map::text_clusters;
use crate::frame::{CellRect, FloatRegions};

/// The most prompts open at once. One more is refused.
pub const MAX_DEPTH: usize = 8;

/// The most answers a history keeps, as Emacs's `history-length`.
pub const HISTORY_LENGTH: usize = 100;

/// The longest answer a history keeps, in bytes, as long as a message may be. A longer one
/// is not recorded.
pub const MAX_ENTRY_BYTES: usize = 4096;

/// The most rows a prompt's text takes. The rest is cut, and ended by `…`.
pub const MAX_LABEL_ROWS: usize = 3;

/// The columns a prompt that reads an answer leaves for it, past the end of its text.
const INPUT_COLUMNS: usize = 20;

/// The columns a message leaves for the answer, as it takes the end of the answer's row.
const MIN_INPUT_COLUMNS: usize = 8;

/// What ends a message cut to fit.
const CUT: &str = "…";

/// What a prompt reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A whole number, kept in the history named, if any.
    Number { history: Option<&'static str> },
    /// A passphrase. It shows as one `*` per character, keeps no undo record and enters no
    /// history. Its buffer may hold secrets, so every rule for such a buffer applies.
    Passphrase,
    /// A question answered by `y` or `n`. `yes` names the command `y` runs, and `no` the
    /// one `n` runs, if any. Each checks again what was true when it was asked.
    Question {
        yes: &'static str,
        no: Option<&'static str>,
    },
}

impl Kind {
    /// The history its answers go to, if any.
    pub fn history(self) -> Option<&'static str> {
        match self {
            Self::Number { history } => history,
            Self::Passphrase | Self::Question { .. } => None,
        }
    }

    /// Whether `y` and `n` answer it, and nothing is typed.
    pub fn is_question(self) -> bool {
        matches!(self, Self::Question { .. })
    }
}

/// Why a prompt did not open, or did not take an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptError {
    /// [`MAX_DEPTH`] prompts are open already.
    TooDeep,
    /// No prompt of this kind is open.
    NoPrompt,
    /// The answer is not what the prompt reads. The prompt stays open and says so.
    Invalid,
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooDeep => "Too many prompts open",
            Self::NoPrompt => "Not in the minibuffer",
            Self::Invalid => "Not a valid answer",
        })
    }
}

impl std::error::Error for PromptError {}

/// One open prompt.
pub struct Prompt {
    /// What it says before the answer, with its depth first when it is not the outermost.
    pub(crate) label: String,
    pub(crate) kind: Kind,
    /// The buffer that holds the answer, dropped with the prompt.
    pub(crate) buffer: BufferId,
    /// The window it came from, which commands act on once it closes.
    pub(crate) beneath: usize,
    /// What a key typed in it said, until the next key.
    pub(crate) message: Option<String>,
    /// Whether its answer may enter its history: not one opened while a buffer that may
    /// hold secrets is shown.
    pub(crate) records: bool,
    /// Where `M-p` and `M-n` have got to: the entry shown, 0 the newest.
    pub(crate) walk: Option<usize>,
    /// What was typed before the walk began, for `M-n` past the newest.
    pub(crate) typed: Option<String>,
}

impl Prompt {
    /// What it says before the answer.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// What it reads.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The buffer that holds the answer.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// What a key typed in it said, until the next key.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl fmt::Debug for Prompt {
    /// The kind and the buffer, and never the text, which may be a passphrase's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prompt")
            .field("kind", &self.kind)
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

/// How a prompt ended, for the dispatcher.
pub enum Outcome {
    /// The prompt at `depth`, 1 for the outermost, was answered with `text`.
    Answered { depth: usize, text: String },
    /// A question was answered: run the command named.
    Run(&'static str),
}

impl fmt::Debug for Outcome {
    /// Never the text, which may be a passphrase.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Answered { depth, .. } => write!(f, "Answered {{ depth: {depth}, .. }}"),
            Self::Run(command) => write!(f, "Run({command:?})"),
        }
    }
}

/// The answers given to each history, newest first, in memory only: never written to
/// disk or carried through a restart (ADR 0013, ADR 0051).
#[derive(Default)]
pub struct Histories {
    pub(crate) lists: Vec<(&'static str, VecDeque<String>)>,
}

impl Histories {
    /// Records `text` as the newest of `name`. An answer given before moves to the front,
    /// and the oldest goes past [`HISTORY_LENGTH`]. Nothing empty or longer than
    /// [`MAX_ENTRY_BYTES`] is recorded.
    pub(crate) fn record(&mut self, name: &'static str, text: &str) {
        if text.is_empty() || text.len() > MAX_ENTRY_BYTES {
            return;
        }
        let index = match self.lists.iter().position(|(other, _)| *other == name) {
            Some(index) => index,
            None => {
                self.lists.push((name, VecDeque::new()));
                self.lists.len() - 1
            }
        };
        let list = &mut self.lists[index].1;
        list.retain(|entry| entry != text);
        list.push_front(text.to_owned());
        list.truncate(HISTORY_LENGTH);
    }

    /// The answers of `name`, newest first.
    pub fn entries(&self, name: &str) -> impl Iterator<Item = &str> {
        self.lists
            .iter()
            .filter(move |(other, _)| *other == name)
            .flat_map(|(_, list)| list.iter().map(String::as_str))
    }

    /// The answer `index` steps back in `name`, 0 the newest.
    pub(crate) fn get(&self, name: &str, index: usize) -> Option<&str> {
        self.entries(name).nth(index)
    }
}

/// Where the parts of a prompt go inside its float.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptLayout {
    pub regions: FloatRegions,
    /// The rows of the label, each a range of it, from the top of the inner cells.
    pub label: Vec<Range<usize>>,
    /// Whether the label was cut, and ends with `…`.
    pub label_cut: bool,
    /// Where the answer is typed, on the label's last row after it. Empty for a question.
    pub input: CellRect,
    /// Where a message goes, at the end of that row. Empty with none.
    pub message: CellRect,
}

/// The cells of `text` as one row.
pub fn cells_of(text: &str) -> usize {
    text_clusters(text, 0).map(|cluster| cluster.cells).sum()
}

/// `text` cut into rows of at most `width` cells, after the last space that fits, or at a
/// whole cluster where a word is longer than a row, and at most `rows` of them. Returns the
/// rows and whether some was left over, in which case the last row leaves a cell for `…`.
fn rows_of(text: &str, width: usize, rows: usize) -> (Vec<Range<usize>>, bool) {
    let width = width.max(1);
    let mut out = Vec::new();
    let (mut start, mut used) = (0, 0);
    // Where the row could end after a space, and the cells it would hold then.
    let mut space = None;
    for cluster in text_clusters(text, 0) {
        if used + cluster.cells > width && used > 0 {
            let (end, kept) = space
                .filter(|&(end, _)| end > start)
                .unwrap_or((cluster.offset, used));
            out.push(start..end);
            if out.len() == rows {
                return (cut_last(text, out, width), true);
            }
            (start, used, space) = (end, used - kept, None);
        }
        used += cluster.cells;
        if cluster.text == " " {
            space = Some((cluster.offset + 1, used));
        }
    }
    out.push(start..text.len());
    (out, false)
}

/// Shortens the last of `rows` so that a `…` fits after it in `width` cells.
fn cut_last(text: &str, mut rows: Vec<Range<usize>>, width: usize) -> Vec<Range<usize>> {
    if let Some(last) = rows.last_mut() {
        let mut used = 0;
        let mut end = last.start;
        for cluster in text_clusters(&text[last.clone()], 0) {
            used += cluster.cells;
            if used >= width {
                break;
            }
            end = last.start + cluster.offset + cluster.text.len();
        }
        last.end = end;
    }
    rows
}

/// How many rows a prompt with `label` takes in a float `inner_columns` wide.
pub fn label_rows(label: &str, kind: Kind, inner_columns: usize) -> usize {
    rows_of(label, label_width(kind, inner_columns), MAX_LABEL_ROWS)
        .0
        .len()
}

/// The cells of a row the label may take: all of it for a question, and what leaves
/// room for an answer otherwise.
fn label_width(kind: Kind, inner_columns: usize) -> usize {
    if kind.is_question() {
        inner_columns
    } else {
        inner_columns
            .saturating_sub(INPUT_COLUMNS)
            .max(inner_columns / 2)
    }
}

/// Where the label, the answer and a message go in `regions`, the float of a prompt with
/// `label`. A message takes the end of the last row, leaving the answer at least 8 columns,
/// and the answer what is between. A question has no answer to leave room for.
pub fn layout(
    regions: FloatRegions,
    label: &str,
    kind: Kind,
    message: Option<&str>,
) -> PromptLayout {
    let inner = regions.inner;
    let (rows, label_cut) = rows_of(label, label_width(kind, inner.columns), MAX_LABEL_ROWS);
    let last = rows.last().map_or(0, |row| cells_of(&label[row.clone()]));
    let after = last + usize::from(label_cut);
    let row = inner.row
        + rows
            .len()
            .saturating_sub(1)
            .min(inner.rows.saturating_sub(1));
    let left = inner.columns.saturating_sub(after);
    let room = if kind.is_question() {
        left
    } else {
        left.saturating_sub(MIN_INPUT_COLUMNS)
    };
    let message_columns = message.map_or(0, |message| (cells_of(message) + 3).min(room));
    let input_columns = if kind.is_question() {
        0
    } else {
        inner
            .columns
            .saturating_sub(after)
            .saturating_sub(message_columns)
    };
    let column = inner.column + after.min(inner.columns);
    let input = CellRect {
        row,
        column,
        rows: usize::from(input_columns > 0),
        columns: input_columns,
    };
    let message = CellRect {
        row,
        column: inner.column + inner.columns - message_columns,
        rows: usize::from(message_columns > 0),
        columns: message_columns,
    };
    PromptLayout {
        regions,
        label: rows,
        label_cut,
        input,
        message,
    }
}

/// `message` cut at a whole cluster to fit, with the brackets around it, in `columns` cells,
/// and `…` if it was cut: what a prompt's message row shows.
pub fn fit_message(message: &str, columns: usize) -> (&str, &'static str) {
    let room = columns.saturating_sub(3);
    if cells_of(message) <= room {
        return (message, "");
    }
    let mut used = 0;
    let mut end = 0;
    for cluster in text_clusters(message, 0) {
        used += cluster.cells;
        if used >= room {
            break;
        }
        end = cluster.offset + cluster.text.len();
    }
    (&message[..end], CUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameSize, float_regions, layout as frame_layout};

    /// A float of `columns` columns, which 60% of a frame gives.
    fn regions(columns: usize) -> FloatRegions {
        let regions = float_regions(
            &frame_layout(FrameSize {
                rows: 30,
                columns: columns * 10 / 6,
            }),
            1,
        );
        assert_eq!(regions.outer.columns, columns);
        regions
    }

    #[test]
    fn a_history_keeps_the_newest_first_and_moves_a_repeat_to_the_front() {
        let mut histories = Histories::default();
        for text in ["1", "2", "3", "2"] {
            histories.record("line", text);
        }
        assert_eq!(
            histories.entries("line").collect::<Vec<_>>(),
            ["2", "3", "1"]
        );
        assert_eq!(histories.get("line", 2), Some("1"));
        assert_eq!(histories.entries("other").count(), 0);
    }

    #[test]
    fn a_history_keeps_a_hundred_and_nothing_empty_or_too_long() {
        let mut histories = Histories::default();
        for n in 0..150 {
            histories.record("line", &n.to_string());
        }
        histories.record("line", "");
        histories.record("line", &"x".repeat(MAX_ENTRY_BYTES + 1));
        let entries: Vec<_> = histories.entries("line").collect();
        assert_eq!(entries.len(), HISTORY_LENGTH);
        assert_eq!((entries[0], entries[99]), ("149", "50"));
    }

    #[test]
    fn a_short_prompt_takes_one_row_and_the_answer_the_rest() {
        let regions = regions(60);
        let layout = layout(
            regions,
            "Go to line: ",
            Kind::Number { history: None },
            None,
        );
        let inner = regions.inner;
        assert_eq!(
            (layout.label.len(), layout.label.first()),
            (1, Some(&(0..12)))
        );
        assert_eq!(layout.input.row, inner.row);
        assert_eq!(layout.input.column, inner.column + 12);
        assert_eq!(layout.input.columns, inner.columns - 12);
        assert_eq!(layout.message.columns, 0);
    }

    #[test]
    fn a_message_takes_the_end_of_the_row() {
        let regions = regions(60);
        let kind = Kind::Number { history: None };
        let layout = layout(regions, "Go to line: ", kind, Some("Not a number"));
        let inner = regions.inner;
        assert_eq!(layout.message.columns, 15);
        assert_eq!(layout.message.column + 15, inner.column + inner.columns);
        assert_eq!(
            layout.input.column + layout.input.columns,
            layout.message.column
        );
    }

    #[test]
    fn a_long_question_breaks_at_clusters_and_is_cut_after_three_rows() {
        let regions = regions(40);
        let width = regions.inner.columns;
        let text = "あ".repeat(width * 2);
        let kind = Kind::Question { yes: "x", no: None };
        let layout = layout(regions, &text, kind, None);
        assert_eq!(layout.label.len(), 3);
        assert!(layout.label_cut);
        for row in &layout.label {
            assert!(cells_of(&text[row.clone()]) <= width);
            assert!(text.is_char_boundary(row.start) && text.is_char_boundary(row.end));
        }
        // The last row leaves a cell for the `…`.
        assert!(cells_of(&text[layout.label[2].clone()]) < width);
        assert_eq!(layout.input.columns, 0, "nothing is typed in a question");
        let short = layout_rows("Exit? (y or n)", kind, width);
        assert_eq!(short, 1);
    }

    fn layout_rows(text: &str, kind: Kind, width: usize) -> usize {
        label_rows(text, kind, width)
    }

    #[test]
    fn a_message_that_does_not_fit_is_cut_with_its_brackets_kept() {
        assert_eq!(fit_message("No match", 11), ("No match", ""));
        assert_eq!(fit_message("Beginning of buffer", 12), ("Beginnin", "…"));
        let (shown, cut) = fit_message("全角の文字", 9);
        assert_eq!((shown, cut), ("全角", "…"));
    }

    #[test]
    fn nothing_prints_what_was_typed() {
        let outcome = Outcome::Answered {
            depth: 1,
            text: "hunter2".into(),
        };
        assert!(!format!("{outcome:?}").contains("hunter2"));
    }
}
