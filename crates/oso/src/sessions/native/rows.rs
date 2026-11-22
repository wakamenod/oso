//! A native session's rows (ADR 0165, ADR 0130's buffer design): the events of its stream
//! folded into the person's messages, the replies, thinking, tool calls with their results,
//! notes and each turn's result; and after each event, which rows changed, so that the buffer
//! is redrawn row by row and never whole.
//!
//! Must not depend on: threads, files or the UI; [`Rows::apply`] is a pure step.
//!
//! - A block that streams (`--include-partial-messages`) gets its row when it starts, and
//!   grows with each delta; the whole message that follows finds the row again by its
//!   message's id and its order, a tool call by its id, so a block has one row whether or not
//!   its parts came. A reply that grows says from which line ([`RowChange::Grew`]), so that
//!   only its last lines are drawn again; a folded row that shows nothing new says nothing.
//! - A tool call is found again by its id for the session's life: its result, a denial and a
//!   task's progress land on it, also after its turn ended. A subagent's own messages are
//!   rows one level deeper.
//! - Calls of one tool that reads or searches, one after another at one depth, share a row
//!   (`Read 3 files`), at most [`MAX_GROUP`], as the author chose (`native-sessions-ux.md`):
//!   never a call that names a path outside the session's directory or one that may hold
//!   secrets, which keeps a row of its own. A call joins while its input streams, before its
//!   path is known; once it is, a call that may not share leaves for a row of its own, or,
//!   when rows came after, is marked so that its row says so. Each call in a row folds on its
//!   own.
//! - A message is the person's only when oso sent its text ([`Rows::sent`]) and the CLI
//!   replayed it, or as a listed slash command, never replayed (step 2d). Any other replay is a note, or nothing when a command or hook wrapped it, so
//!   no text from elsewhere is shown as the person's words.
//! - Rows are kept for the session's life; each holds at most what `wire`'s limits let in.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use super::wire::{
    Assistant, Block, BlockKind, Event, PartKind, Partial, Task, ToolInput, ToolResult, TurnResult,
    User,
};

mod calls;
mod sent;

use calls::new_call;

/// The tools whose calls one after another share a row: they read or search, and change
/// nothing.
const GROUPED: [&str; 3] = ["Read", "Grep", "Glob"];
/// The tools whose results carry a file's change to show as a diff.
const EDITS: [&str; 4] = ["Edit", "MultiEdit", "Write", "NotebookEdit"];

/// The most calls one row holds.
pub const MAX_GROUP: usize = 50;

/// One row of a session's buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    /// 0 for the session's own, 1 for a subagent's.
    pub depth: u8,
    /// Shown as its first line only; `TAB` turns it.
    pub folded: bool,
}

/// What a row shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A message oso sent as the person's.
    Prompt(String),
    /// The assistant's text; `streaming` while its block is open, `cut` once it reached
    /// `wire`'s limit.
    Reply {
        text: String,
        streaming: bool,
        cut: bool,
    },
    /// Thinking, which the CLI may leave empty, and how long it took.
    Thinking {
        text: String,
        millis: Option<u64>,
        streaming: bool,
    },
    /// Tool calls: one, or several of one tool that [`GROUPED`] names.
    Tools(Vec<Tool>),
    /// A line from the CLI that is not the person's: a command's output, a message from
    /// elsewhere.
    Note(String),
    /// The conversation was compacted, from and to so many tokens.
    Compacted { pre: Option<u64>, post: Option<u64> },
    /// A turn ended.
    Result(TurnResult),
    /// A line that did not read: its length.
    Unreadable(usize),
}

/// A tool call and what came of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    /// What its input works on, once the whole message came.
    pub input: ToolInput,
    /// How many bytes of its input streamed so far.
    pub streamed: usize,
    pub state: ToolState,
    pub result: Option<ToolResult>,
    /// What its subagent or task says it does now.
    pub progress: Option<String>,
    /// Shown as its first line only, in a row of several calls.
    pub folded: bool,
    /// It shares a row though it names a path outside the session's directory, or one that
    /// may hold secrets: its row says so.
    pub apart: bool,
    /// The person approved it on its card, which its row says: a plan's, whether edited, and
    /// the mode after it (part 3d).
    pub approved: Option<(bool, oso_core::editor::Approval)>,
}

