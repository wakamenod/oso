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
//!   only its last lines are drawn again, the last once more as it ends; a folded row that shows
//!   nothing new says nothing.
//! - A blank line sets a row apart from the one before (`Row::gap`), as it is added. A row whose
//!   change came is open.
//! - A tool call is found again by its id for the session's life: its result, a denial and a
//!   task's progress land on it, also after its turn ended. A subagent's own messages are
//!   rows one level deeper.
//! - Calls of one tool that reads or searches, one after another at one depth, share a row
//!   (`Read ×3`), at most [`MAX_GROUP`], as the author chose (`native-sessions-ux.md`):
//!   never a call that names a path outside the session's directory or one that may hold
//!   secrets, which keeps a row of its own. A call joins while its input streams, before its
//!   path is known; once it is, a call that may not share leaves for a row of its own, or,
//!   when rows came after, is marked so that its row says so. Each call in a row folds on its
//!   own.
//! - Where a streamed reply is drawn again from is `shown`'s to say: its lines are read once as
//!   they end.
//! - A message is the person's only when oso sent its text ([`Rows::sent`]) and the CLI
//!   replayed it as one that came through oso (ADR 0181), or as a listed slash command, never
//!   replayed (step 2d). Any other replay is a note, or nothing when a command or hook wrapped
//!   it, so no text from elsewhere is shown as the person's words. What an agent's tool started
//!   the session with, which oso sent too, is a brief ([`Rows::sent_brief`]).
//! - Rows are kept for the session's life; each holds at most what `wire`'s limits let in.
//! - A session's earlier turns, read from its transcript at its start, are folded in by
//!   `past` before anything its process says, and the ids of its newest messages are kept so
//!   that a later start reads only the turns after them (step 5).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use super::shown::{self, LineStart};
use super::wire::remote::Came;
use super::wire::{
    Assistant, Block, BlockKind, Event, PartKind, Partial, Task, ToolInput, ToolResult, TurnResult,
    User,
};

mod calls;
mod changes;
mod past;
mod sent;

use sent::Sent;

use calls::new_call;
pub use changes::{Changes, RowChange};

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
    /// A blank line sets it apart from the row before.
    pub gap: bool,
}

/// What a row shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A message oso sent as the person's.
    Prompt(String),
    /// The message an agent's tool started the session with: `from` says whose it is.
    Brief { from: String, text: String },
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
    /// The person is asked about it, and has not answered.
    pub waiting: bool,
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
    sent: VecDeque<Sent>,
    /// Whose the session's first message is, read from its transcript, when an agent's tool
    /// started it.
    started_by: Option<String>,
    /// The calls of the turn under way, which its end closes.
    turn: Vec<String>,
    /// The ids of the session's own newest messages, the newest last: where its transcript's
    /// later turns follow on (step 5).
    kept: VecDeque<String>,
}

/// A block that started: its message's id, its index, its row and its place in the row; for
/// a reply, where its last line starts, its gap among the lines before it.
#[derive(Debug)]
struct Open {
    message_id: String,
    index: u64,
    row: usize,
    slot: usize,
    mark: LineStart,
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
            started_by: None,
            turn: Vec::new(),
            kept: VecDeque::new(),
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
                uuid,
                ..
            } => {
                self.keep(uuid, true);
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
            | Event::Status { .. }
            | Event::Answer(_)
            | Event::Connection(_)
            | Event::Lifecycle(_)
            | Event::Suggestion(_)
            | Event::Other(_) => Vec::new(),
        };
        changes.sort_by_key(|change| (change.row(), change.rank()));
        changes.dedup_by_key(|change| change.row());
        changes
    }

    /// Adds a row, set apart from the one before by a blank line: not from a turn's result,
    /// which ends with one, nor a tool row from a tool row, nor a turn's result from its turn.
    fn push(&mut self, kind: Kind, depth: u8, folded: bool) -> RowChange {
        let joins = matches!(
            (self.rows.last().map(|row| &row.kind), &kind),
            (None | Some(Kind::Result(_)), _)
                | (Some(Kind::Tools(_)), Kind::Tools(_))
                | (_, Kind::Result(_))
        );
        self.rows.push(Row {
            kind,
            depth,
            folded,
            gap: !joins,
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
                    mark: LineStart::after(usize::from(self.rows[row].gap)),
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
                    at: open.mark,
                };
                let start = shown.len();
                let fit = grow(shown, text);
                let newly_cut = !fit && !*cut;
                *cut |= !fit;
                if shown[start..].contains('\n') {
                    open.mark = shown::walk(shown, open.mark, true, |_| {});
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

    fn assistant(&mut self, mut message: Assistant) -> Vec<RowChange> {
        let depth = u8::from(message.parent.is_some());
        self.keep(message.uuid.take(), depth == 0);
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

    /// Puts a reply's whole text in its open row: only its last line, now ended, changes when
    /// it is what streamed, and only its last lines when it grew from it.
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
            return ends(&open, shown);
        }
        let grew = text.starts_with(shown.as_str());
        *shown = text;
        Some(if grew {
            RowChange::Grew {
                row: open.row,
                at: open.mark,
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

    fn user(&mut self, mut message: User) -> Vec<RowChange> {
        self.keep(message.uuid.take(), message.parent.is_none());
        let mut changes = Vec::new();
        for mut result in message.results {
            let id = result.tool_use_id.clone();
            let mut changed = false;
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
                changed |= !result.error && !result.patch.is_empty();
                tool.result = Some(result);
            }));
            // A change shows open: the person sees what was changed without a key.
            if let Some(&(row, _)) = self.calls.get(&id).filter(|_| changed) {
                self.rows[row].folded = false;
            }
        }
        // A subagent's prompt is its tool call's input, shown there.
        if message.parent.is_some() || message.synthetic || !message.replay {
            return changes;
        }
        if let Some(text) = message.text {
            changes.extend(match message.came {
                Came::AsSaid => self.replayed(text),
                // Not oso's, whatever its text: what oso sent still waits for its own replay.
                Came::Elsewhere => self.not_sent(text),
                Came::Remote => Some(self.remote(&text)),
            });
        }
        changes
    }

    /// A turn ended: what still streams or runs is closed, and the result is a row. The calls
    /// stay known by their ids, so a late result or task still finds its call.
    fn result(&mut self, result: TurnResult) -> Vec<RowChange> {
        let mut changes = Vec::new();
        for open in std::mem::take(&mut self.open) {
            let kind = &mut self.rows[open.row].kind;
            if let Kind::Reply {
                text, streaming, ..
            } = kind
            {
                *streaming = false;
                changes.extend(ends(&open, text));
            } else if close(kind) {
                changes.push(RowChange::Changed(open.row));
            }
        }
        changes.extend(self.unfinish_turn());
        changes.push(self.push(Kind::Result(result), 0, false));
        changes
    }

    /// The calls of the turn that still stream or run are unfinished now: their rows changed.
    fn unfinish_turn(&mut self) -> Vec<RowChange> {
        let mut changes = Vec::new();
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
        changes
    }
}

/// The reply of block `open`, now whole, whose text is `text`: its last line, drawn as written
/// until it ended, changes.
fn ends(open: &Open, text: &str) -> Option<RowChange> {
    (open.mark.from < text.len()).then_some(RowChange::Grew {
        row: open.row,
        at: open.mark,
    })
}

/// Ends a row of thinking that streamed; whether it shows otherwise now.
fn close(kind: &mut Kind) -> bool {
    match kind {
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
