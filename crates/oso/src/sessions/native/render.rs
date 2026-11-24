//! A native session's row as text and faces (ADR 0165, "The rows' look"): the person's
//! messages after `〉`, an agent's brief under a heading of its own, replies two columns in, thinking and tool calls as one line that `TAB`
//! unfolds (`tools`), an edit as a diff, and each turn's result as a dim line at the right.
//!
//! Must not depend on: the theme, the UI or files; the host maps [`Face`] to the theme's faces
//! and puts the text in the session's buffer.
//!
//! - A row's text ends with a newline, but an empty reply's, which may be empty. A row with a
//!   gap starts with a blank line, which sets it apart from the row before; a turn's result
//!   ends with one.
//! - Only oso's marks stand in a row's first two columns, a nested row's [`GUIDE`] among them: every line of a reply, a note or a
//!   tool's result is further in, whatever its text says, so none passes for a row of oso's.
//! - A reply is drawn as its Markdown says, its marks hidden ([`super::shown`]), with faces no
//!   row of oso's has, its tables for its window's width; its code's own highlighting lies over it.
//! - What comes from the model or a tool is shown as text, never read as anything: what is
//!   shown on a row's one line (a tool's name and target, a reason, a progress line) has its
//!   control characters turned to spaces, and is cut with `…`.
//! - Only what is shown is walked, so drawing a row costs what it shows, not what it holds.
//! - A reply that grows is drawn again from its last line only ([`tail`]).
//! - Paths under the session's directory are shown relative to it.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;

use oso_core::minibuffer::cells_of;

mod queued;
mod reply;
mod runs;
mod tools;

pub use queued::queue_rows;
pub(super) use reply::reply_lines;
pub use reply::{tables, tail};
#[cfg(test)]
pub(super) use tools::{
    MAX_DIFF_LINE, MAX_DIFF_SHOWN, MAX_SHOWN_RESULT, RESULT_LINES, first_lines,
};

use super::capped;
use super::rows::{Kind, Row, Tool};
use super::shown::{LineStart, Style};
use super::turns::interrupted;
use super::wire::TurnResult;

/// What stands in the first two columns of a nested row, a subagent's or an open row's call.
pub(super) const GUIDE: &str = "│ ";
/// The most of what a row's one line shows of a target, in bytes.
const MAX_TARGET: usize = 160;

/// What a stretch of a row's text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// What the person does next, such as a key.
    Prompt,
    /// What the lines of a message of the person's start with, which its band is drawn by.
    PromptMark,
    /// What the person said.
    Said,
    /// What the lines of an agent's brief start with, and who it is from.
    BriefMark,
    /// What an agent wrote to start the session.
    Brief,
    /// The marks before a row, and what is said less loudly.
    Dim,
    /// What Claude thought, its words alone: the row's own line is oso's.
    Thought,
    /// A tool's badge, by what its tool does, on a chip.
    Icon(Icon),
    /// A tool's name.
    Tool,
    /// The mark of a call or of thinking under way.
    Running,
    /// What a tool works on: a path, a command.
    Target,
    Added,
    Removed,
    Error,
    /// A call that waits for the person.
    Waiting,
    /// Of a reply, by its Markdown.
    Reply(Style),
    /// A permission card's frame and title: oso's words alone, never Claude's (ADR 0180).
    CardTitle,
    /// A key of a permission card, on its chip: oso's alone too.
    CardKey,
}

/// What a tool does, which its badge's colours tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Read,
    Write,
    Shell,
    Search,
    Agent,
    Web,
    Task,
    Other,
}

/// A row's text, its faces as byte ranges of the text, and for a row of several calls shown
/// open, each call's place in it and its bytes, for `TAB` and `RET` on one call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
    pub calls: Vec<(usize, Range<usize>)>,
    /// The lines of each whole table of a reply among its row's, which depend on the width, and
    /// about what they took to draw, in cells.
    pub tables: Vec<Range<usize>>,
    pub cells: usize,
}

