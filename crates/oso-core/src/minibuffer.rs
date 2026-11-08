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
use crate::complete::Candidates;
use crate::display_map::text_clusters;
use crate::frame::{CellRect, FloatRegions, FrameLayout, float_regions};
use crate::rope::Rope;

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
pub const CUT: &str = "…";

/// What a prompt reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A whole number, kept in the history named, if any.
    Number { history: Option<&'static str> },
    /// A passphrase. It shows as one `*` per character, keeps no undo record and enters no
    /// history. Its buffer may hold secrets, so every rule for such a buffer applies.
    Passphrase,
    /// One of a list of candidates, matched as typed and kept in the history named
    /// (ADR 0076). `RET` gives the selected candidate.
    Complete { history: &'static str },
    /// A question answered by `y` or `n`. `yes` names the command `y` runs, confirmed,
    /// which checks again what was true when it was asked. `n` runs nothing.
    Question { yes: &'static str },
}

impl Kind {
    /// The history its answers go to, if any.
    pub fn history(self) -> Option<&'static str> {
        match self {
            Self::Number { history } => history,
            Self::Complete { history } => Some(history),
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
    /// The prompt keeps no history to walk.
    NoHistory,
    /// The answer is not what the prompt reads, or a history walk is at its end. The prompt
    /// stays open.
    Invalid,
}

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooDeep => "Too many prompts open",
            Self::NoPrompt => "No prompt is open",
            Self::NoHistory => "No history here",
            Self::Invalid => "Not a valid answer",
        })
    }
}

impl std::error::Error for PromptError {}

/// Names one prompt, so that what waits for its answer finds it whatever opens and closes
/// meanwhile. No two prompts of one editor share one, and a later one's is greater.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PromptId(pub(crate) u64);

/// The whole number `text` holds, as a prompt for a number reads it, with spaces around it.
pub fn parse_number(text: &str) -> Option<i64> {
    text.trim().parse().ok()
}

/// How many `*` a passphrase of `text` shows in `columns` cells with its cursor at `cursor`,
/// and the cursor's column among them: one per character, with the cursor in view, at the
/// last column at most. O(log n), from the rope's summaries.
pub fn mask(text: &Rope, cursor: usize, columns: usize) -> (usize, usize) {
    let shown = columns.max(1);
    let typed = text.char_count();
    let before = text.slice(0..cursor).char_count();
    let start = before.saturating_sub(shown - 1);
    (typed.saturating_sub(start).min(shown), before - start)
}

/// One open prompt.
pub struct Prompt {
    pub(crate) id: PromptId,
    /// What it says before the answer, with its depth first when it is not the outermost.
    pub(crate) label: String,
    pub(crate) kind: Kind,
    /// The buffer that holds the answer, dropped with the prompt.
    pub(crate) buffer: BufferId,
    /// The window it came from, which commands act on once it closes.
    pub(crate) beneath: usize,
    /// What a key typed in it said, until the next key.
    pub(crate) message: Option<String>,
    /// That message was said by a command that got a passphrase, so the control socket
    /// leaves it out.
    pub(crate) message_withheld: bool,
    /// Whether its answer may enter its history: not one opened while a buffer that may
    /// hold secrets is shown.
    pub(crate) records: bool,
    /// Where `M-p` and `M-n` have got to: the entry shown, 0 the newest.
    pub(crate) walk: Option<usize>,
    /// What was typed before the walk began, for `M-n` past the newest.
    pub(crate) typed: Option<String>,
    /// Its candidates, for a prompt that completes, and the revision of its buffer they
    /// were last matched against.
    pub(crate) completion: Option<(Candidates, u64)>,
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

    /// Its name, which its [`Outcome`] carries.
    pub fn id(&self) -> PromptId {
        self.id
    }

    /// The buffer that holds the answer.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// What a key typed in it said, until the next key.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Whether its message was said by a command that got a passphrase, which the control
    /// socket leaves out (ADR 0072).
    pub fn message_withheld(&self) -> bool {
        self.message_withheld
    }

    /// Its candidates, for a prompt that completes (ADR 0076).
    pub fn candidates(&self) -> Option<&Candidates> {
        self.completion.as_ref().map(|(candidates, _)| candidates)
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
    /// The prompt `prompt` was answered with `text`.
    Answered { prompt: PromptId, text: String },
    /// A question was answered: run the command named.
    Run(&'static str),
}

impl fmt::Debug for Outcome {
    /// Never the text, which may be a passphrase.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Answered { prompt, .. } => write!(f, "Answered {{ prompt: {prompt:?}, .. }}"),
            Self::Run(command) => write!(f, "Run({command:?})"),
        }
    }
}

