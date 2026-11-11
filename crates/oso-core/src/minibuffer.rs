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

use std::fmt;
use std::ops::Range;

pub use candidates::{BINDING_CLOSE, BINDING_OPEN, CandidateRow, annotation_column, candidate_row};
pub(crate) use completion::completion_layout;
use cut::rows_of;
pub use cut::{cells_of, cut_to, fit_message};

use crate::buffer::BufferId;
use crate::complete::Candidates;
use crate::frame::{CellRect, FloatRegions, FrameLayout, float_regions};
use crate::rope::Rope;

mod candidates;
mod completion;
mod cut;
mod history;

pub(crate) use history::Histories;

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

/// What a question's keys say when it names nothing else.
pub const VERBS: [&str; 2] = ["yes", "no"];

/// What a key's chip shows: the key, with a cell on each side.
pub const KEYS: [&str; 2] = [" y ", " n "];

/// The cells between one key's verb and the next key.
pub const KEY_GAP: usize = 2;

/// What ends a message cut to fit.
pub const CUT: &str = "…";

/// What a prompt reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A whole number, kept in the history named, if any.
    Number { history: Option<&'static str> },
    /// Any text, kept in the history named, if any (ADR 0085).
    Text { history: Option<&'static str> },
    /// A passphrase. It shows as one `*` per character, keeps no undo record and enters no
    /// history. Its buffer may hold secrets, so every rule for such a buffer applies.
    Passphrase,
    /// One of a list of candidates, matched as typed and kept in the history named
    /// (ADR 0076). `RET` gives the selected candidate.
    Complete { history: &'static str },
    /// A question answered by `y` or `n`. `yes` names the command `y` runs, confirmed,
    /// which checks again what was true when it was asked. `n` runs nothing. `verbs` say
    /// what `y` and `n` do, after their keys (ADR 0075).
    Question {
        yes: &'static str,
        verbs: [&'static str; 2],
    },
}

impl Kind {
    /// The history its answers go to, if any.
    pub fn history(self) -> Option<&'static str> {
        match self {
            Self::Number { history } | Self::Text { history } => history,
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
    /// It opened while a buffer that may hold secrets was shown, or one was shown beneath it
    /// since, so its answer enters no history.
    pub(crate) around_secret: bool,
    /// What a control socket request opened: its answer is not the person's, so it enters no
    /// history (ADR 0076).
    pub(crate) from_socket: bool,
    /// Where `M-p` and `M-n` have got to: the entry shown, 0 the newest.
    pub(crate) walk: Option<usize>,
    /// What was typed before the walk began, for `M-n` past the newest.
    pub(crate) typed: Option<String>,
    /// The agent channel caused it, and its label says so (ADR 0085).
    pub(crate) from_agent: bool,
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

    /// Whether it opened while a buffer that may hold secrets was shown, or one was shown
    /// beneath it since. Its answer then enters no history, and the control socket leaves
    /// out what it holds.
    pub fn around_secret(&self) -> bool {
        self.around_secret
    }

    /// Whether the agent channel caused it: its label starts with `Agent: `, after its
    /// depth, and only the person answers it (ADR 0085).
    pub fn from_agent(&self) -> bool {
        self.from_agent
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

/// Where the parts of a prompt go inside its float (ADR 0075).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptLayout {
    pub regions: FloatRegions,
    /// The rows of the label it shows, each a range of it. A float too short for all of them
    /// keeps the first, the last of them cut.
    pub label: Vec<Range<usize>>,
    /// Whether the label was cut, and its last row ends with `…`.
    pub label_cut: bool,
    /// Where those rows go: from the top of the inner cells, or before the answer where the
    /// float has one row.
    pub label_cells: CellRect,
    /// Where the answer is typed, on the last row of the inner cells. Empty for a question.
    pub input: CellRect,
    /// The answer's row as the field around it is drawn, with the message. Empty for a
    /// question, and where the float has one row.
    pub field: CellRect,
    /// The cells the field may reach into past its own: the padding beside it, the gap above
    /// and the padding below, where the float has them (ADR 0075). Empty with no field.
    pub field_room: CellRect,
    /// Where a message goes: at the end of the answer's row, or at the start of a question's
    /// keys. Empty with none.
    pub message: CellRect,
    /// A question's keys, `y` and `n`, on chips of [`KEYS`]'s cells, at the end of its last
    /// row, which the label never takes. Cut only at the edge of a float narrower than both.
    pub keys: [CellRect; 2],
    /// Each key's verb after it, a cell on: empty where the row is too narrow for them.
    pub verbs: [CellRect; 2],
    /// A list's count, such as `3/57`, at the end of the answer's row. Empty with no list
    /// (ADR 0076).
    pub count: CellRect,
}

/// The cells of a row the label may take where it shares the row with the answer: all of
/// it less the keys for a question, and what leaves room for an answer otherwise.
fn label_width(kind: Kind, inner_columns: usize) -> usize {
    if kind.is_question() {
        inner_columns.saturating_sub(keys_cells(None) + 1)
    } else {
        inner_columns
            .saturating_sub(INPUT_COLUMNS)
            .max(inner_columns / 2)
    }
}

/// The cells a question's keys take, with `verbs` after them or none.
fn keys_cells(verbs: Option<[&str; 2]>) -> usize {
    // Where the `n` key or its verb ends, from column 0.
    let [[_, (n, n_cells)], [_, (no, no_cells)]] = keys_at(0, verbs);
    (n + n_cells).max(no + no_cells)
}

/// The cells a field on the last row of `regions`' inner cells may reach into: the padding
/// at its sides, and the rows of padding above and below it where they are.
fn field_room(regions: &FloatRegions) -> CellRect {
    let (outer, inner) = (regions.outer, regions.inner);
    let row = inner.row + inner.rows.saturating_sub(1);
    let above = usize::from(regions.gap);
    CellRect {
        row: row - above,
        column: outer.column,
        rows: above + 1 + usize::from(regions.padded_below()),
        columns: outer.columns,
    }
}

/// The columns and widths of a question's two keys from column `at`, and of `verbs` after
/// them, or none.
fn keys_at(at: usize, verbs: Option<[&str; 2]>) -> [[(usize, usize); 2]; 2] {
    let [y, n] = KEYS.map(cells_of);
    let [yes, no] = verbs.map_or([0, 0], |verbs| verbs.map(|verb| cells_of(verb) + 1));
    let second = at + y + yes + if verbs.is_some() { KEY_GAP } else { 1 };
    [[(at, y), (second, n)], [(at + y, yes), (second + n, no)]]
}

/// A prompt's label cut into rows, and the float that holds them.
struct Label {
    regions: FloatRegions,
    /// The rows shown.
    rows: Vec<Range<usize>>,
    cut: bool,
    cells: CellRect,
    /// Whether the label shares the float's one row with the answer.
    one_row: bool,
}

/// Cuts `body` into the rows a float in `frame` shows of it, and places them: on rows of
/// their own, or first on a float of one row.
fn place_label(frame: &FrameLayout, body: &str, kind: Kind) -> Label {
    // The width does not depend on the rows, nor whether there is one.
    let probe = float_regions(frame, 1);
    let one_row = probe.inner.rows <= 1;
    let (width, most) = if one_row {
        (label_width(kind, probe.inner.columns), 1)
    } else {
        (probe.inner.columns, MAX_LABEL_ROWS)
    };
    // A row the keys leave no room on shows none of the label.
    let (mut rows, mut cut) = if width == 0 {
        (Vec::new(), false)
    } else {
        rows_of(body, width, most)
    };
    let regions = float_regions(frame, rows.len());
    let inner = regions.inner;
    let shown = if one_row {
        inner.rows
    } else {
        inner.rows - 1 - usize::from(regions.gap)
    };
    // A float too short for every row of the label keeps the first, cut with `…`, so that the
    // question's opening and its depth always show.
    if rows.len() > shown.max(1) {
        (rows, cut) = rows_of(body, width, shown.max(1));
    }
    let columns = if one_row {
        // Never past the keys, even for a cluster wider than the row.
        let last = rows.last().map_or(0, |row| cells_of(&body[row.clone()]));
        (last + usize::from(cut)).min(width).min(inner.columns)
    } else {
        inner.columns
    };
    let cells = CellRect {
        row: inner.row,
        column: inner.column,
        rows: shown.min(rows.len()),
        columns,
    };
    Label {
        regions,
        rows,
        cut,
        cells,
        one_row,
    }
}

/// Where a prompt with `label` floats in a frame laid out as `frame`, and where its label,
/// its answer, a message and a question's keys go in it (ADR 0075). The label takes rows of
/// its own, cut into them once, and the float is as tall as they are. The answer's row comes
/// below, with a message at its end that leaves the answer at least 8 columns. A question
/// has its keys there instead. A float of one row puts the label first on it, as ADR 0072
/// did.
pub fn layout(frame: &FrameLayout, label: &str, kind: Kind, message: Option<&str>) -> PromptLayout {
    let body = match kind {
        Kind::Question { .. } => label.strip_suffix(QUESTION_END).unwrap_or(label),
        _ => label,
    };
    let Label {
        regions,
        rows,
        cut: label_cut,
        cells: label_cells,
        one_row,
    } = place_label(frame, body, kind);
    let inner = regions.inner;
    let row = inner.row + inner.rows.saturating_sub(1);
    let label_columns = label_cells.columns;
    // Where what follows the label on the last row starts: after it on a row it shares.
    let start = if one_row {
        label_columns + usize::from(kind.is_question() && label_columns > 0)
    } else {
        0
    };
    let left = inner.columns.saturating_sub(start);
    let rect = |column: usize, columns: usize| {
        // A float of no rows holds nothing.
        let columns = if inner.rows == 0 {
            0
        } else {
            columns.min(inner.columns.saturating_sub(column))
        };
        CellRect {
            row,
            column: inner.column + column.min(inner.columns),
            rows: usize::from(columns > 0 && inner.rows > 0),
            columns,
        }
    };
    let message_cells =
        |room: usize| message.map_or(0, |message| (cells_of(message) + 3).min(room));
    if let Kind::Question { verbs, .. } = kind {
        // The verbs go before the keys do, and the keys stay whole while the row holds them.
        let verbs = Some(verbs).filter(|verbs| !one_row && keys_cells(Some(*verbs)) <= left);
        let [keys, verbs] = keys_at(
            start.max(inner.columns.saturating_sub(keys_cells(verbs))),
            verbs,
        );
        let message_columns = message_cells(keys[0].0.saturating_sub(start + 1));
        return PromptLayout {
            regions,
            label: rows,
            label_cut,
            label_cells,
            input: rect(0, 0),
            field: rect(0, 0),
            field_room: rect(0, 0),
            message: rect(start, message_columns),
            keys: keys.map(|(column, columns)| rect(column, columns)),
            verbs: verbs.map(|(column, columns)| rect(column, columns)),
            count: rect(0, 0),
        };
    }
    let message_columns = message_cells(left.saturating_sub(MIN_INPUT_COLUMNS));
    PromptLayout {
        regions,
        label: rows,
        label_cut,
        label_cells,
        input: rect(start, left - message_columns),
        field: rect(0, if one_row { 0 } else { inner.columns }),
        field_room: if one_row {
            rect(0, 0)
        } else {
            field_room(&regions)
        },
        message: rect(inner.columns - message_columns, message_columns),
        keys: [rect(0, 0); 2],
        verbs: [rect(0, 0); 2],
        count: rect(0, 0),
    }
}

/// What ends a question's label: what the keys that answer it are.
pub const QUESTION_END: &str = " (y or n)";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameSize, single as frame_layout};

    /// A frame whose floats are `columns` wide, as 60% of it gives.
    fn frame(columns: usize) -> FrameLayout {
        let frame = frame_layout(
            FrameSize {
                rows: 30,
                columns: columns * 10 / 6,
            },
            0,
        );
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

    const QUESTION: Kind = Kind::Question {
        yes: "x",
        verbs: ["exit", "stay"],
    };

    #[test]
    fn a_short_prompt_takes_a_row_and_the_answer_the_row_after_the_gap() {
        let kind = Kind::Number { history: None };
        let layout = layout(&frame(60), "Go to line: ", kind, None);
        let inner = layout.regions.inner;
        assert_eq!(
            (layout.label.len(), layout.label.first()),
            (1, Some(&(0..12)))
        );
        assert_eq!(layout.label_cells, CellRect { rows: 1, ..inner });
        let row = CellRect {
            row: inner.row + 2,
            rows: 1,
            ..inner
        };
        assert_eq!(layout.input, row);
        assert_eq!(layout.field, row);
        let outer = layout.regions.outer;
        assert_eq!(
            layout.field_room,
            CellRect {
                row: inner.row + 1,
                rows: 3,
                ..outer
            },
            "the gap, the answer's row and the padding below"
        );
        assert_eq!(layout.message.columns, 0);
        assert_eq!(layout.keys[0].columns, 0);
    }

    #[test]
    fn a_message_takes_the_end_of_the_answers_row() {
        let kind = Kind::Number { history: None };
        let layout = layout(&frame(60), "Go to line: ", kind, Some("Not a number"));
        let inner = layout.regions.inner;
        assert_eq!(layout.message.columns, 15);
        assert_eq!(layout.message.column + 15, inner.column + inner.columns);
        assert_eq!(layout.message.row, layout.input.row);
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
        let layout = layout(&frame, &text, QUESTION, None);
        assert_eq!(
            layout.regions.inner.rows, 5,
            "the label's rows, the gap and the keys"
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
        let short = super::layout(&frame, "Exit? (y or n)", QUESTION, None);
        assert_eq!(short.label.len(), 1);
        assert_eq!(short.label[0], 0..5, "the keys stand for `(y or n)`");
    }

    #[test]
    fn a_questions_keys_and_verbs_end_its_last_row() {
        let layout = layout(&frame(40), "Exit? (y or n)", QUESTION, None);
        let inner = layout.regions.inner;
        let end = inner.column + inner.columns;
        let [y, n] = layout.keys;
        let [yes, no] = layout.verbs;
        assert_eq!(y.row, inner.row + 2);
        assert_eq!(
            (y.columns, yes.columns, n.columns, no.columns),
            (3, 5, 3, 5)
        );
        assert_eq!(yes.column, y.column + 3);
        assert_eq!(n.column, yes.column + 5 + KEY_GAP);
        assert_eq!((no.column, no.column + no.columns), (n.column + 3, end));
        let asked = super::layout(&frame(40), "Exit?", QUESTION, Some("Answer y or n"));
        assert_eq!(asked.message.column, inner.column);
        assert!(asked.message.column + asked.message.columns < asked.keys[0].column);
    }

    #[test]
    fn the_keys_stay_whole_while_they_fit_and_nothing_passes_a_narrow_float() {
        let label = format!("Save over it?{QUESTION_END}");
        for rows in 0..8 {
            for columns in 1..40 {
                let frame = frame_layout(FrameSize { rows, columns }, 0);
                let layout = layout(&frame, &label, QUESTION, Some("Answer y or n"));
                let inner = layout.regions.inner;
                let end = inner.column + inner.columns;
                let all = layout
                    .keys
                    .iter()
                    .chain(&layout.verbs)
                    .chain([&layout.message, &layout.label_cells]);
                for rect in all {
                    assert!(rect.column + rect.columns <= end, "{rows}x{columns}");
                }
                let [y, n] = layout.keys;
                if inner.rows > 0 && inner.columns >= keys_cells(None) {
                    assert_eq!((y.columns, n.columns), (3, 3), "{rows}x{columns}");
                    let label = layout.label_cells;
                    if label.row + label.rows > y.row {
                        assert!(label.column + label.columns <= y.column, "{rows}x{columns}");
                    }
                }
                if layout.verbs[0].columns > 0 {
                    assert!(inner.rows > 1, "{rows}x{columns}: no verbs on a shared row");
                }
            }
        }
    }

    #[test]
    fn an_odd_file_name_never_hides_the_keys_or_the_questions_opening() {
        let names = [
            format!("x {}", "x".repeat(36)),
            "全角".repeat(10),
            "\u{202e}".repeat(8),
            "a\nb\u{7}c".to_owned(),
        ];
        for name in &names {
            let label = format!("Save over {name}? It changed on disk since it was read.");
            for rows in 0..8 {
                for columns in 0..80 {
                    let frame = frame_layout(FrameSize { rows, columns }, 0);
                    let layout = layout(&frame, &label, QUESTION, Some("Answer y or n"));
                    let inner = layout.regions.inner;
                    let [y, n] = layout.keys;
                    let at = format!("{rows}x{columns} {name:?}");
                    if inner.rows > 0 && inner.columns >= keys_cells(None) {
                        assert_eq!((y.columns, n.columns), (3, 3), "{at}");
                    }
                    for other in [layout.label_cells, layout.message] {
                        let overlap = other.rows > 0
                            && other.row + other.rows > y.row
                            && other.column + other.columns > y.column;
                        assert!(!overlap || y.columns == 0, "{at}: {other:?} over {y:?}");
                    }
                    if let Some(first) = layout.label.first() {
                        assert_eq!(first.start, 0, "{at}: the first words show");
                    }
                }
            }
        }
    }

    #[test]
    fn a_label_of_one_row_keeps_its_opening_and_the_start_of_a_long_name() {
        // Two text rows: the label has one row of 56 columns.
        let frame = frame_layout(
            FrameSize {
                rows: 4,
                columns: 100,
            },
            0,
        );
        let name = "n".repeat(40);
        let label = format!("Save over {name}? It changed on disk since it was read.");
        let layout = layout(&frame, &label, QUESTION, None);
        assert_eq!(layout.label.len(), 1);
        let row = &label[layout.label[0].clone()];
        assert!(
            row.starts_with(&format!("Save over {}", &name[..20])),
            "{row}"
        );
        assert!(layout.label_cut);
        // A word too long for any row starts where it is, at the narrowest float too.
        let narrow = frame_layout(
            FrameSize {
                rows: 4,
                columns: 40,
            },
            0,
        );
        let layout = super::layout(&narrow, &label, QUESTION, None);
        let row = &label[layout.label[0].clone()];
        assert!(row.starts_with("Save over nnnn"), "{row}");
    }

    #[test]
    fn a_cluster_wider_than_a_shared_row_leaves_the_keys_whole() {
        // One text row, a float of 11 columns: 3 columns of label before the keys.
        let frame = frame_layout(
            FrameSize {
                rows: 3,
                columns: 15,
            },
            0,
        );
        let layout = layout(&frame, "\u{202e}", QUESTION, None);
        let [y, n] = layout.keys;
        assert_eq!((y.columns, n.columns), (3, 3));
        assert!(layout.label_cells.column + layout.label_cells.columns < y.column);
    }

    #[test]
    fn a_float_of_one_row_puts_the_label_before_the_answer() {
        let frame = frame_layout(
            FrameSize {
                rows: 3,
                columns: 100,
            },
            0,
        );
        let kind = Kind::Number { history: None };
        let layout = layout(&frame, "Go to line: ", kind, None);
        let inner = layout.regions.inner;
        assert_eq!(inner.rows, 1);
        assert_eq!(layout.label_cells.columns, 12);
        assert_eq!(layout.input.column, inner.column + 12);
        assert_eq!(layout.field.columns, 0, "no field on a shared row");
        let asked = super::layout(&frame, "Exit? (y or n)", QUESTION, None);
        assert_eq!(asked.keys[0].column, inner.column + inner.columns - 7);
        assert_eq!(asked.verbs[0].columns, 0);
    }

    #[test]
    fn a_short_float_keeps_the_first_row_of_the_label() {
        let frame = frame_layout(
            FrameSize {
                rows: 4,
                columns: 100,
            },
            0,
        );
        let text = "word ".repeat(40);
        let layout = layout(&frame, &text, Kind::Number { history: None }, None);
        assert_eq!(layout.regions.inner.rows, 2);
        assert_eq!(layout.label.len(), 1);
        assert_eq!(layout.label[0].start, 0, "the first row of three");
        assert!(layout.label_cut);
        // No gap and no padding below: the field keeps to its row but for the sides.
        assert_eq!(layout.field_room.rows, 1);
        assert_eq!(layout.field_room.row, layout.input.row);
        assert_eq!(layout.input.row, layout.regions.inner.row + 1);
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