impl Rendered {
    fn put(&mut self, text: &str, face: Option<Face>) {
        let start = self.text.len();
        self.text.push_str(text);
        if let Some(face) = face
            && !text.is_empty()
        {
            self.faces.push((start..self.text.len(), face));
        }
    }

    /// `text`'s lines, each after `before`, its spaces; `face` on each line's text, if any. An
    /// empty line has no spaces at its end, so that no row ends in them.
    fn lines(&mut self, text: &str, before: &str, mark: Option<Face>, face: Option<Face>) {
        let mut any = false;
        for line in text.lines() {
            self.put(blank_mark(before, line.is_empty()), mark);
            self.put(line, face);
            self.put("\n", None);
            any = true;
        }
        if !any {
            self.put("\n", None);
        }
    }

    /// A message: its first line after `first`, the others in line with it. What a line starts
    /// with, or an empty one's break, is in `mark`, a face no other row has; the rest in `words`.
    fn said(&mut self, first: &str, text: &str, (mark, words): (Face, Face)) {
        for (at, line) in text
            .lines()
            .chain(text.is_empty().then_some(""))
            .enumerate()
        {
            let before = blank_mark(if at == 0 { first } else { "   " }, line.is_empty());
            self.put(before, Some(mark));
            self.put(line, Some(words));
            // With the words, so that a line is two runs.
            let end = if before.is_empty() { mark } else { words };
            self.put("\n", Some(end));
        }
    }
}

/// The mark before a line: `mark`, or for an empty line, `mark` without the spaces at its end.
fn blank_mark(mark: &str, empty: bool) -> &str {
    if empty {
        mark.trim_end_matches(' ')
    } else {
        mark
    }
}

/// `row` as it shows, its paths relative to `directory`, for a window `width` cells wide.
pub fn render(row: &Row, directory: &Path, width: usize) -> Rendered {
    if matches!(row.kind, Kind::Reply { .. }) {
        return tail(row, LineStart::default(), width);
    }
    let mut out = Rendered::default();
    if row.gap {
        out.put("\n", None);
    }
    let indent = GUIDE.repeat(usize::from(row.depth));
    let follow = format!("{indent}  ");
    match &row.kind {
        Kind::Prompt(text) => out.said("〉 ", text, (Face::PromptMark, Face::Said)),
        Kind::Brief { from, text } => {
            // No `〉`: it is not the person's.
            out.put(
                &format!("@  from {}\n", one_line(from)),
                Some(Face::BriefMark),
            );
            out.said("   ", text, (Face::BriefMark, Face::Brief));
        }
        Kind::Reply { .. } => {}
        Kind::Thinking {
            text,
            millis,
            streaming,
        } => {
            let (mark, face) = match (streaming, row.folded || text.is_empty()) {
                (true, _) => ("…", Face::Running),
                (false, true) => ("▸", Face::Dim),
                (false, false) => ("▾", Face::Dim),
            };
            let took = millis.filter(|_| !streaming);
            let took = took.map_or(String::new(), |millis| format!(" · {}", seconds(millis)));
            out.put(&indent, Some(Face::Dim));
            out.put(mark, Some(face));
            out.put(&format!(" Thinking{took}\n"), Some(Face::Dim));
            if !row.folded && !text.is_empty() {
                out.lines(
                    text,
                    &format!("{indent}    "),
                    Some(Face::Dim),
                    Some(Face::Thought),
                );
            }
        }
        Kind::Tools(tools) => match tools.as_slice() {
            [tool] => tools::tool_row(&mut out, tool, row.folded, &indent, directory),
            _ => tools::group_row(&mut out, tools, row.folded, &indent, directory),
        },
        Kind::Note(text) => out.lines(text, &follow, Some(Face::Dim), Some(Face::Dim)),
        Kind::Compacted { pre, post } => {
            let counts = match (pre, post) {
                (Some(pre), Some(post)) => {
                    format!(" · {} → {} tokens", tokens(*pre), tokens(*post))
                }
                _ => String::new(),
            };
            out.put(&format!("── compacted{counts} ──\n"), Some(Face::Dim));
        }
        Kind::Result(result) => {
            let failed = result.error && !interrupted(result);
            let face = if failed { Face::Error } else { Face::Dim };
            let footer = footer(result);
            let before = width.saturating_sub(cells_of(&footer)).max(2);
            out.put(&" ".repeat(before), None);
            out.put(&footer, Some(face));
            out.put("\n\n", None);
        }
        Kind::Unreadable(bytes) => {
            let line = format!("⚠ unreadable event ({bytes} bytes)\n");
            out.put(&line, Some(Face::Error));
        }
    }
    out
}