/// Where a tool call is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// Its input streams.
    Streaming,
    Running,
    Done,
    Failed,
    /// Refused before it ran, with the CLI's reason.
    Denied(String),
    /// The turn ended without its result.
    Unfinished,
}

/// What changed with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// A row was added at the end, at this index.
    Added(usize),
    /// The row at this index changed.
    Changed(usize),
    /// The reply at `row` grew: its first `lines` lines as drawn stay, and what follows them
    /// is [`render::tail`](super::render::tail) from the text's byte `from`.
    Grew {
        row: usize,
        lines: usize,
        from: usize,
    },
}

impl RowChange {
    /// The row that changed.
    pub fn row(self) -> usize {
        match self {
            Self::Added(row) | Self::Changed(row) | Self::Grew { row, .. } => row,
        }
    }

    /// Of two changes to one row, which says more: an addition, then a change, then growth.
    pub fn rank(self) -> u8 {
        match self {
            Self::Added(_) => 0,
            Self::Changed(_) => 1,
            Self::Grew { .. } => 2,
        }
    }
}

/// Changes gathered over several events, one a row, in their rows' order: what says more of
/// a row wins, and of two growths the first, which the later ones grew on.
#[derive(Debug, Default)]
pub struct Changes(std::collections::BTreeMap<usize, RowChange>);

