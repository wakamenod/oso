//! A native session's row as text and faces (ADR 0165, "The rows' look"): the person's
//! messages after `〉`, replies two columns in, thinking and tool calls as one line that `TAB`
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
//! - A reply's text has no face of its own, so that Markdown's highlighting can lie over it.
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
mod runs;
mod tools;

pub use queued::queue_rows;
#[cfg(test)]
pub(super) use tools::{
    MAX_DIFF_LINE, MAX_DIFF_SHOWN, MAX_SHOWN_RESULT, RESULT_LINES, first_lines,
};

use super::capped;
use super::rows::{Kind, Row, Tool};
use super::turns::interrupted;
use super::wire::{MAX_TEXT, TurnResult};

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
    /// The marks before a row, and what is said less loudly.
    Dim,
    /// A tool's badge and name.
    Tool,
    /// What a tool works on: a path, a command.
    Target,
    Added,
    Removed,
    Error,
    /// A call that waits for the person.
    Waiting,
    /// A permission card's frame and title: oso's words alone, never Claude's (ADR 0180).
    CardTitle,
    /// A key of a permission card, on its chip: oso's alone too.
    CardKey,
}

/// A row's text, its faces as byte ranges of the text, and for a row of several calls shown
/// open, each call's place in it and its bytes, for `TAB` and `RET` on one call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub faces: Vec<(Range<usize>, Face)>,
    pub calls: Vec<(usize, Range<usize>)>,
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

    /// The person's message: its first line after `〉`, the others in line with it. What a line
    /// starts with, or an empty one's break, has a face of its own, which no other row has.
    fn prompt(&mut self, text: &str) {
        for (at, line) in text
            .lines()
            .chain(text.is_empty().then_some(""))
            .enumerate()
        {
            let before = if at == 0 { "〉 " } else { "   " };
            self.put(blank_mark(before, line.is_empty()), Some(Face::PromptMark));
            // Plain, as the person's words are read.
            self.put(line, None);
            let start = line.is_empty() && at > 0;
            self.put("\n", start.then_some(Face::PromptMark));
        }
    }
}

/// The mark before a line: `mark`, or for an empty line, `mark` without the spaces at its end.
pub(super) fn blank_mark(mark: &str, empty: bool) -> &str {
    if empty {
        mark.trim_end_matches(' ')
    } else {
        mark
    }
}

/// What each line of reply `row` starts with.
pub(super) fn reply_indent(row: &Row) -> String {
    GUIDE.repeat(usize::from(row.depth)) + "  "
}

/// What follows the first `lines` lines of a reply row as drawn, its gap among them, from its
/// text's byte `from`, where a line starts: what
/// [`RowChange::Grew`](super::rows::RowChange::Grew) says to draw again. Together they draw
/// what [`render`] draws, from the last line only.
pub fn tail(row: &Row, lines: usize, from: usize) -> Rendered {
    let mut out = Rendered::default();
    let Kind::Reply { text, cut, .. } = &row.kind else {
        return out;
    };
    if lines == 0 && row.gap {
        out.put("\n", None);
    }
    let from = text.floor_char_boundary(from.min(text.len()));
    let rest = &text[from..];
    let indent = reply_indent(row);
    if !rest.is_empty() {
        out.lines(rest, &indent, Some(Face::Dim), None);
    }
    if *cut {
        out.put(
            &format!("{indent}… cut at {} MiB\n", MAX_TEXT >> 20),
            Some(Face::Dim),
        );
    }
    out
}

/// `row` as it shows, its paths relative to `directory`, for a window `width` cells wide.
pub fn render(row: &Row, directory: &Path, width: usize) -> Rendered {
    if matches!(row.kind, Kind::Reply { .. }) {
        return tail(row, 0, 0);
    }
    let mut out = Rendered::default();
    if row.gap {
        out.put("\n", None);
    }
    let indent = GUIDE.repeat(usize::from(row.depth));
    let follow = format!("{indent}  ");
    match &row.kind {
        Kind::Prompt(text) => out.prompt(text),
        Kind::Reply { .. } => {}
        Kind::Thinking {
            text,
            millis,
            streaming,
        } => {
            let mark = match (streaming, row.folded || text.is_empty()) {
                (true, _) => '…',
                (false, true) => '▸',
                (false, false) => '▾',
            };
            let took = millis.filter(|_| !streaming);
            let took = took.map_or(String::new(), |millis| format!(" · {}", seconds(millis)));
            out.put(&format!("{indent}{mark} Thinking{took}\n"), Some(Face::Dim));
            if !row.folded && !text.is_empty() {
                out.lines(
                    text,
                    &format!("{indent}    "),
                    Some(Face::Dim),
                    Some(Face::Dim),
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