/// A turn's result line: how long it took and what it cost, or how it failed.
fn footer(result: &TurnResult) -> String {
    let mut line = String::new();
    if interrupted(result) {
        line.push_str("✗ interrupted");
    } else if result.error {
        let subtype = if result.subtype.is_empty() {
            "error"
        } else {
            &result.subtype
        };
        let _ = write!(line, "✗ {}", one_line(subtype));
        if let Some(reason) = result
            .reason
            .as_deref()
            .filter(|reason| *reason != "completed")
        {
            let _ = write!(line, " ({})", one_line(reason));
        }
    }
    let mut add = |part: String| {
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(&part);
    };
    if let Some(millis) = result.millis {
        add(seconds(millis));
    }
    if let Some(cost) = result
        .cost_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
    {
        add(if cost < 1.0 {
            format!("${cost:.4}")
        } else {
            format!("${cost:.2}")
        });
    }
    if result.denials > 0 {
        add(format!("{} denied", result.denials));
    }
    line
}

/// What a call works on, on one line: a path relative to `directory`, a command, a pattern.
pub fn target(tool: &Tool, directory: &Path) -> String {
    let input = &tool.input;
    let path = input.path.as_deref().map(|path| relative(path, directory));
    let text = input
        .text
        .as_deref()
        .map(|text| capped(text, MAX_TARGET + 4).0);
    let shown = match tool.name.as_str() {
        "Grep" | "Glob" => match (text, path) {
            (Some(pattern), Some(path)) => format!("\"{pattern}\" in {path}"),
            (Some(pattern), None) => format!("\"{pattern}\""),
            (None, path) => path.unwrap_or_default(),
        },
        "Agent" | "Task" => {
            let kind = input.kind.as_deref().unwrap_or("agent");
            text.map_or_else(|| kind.to_owned(), |text| format!("{kind} · {text}"))
        }
        "AskUserQuestion" => {
            let question = input.question.as_deref().unwrap_or_default();
            capped(question, MAX_TARGET + 4).0.to_owned()
        }
        "TodoWrite" => input
            .items
            .map(|items| format!("{items} items"))
            .unwrap_or_default(),
        _ => path.or_else(|| text.map(str::to_owned)).unwrap_or_default(),
    };
    one_line(&shown)
}

/// `path` relative to `directory` when it is under it.
fn relative(path: &str, directory: &Path) -> String {
    match Path::new(path).strip_prefix(directory) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

/// `text` on one line, its control characters as spaces, cut to [`MAX_TARGET`] with `…`.
pub(super) fn one_line(text: &str) -> String {
    let (shown, cut) = capped(text, MAX_TARGET);
    let mut line: String = shown
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if cut {
        line.push('…');
    }
    line
}

/// A token count as the CLI says it: `20.2k` past a thousand.
pub fn tokens(count: u64) -> String {
    if count < 1000 {
        return count.to_string();
    }
    #[expect(clippy::cast_precision_loss, reason = "a count shown to one decimal")]
    let thousands = count as f64 / 1000.0;
    format!("{thousands:.1}k")
}

/// Milliseconds as the CLI says them: `850 ms`, `4.7 s`, `2 min 5 s`.
pub fn seconds(millis: u64) -> String {
    match millis {
        0..1000 => format!("{millis} ms"),
        1000..60_000 => format!("{}.{} s", millis / 1000, millis % 1000 / 100),
        _ => format!("{} min {} s", millis / 60_000, millis % 60_000 / 1000),
    }
}