impl Changes {
    pub fn add(&mut self, change: RowChange) {
        let kept = self.0.entry(change.row()).or_insert(change);
        if change.rank() < kept.rank() {
            *kept = change;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl IntoIterator for Changes {
    type Item = RowChange;
    type IntoIter = std::collections::btree_map::IntoValues<usize, RowChange>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_values()
    }
}

/// A native session's rows.
#[derive(Debug)]
pub struct Rows {
    rows: Vec<Row>,
    /// The session's directory: a path outside it never shares a row.
    directory: PathBuf,
    /// Blocks that started and that no whole message has filled yet, in the order they
    /// started.
    open: Vec<Open>,
    /// Each tool call's row and place in it, by its id.
    calls: HashMap<String, (usize, usize)>,
    /// The texts oso sent that the CLI has not replayed yet, oldest first.
    sent: VecDeque<String>,
    /// The calls of the turn under way, which its end closes.
    turn: Vec<String>,
}

/// A block that started: its message's id, its index, its row and its place in the row; for
/// a reply, where its last line starts and how many lines are before it.
#[derive(Debug)]
struct Open {
    message_id: String,
    index: u64,
    row: usize,
    slot: usize,
    last_line: usize,
    lines: usize,
}

impl Rows {
    /// No rows yet, for a session that runs in `directory`.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            rows: Vec::new(),
            directory,
            open: Vec::new(),
            calls: HashMap::new(),
            sent: VecDeque::new(),
            turn: Vec::new(),
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The session's directory, which paths are shown relative to.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Adds a note of oso's own, such as a resumed session's first line.
    pub fn note(&mut self, text: &str) -> RowChange {
        self.push(Kind::Note(text.to_owned()), 0, false)
    }

    /// Turns the row at `index` between folded and not, or with `slot` that call of the row
    /// when the row is open and holds several. Rows of one line do not fold.
    pub fn toggle(&mut self, index: usize, slot: Option<usize>) -> Option<RowChange> {
        let row = self.rows.get_mut(index)?;
        match (&mut row.kind, slot) {
            (Kind::Tools(tools), Some(slot)) if tools.len() > 1 && !row.folded => {
                let tool = tools.get_mut(slot)?;
                tool.folded = !tool.folded;
            }
            (Kind::Tools(_) | Kind::Thinking { .. }, _) => row.folded = !row.folded,
            _ => return None,
        }
        Some(RowChange::Changed(index))
    }

    /// Folds `event` in, and says which rows changed: once each, in their order.
    pub fn apply(&mut self, event: Event) -> Vec<RowChange> {
        let mut changes = match event {
            Event::Partial(partial) => self.partial(partial).into_iter().collect(),
            Event::Assistant(message) => self.assistant(message),
            Event::User(message) => self.user(message),
            Event::Denied {
                tool_use_id,
                message,
            } => self
                .update_call(&tool_use_id, |tool| tool.state = ToolState::Denied(message))
                .into_iter()
                .collect(),
            Event::Task(task) => self.task(task).into_iter().collect(),
            Event::Compacted {
                pre_tokens,
                post_tokens,
                ..
            } => {
                let kind = Kind::Compacted {
                    pre: pre_tokens,
                    post: post_tokens,
                };
                vec![self.push(kind, 0, false)]
            }
            Event::Result(result) => self.result(result),
            Event::Unreadable { bytes } => vec![self.push(Kind::Unreadable(bytes), 0, false)],
            Event::Init(_)
            | Event::RateLimit(_)
            | Event::Status(_)
            | Event::Answer(_)
            | Event::Suggestion(_)
            | Event::Other(_) => Vec::new(),
        };
        changes.sort_by_key(|change| (change.row(), change.rank()));
        changes.dedup_by_key(|change| change.row());
        changes
    }

    fn push(&mut self, kind: Kind, depth: u8, folded: bool) -> RowChange {
        self.rows.push(Row {
            kind,
            depth,
            folded,
        });
        RowChange::Added(self.rows.len() - 1)
    }

    fn partial(&mut self, partial: Partial) -> Option<RowChange> {
        let depth = u8::from(partial.parent.is_some());
        match partial.part {
            PartKind::Start { index, block } => {
                let (change, row, slot) = match block {
                    BlockKind::Text => {
                        let kind = Kind::Reply {
                            text: String::new(),
                            streaming: true,
                            cut: false,
                        };
                        (self.push(kind, depth, false), self.rows.len() - 1, 0)
                    }
                    BlockKind::Thinking => {
                        let kind = Kind::Thinking {
                            text: String::new(),
                            millis: None,
                            streaming: true,
                        };
                        (self.push(kind, depth, true), self.rows.len() - 1, 0)
                    }
                    BlockKind::ToolUse { id, name } => {
                        let call = new_call(id, name, ToolInput::default(), ToolState::Streaming);
                        self.add_call(call, depth)
                    }
                    BlockKind::Other => return None,
                };
                self.open.push(Open {
                    message_id: partial.message_id,
                    index,
                    row,
                    slot,
                    last_line: 0,
                    lines: 0,
                });
                Some(change)
            }
            PartKind::Delta { index, text } => self.delta(&partial.message_id, index, &text),
            PartKind::Stop { .. } | PartKind::Other => None,
        }
    }

    /// More of an open block: a reply grows, folded thinking shows nothing new, and a call
    /// counts its input's bytes.
    fn delta(&mut self, message_id: &str, index: u64, text: &str) -> Option<RowChange> {
        let open = self
            .open
            .iter_mut()
            .find(|open| open.message_id == message_id && open.index == index)?;
        let row = &mut self.rows[open.row];
        match &mut row.kind {
            Kind::Reply {
                text: shown, cut, ..
            } => {
                let change = RowChange::Grew {
                    row: open.row,
                    lines: open.lines,
                    from: open.last_line,
                };
                let start = shown.len();
                let fit = grow(shown, text);
                let newly_cut = !fit && !*cut;
                *cut |= !fit;
                for (at, _) in shown[start..].match_indices('\n') {
                    open.lines += 1;
                    open.last_line = start + at + 1;
                }
                (shown.len() > start || newly_cut).then_some(change)
            }
            Kind::Thinking { text: shown, .. } => {
                let start = shown.len();
                grow(shown, text);
                // Folded, it shows `Thinking…` whatever its text.
                (!row.folded && shown.len() > start).then_some(RowChange::Changed(open.row))
            }
            Kind::Tools(tools) => {
                let tool = tools.get_mut(open.slot)?;
                tool.streamed = tool.streamed.saturating_add(text.len());
                Some(RowChange::Changed(open.row))
            }
            _ => None,
        }
    }

    fn assistant(&mut self, message: Assistant) -> Vec<RowChange> {
        let depth = u8::from(message.parent.is_some());
        let mut changes = Vec::new();
        for block in message.blocks {
            let open = self.take_open(&message.message_id, &block);
            let change = match block {
                Block::Text(text) => self.fill_reply(open, text, depth),
                Block::Thinking { text, millis } => {
                    let kind = Kind::Thinking {
                        text,
                        millis,
                        streaming: false,
                    };
                    Some(self.fill(open.map(|open| open.row), kind, depth))
                }
                Block::ToolUse { id, name, input } => {
                    let call = new_call(id, name, input, ToolState::Running);
                    changes.extend(self.fill_call(open, call, depth));
                    continue;
                }
                Block::Other => continue,
            };
            changes.extend(change);
        }
        changes
    }

    /// The open block of `message_id` that `block` fills: a tool call's by its id, else the
    /// first one of its kind.
    fn take_open(&mut self, message_id: &str, block: &Block) -> Option<Open> {
        let at = self.open.iter().position(|open| {
            open.message_id == message_id
                && match (&self.rows[open.row].kind, block) {
                    (Kind::Reply { .. }, Block::Text(_))
                    | (Kind::Thinking { .. }, Block::Thinking { .. }) => true,
                    (Kind::Tools(tools), Block::ToolUse { id, .. }) => {
                        tools.get(open.slot).is_some_and(|tool| tool.id == *id)
                    }
                    _ => false,
                }
        })?;
        Some(self.open.remove(at))
    }

    /// Puts a reply's whole text in its open row: nothing changes when it is what streamed,
    /// and only its last lines when it grew from it.
    fn fill_reply(&mut self, open: Option<Open>, text: String, depth: u8) -> Option<RowChange> {
        let Some(open) = open else {
            let kind = Kind::Reply {
                text,
                streaming: false,
                cut: false,
            };
            return Some(self.push(kind, depth, false));
        };
        let Kind::Reply {
            text: shown,
            streaming,
            ..
        } = &mut self.rows[open.row].kind
        else {
            return None;
        };
        *streaming = false;
        if *shown == text {
            return None;
        }
        let grew = text.starts_with(shown.as_str());
        *shown = text;
        Some(if grew {
            RowChange::Grew {
                row: open.row,
                lines: open.lines,
                from: open.last_line,
            }
        } else {
            RowChange::Changed(open.row)
        })
    }

    /// Puts `kind` in the open row, keeping how it was folded, or adds a folded row for it.
    fn fill(&mut self, row: Option<usize>, kind: Kind, depth: u8) -> RowChange {
        match row {
            Some(row) => {
                self.rows[row].kind = kind;
                RowChange::Changed(row)
            }
            None => self.push(kind, depth, true),
        }
    }

    fn user(&mut self, message: User) -> Vec<RowChange> {
        let mut changes = Vec::new();
        for mut result in message.results {
            let id = result.tool_use_id.clone();
            changes.extend(self.update_call(&id, |tool| {
                if !matches!(tool.state, ToolState::Denied(_)) {
                    tool.state = if result.error {
                        ToolState::Failed
                    } else {
                        ToolState::Done
                    };
                }
                if !EDITS.contains(&tool.name.as_str()) {
                    result.patch.clear();
                    result.changed = (0, 0);
                }
                tool.result = Some(result);
            }));
        }
        // A subagent's prompt is its tool call's input, shown there.
        if message.parent.is_some() || message.synthetic || !message.replay {
            return changes;
        }
        if let Some(text) = message.text {
            changes.extend(self.replayed(text));
        }
        changes
    }

    /// A turn ended: what still streams or runs is closed, and the result is a row. The calls
    /// stay known by their ids, so a late result or task still finds its call.
    fn result(&mut self, result: TurnResult) -> Vec<RowChange> {
        let mut changes = Vec::new();
        for open in std::mem::take(&mut self.open) {
            if close(&mut self.rows[open.row].kind) {
                changes.push(RowChange::Changed(open.row));
            }
        }
        for id in std::mem::take(&mut self.turn) {
            let Some(&(row, slot)) = self.calls.get(&id) else {
                continue;
            };
            if let Kind::Tools(tools) = &mut self.rows[row].kind
                && let Some(tool) = tools.get_mut(slot)
                && matches!(tool.state, ToolState::Streaming | ToolState::Running)
            {
                tool.state = ToolState::Unfinished;
                changes.push(RowChange::Changed(row));
            }
        }
        changes.push(self.push(Kind::Result(result), 0, false));
        changes
    }
}

/// Ends a row that streamed; whether it shows otherwise now.
fn close(kind: &mut Kind) -> bool {
    match kind {
        // A reply shows the same whether it streams.
        Kind::Reply { streaming, .. } => {
            *streaming = false;
            false
        }
        Kind::Thinking { streaming, .. } if *streaming => {
            *streaming = false;
            true
        }
        _ => false,
    }
}

/// Adds `more` to a streamed text, keeping it within `wire`'s limit; whether all of it fit.
fn grow(text: &mut String, more: &str) -> bool {
    let room = super::wire::MAX_TEXT.saturating_sub(text.len());
    let (kept, cut) = super::capped(more, room);
    text.push_str(kept);
    !cut
}