/// The answers given to each history, newest first, in memory only: never written to
/// disk or carried through a restart (ADR 0013, ADR 0051).
#[derive(Default)]
pub(crate) struct Histories {
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
    pub(crate) fn entries(&self, name: &str) -> impl Iterator<Item = &str> {
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
    /// Whether the label was cut, and its last row ends with `…`.
    pub label_cut: bool,
    /// A question's `(y or n)`, a range of the label that ends its last row, after a `…`
    /// if the rest was cut. It is never broken or cut. Empty for a prompt that reads.
    pub suffix: Range<usize>,
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
        // What a break after a space carries over can still leave no room for a wide cluster,
        // so this breaks again, before it, until the cluster fits or starts a row.
        while used + cluster.cells > width && used > 0 {
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

/// Where a prompt with `label` floats in a frame laid out as `frame`, and where its label,
/// its answer and a message go in it. The label is cut into rows once, and the float is as
/// tall as they are. A message takes the end of the last row, leaving the answer at least 8
/// columns, and the answer what is between. A question has no answer to leave room for.
pub fn layout(frame: &FrameLayout, label: &str, kind: Kind, message: Option<&str>) -> PromptLayout {
    // The width does not depend on the rows.
    let columns = float_regions(frame, 1).inner.columns;
    let width = label_width(kind, columns);
    // A question's `(y or n)` goes whole at the end of the last row, so that it always shows.
    let body = match kind {
        Kind::Question { .. } => label.strip_suffix(QUESTION_END).unwrap_or(label),
        _ => label,
    };
    let suffix = body.len()..label.len();
    let suffix_cells = cells_of(&label[suffix.clone()]);
    let (mut rows, mut label_cut) = rows_of(body, width, MAX_LABEL_ROWS);
    let row_cells = |rows: &[Range<usize>], cut: bool| {
        rows.last().map_or(0, |row| cells_of(&body[row.clone()])) + usize::from(cut)
    };
    if suffix_cells > 0 && row_cells(&rows, label_cut) + suffix_cells > width {
        if rows.len() < MAX_LABEL_ROWS && !label_cut {
            rows.push(body.len()..body.len());
        } else {
            rows = cut_last(body, rows, width.saturating_sub(suffix_cells));
            label_cut = true;
        }
    }
    let regions = float_regions(frame, rows.len());
    let inner = regions.inner;
    // A float too narrow for `(y or n)` clips it at its edge, and nothing goes past that.
    let after = (row_cells(&rows, label_cut) + suffix_cells).min(inner.columns);
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
        suffix,
        input,
        message,
    }
}

/// What ends a question's label: what the keys that answer it are.
pub const QUESTION_END: &str = " (y or n)";

/// `message` cut at a whole cluster to fit, with the brackets around it, in `columns` cells,
/// and `…` if it was cut: what a prompt's message row shows.
pub fn fit_message(message: &str, columns: usize) -> (&str, &'static str) {
    cut_to(message, columns.saturating_sub(3))
}

/// `text` cut at a whole cluster to fit in `cells` cells with a `…` after it, and the `…`,
/// or `text` whole and nothing if it fits. A name an untrusted file gives is cut this way
/// before a question says it, so that it cannot push the question's own words out of view.
pub fn cut_to(text: &str, cells: usize) -> (&str, &'static str) {
    if cells_of(text) <= cells {
        return (text, "");
    }
    let mut used = 0;
    let mut end = 0;
    for cluster in text_clusters(text, 0) {
        used += cluster.cells;
        if used >= cells {
            break;
        }
        end = cluster.offset + cluster.text.len();
    }
    (&text[..end], CUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameSize, layout as frame_layout};

    /// A frame whose floats are `columns` wide, as 60% of it gives.
    fn frame(columns: usize) -> FrameLayout {
        let frame = frame_layout(FrameSize {
            rows: 30,
            columns: columns * 10 / 6,
        });
        assert_eq!(float_regions(&frame, 1).outer.columns, columns);
        frame
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
        let kind = Kind::Number { history: None };
        let layout = layout(&frame(60), "Go to line: ", kind, None);
        let inner = layout.regions.inner;
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
        let kind = Kind::Number { history: None };
        let layout = layout(&frame(60), "Go to line: ", kind, Some("Not a number"));
        let inner = layout.regions.inner;
        assert_eq!(layout.message.columns, 15);
        assert_eq!(layout.message.column + 15, inner.column + inner.columns);
        assert_eq!(
            layout.input.column + layout.input.columns,
            layout.message.column
        );
    }

    #[test]
    fn a_long_question_breaks_at_clusters_and_is_cut_after_three_rows() {
        let frame = frame(40);
        let width = float_regions(&frame, 1).inner.columns;
        let text = "あ".repeat(width * 2);
        let kind = Kind::Question { yes: "x" };
        let layout = layout(&frame, &text, kind, None);
        assert_eq!(
            layout.regions.inner.rows, 3,
            "the float is as tall as the label"
        );
        assert_eq!(layout.label.len(), 3);
        assert!(layout.label_cut);
        for row in &layout.label {
            assert!(cells_of(&text[row.clone()]) <= width);
            assert!(text.is_char_boundary(row.start) && text.is_char_boundary(row.end));
        }
        // The last row leaves a cell for the `…`.
        assert!(cells_of(&text[layout.label[2].clone()]) < width);
        assert_eq!(layout.input.columns, 0, "nothing is typed in a question");
        let short = super::layout(&frame, "Exit? (y or n)", kind, None);
        assert_eq!(short.label.len(), 1);
        assert_eq!(&"Exit? (y or n)"[short.suffix.clone()], QUESTION_END);
    }

    #[test]
    fn a_questions_y_or_n_ends_its_last_row_whole_however_the_rest_is_cut() {
        let kind = Kind::Question { yes: "x" };
        let frame = frame(40);
        let width = float_regions(&frame, 1).inner.columns;
        for words in [3, 7, 10, 60] {
            let label = format!("{}{QUESTION_END}", "word ".repeat(words).trim_end());
            let layout = layout(&frame, &label, kind, None);
            let last = layout.label.last().unwrap();
            let cells = cells_of(&label[last.clone()])
                + usize::from(layout.label_cut)
                + cells_of(&label[layout.suffix.clone()]);
            assert!(cells <= width, "{words} words: {cells} cells in {width}");
            assert_eq!(&label[layout.suffix.clone()], QUESTION_END, "{words} words");
            assert!(layout.label.len() <= MAX_LABEL_ROWS);
            assert!(
                layout
                    .label
                    .iter()
                    .all(|row| row.end <= layout.suffix.start)
            );
        }
    }

    #[test]
    fn a_message_that_does_not_fit_is_cut_with_its_brackets_kept() {
        assert_eq!(fit_message("No match", 11), ("No match", ""));
        assert_eq!(fit_message("Beginning of buffer", 12), ("Beginnin", "…"));
        let (shown, cut) = fit_message("全角の文字", 9);
        assert_eq!((shown, cut), ("全角", "…"));
    }

    #[test]
    fn a_mask_keeps_the_cursor_in_view_and_counts_characters_not_bytes() {
        let text = Rope::from("日本語abc");
        assert_eq!(mask(&text, text.len(), 10), (6, 6));
        assert_eq!(mask(&text, "日本".len(), 10), (6, 2));
        // Four columns: the cursor at the end shows in the last one.
        assert_eq!(mask(&text, text.len(), 4), (3, 3));
        assert_eq!(mask(&text, 0, 4), (4, 0));
    }

    proptest::proptest! {
        /// Every row of a label fits its width, whatever spaces and wide clusters it holds,
        /// and the rows cover the text in order.
        #[test]
        fn prop_label_rows_fit_their_width(
            words in proptest::collection::vec("[a あ]{0,12}", 0..12),
            width in 2usize..30,
        ) {
            let text = words.join(" ");
            let (rows, cut) = rows_of(&text, width, usize::MAX);
            proptest::prop_assert!(!cut);
            let mut at = 0;
            for row in &rows {
                proptest::prop_assert!(cells_of(&text[row.clone()]) <= width);
                proptest::prop_assert_eq!(row.start, at);
                at = row.end;
            }
            proptest::prop_assert_eq!(at, text.len());
        }
    }

    #[test]
    fn y_or_n_stays_whole_while_it_fits_and_nothing_passes_a_narrow_float() {
        let kind = Kind::Question { yes: "x" };
        let label = format!("Save over it?{QUESTION_END}");
        for columns in 1..40 {
            let frame = frame_layout(FrameSize { rows: 30, columns });
            let layout = layout(&frame, &label, kind, Some("Answer y or n"));
            let inner = layout.regions.inner;
            let end = inner.column + inner.columns;
            assert!(
                layout.message.column + layout.message.columns <= end,
                "{columns}"
            );
            assert!(layout.input.column <= end, "{columns}");
            let last = layout.label.last().unwrap();
            let row = cells_of(&label[last.clone()])
                + usize::from(layout.label_cut)
                + cells_of(QUESTION_END);
            if cells_of(QUESTION_END) <= label_width(kind, inner.columns) {
                assert!(row <= inner.columns, "{columns}: the whole of it fits");
            }
        }
    }

    #[test]
    fn a_wide_cluster_after_a_break_at_a_space_starts_a_row() {
        let text = "aaaaaaaaaa aああああああ";
        let (rows, _) = rows_of(text, 10, usize::MAX);
        assert!(
            rows.iter().all(|row| cells_of(&text[row.clone()]) <= 10),
            "{rows:?}"
        );
    }

    #[test]
    fn a_long_name_is_cut_to_its_cells() {
        let name = "x".repeat(255);
        let (shown, cut) = cut_to(&name, 40);
        assert_eq!((cells_of(shown), cut), (39, CUT));
        assert_eq!(cut_to("notes.md", 40), ("notes.md", ""));
    }

    #[test]
    fn a_number_reads_with_spaces_around_it() {
        assert_eq!(parse_number(" 42 "), Some(42));
        assert_eq!(parse_number("-3"), Some(-3));
        assert_eq!(parse_number("4 2"), None);
    }

    #[test]
    fn nothing_prints_what_was_typed() {
        let outcome = Outcome::Answered {
            prompt: PromptId(1),
            text: "hunter2".into(),
        };
        assert!(!format!("{outcome:?}").contains("hunter2"));
    }
}
