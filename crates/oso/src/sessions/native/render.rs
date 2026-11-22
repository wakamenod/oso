//! A native session's row as text and faces (ADR 0165, ADR 0130's buffer design): the
//! person's messages after `› `, replies after `● `, thinking and tool calls as one line that
//! `TAB` unfolds, an edit as a diff, and each turn's result as a dim line.
//!
//! Must not depend on: the theme, the UI or files; the host maps [`Face`] to the theme's faces
//! and puts the text in the session's buffer.
//!
//! - A row's text ends with a newline. A turn's result ends with a blank line, which sets one
//!   turn apart from the next.
//! - A reply's text has no face of its own, so that Markdown's highlighting can lie over it.
//! - What comes from the model or a tool is shown as text, never read as anything: what is
//!   shown on a row's one line (a tool's name and target, a reason, a progress line) has its
//!   control characters turned to spaces, and is cut with `…`.
//! - A result longer than [`LONG_RESULT`] lines shows its first and last [`RESULT_EDGE`], and
//!   at most [`MAX_SHOWN_RESULT`] bytes; `RET` shows it whole (ADR 0130). Only what is shown is
//!   walked, so drawing a row costs what it shows, not what it holds. A text cut as it came
//!   says so.
//! - A reply that grows is drawn again from its last line only ([`tail`]).
//! - Paths under the session's directory are shown relative to it.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;

mod queued;

pub use queued::queue_rows;

use super::capped;
use super::rows::{Kind, Row, Tool, ToolState};
use super::turns::interrupted;
use super::wire::{Hunk, MAX_TEXT, ToolResult, TurnResult};

/// Past this many lines, a result shows only its edges.
pub const LONG_RESULT: usize = 200;
/// How many lines a long result shows at each end.
pub const RESULT_EDGE: usize = 50;
/// The most of a result shown, in bytes: half from its start, half from its end.
pub const MAX_SHOWN_RESULT: usize = 64 << 10;
/// The most of what a row's one line shows of a target, in bytes.
const MAX_TARGET: usize = 160;

/// What a stretch of a row's text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    /// A message of the person's.
    Prompt,
    /// The marks before a prompt, a reply or a row, and what is said less loudly.
    Dim,
    /// A tool's name.
    Tool,
    /// What a tool works on: a path, a command.
    Target,
    Added,
    Removed,
    /// A diff's hunk header.
    Hunk,
    Error,
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

    /// `text`'s lines, each after `first` or, past the first, `rest`; `face` on each line's
    /// text, if any.
    fn lines(
        &mut self,
        text: &str,
        first: &str,
        rest: &str,
        mark: Option<Face>,
        face: Option<Face>,
    ) {
        let mut any = false;
        for (at, line) in text.lines().enumerate() {
            self.put(if at == 0 { first } else { rest }, mark);
            self.put(line, face);
            self.put("\n", None);
            any = true;
        }
        if !any {
            self.put(first, mark);
            self.put("\n", None);
        }
    }
}

/// What follows the first `lines` lines of a reply row as drawn, from its text's byte `from`,
/// where a line starts: what [`RowChange::Grew`](super::rows::RowChange::Grew) says to draw
/// again. Together they draw what [`render`] draws, from the last line only.
pub fn tail(row: &Row, lines: usize, from: usize) -> Rendered {
    let mut out = Rendered::default();
    let Kind::Reply { text, cut, .. } = &row.kind else {
        return out;
    };
    let from = text.floor_char_boundary(from.min(text.len()));
    let rest = &text[from..];
    let indent = "  ".repeat(usize::from(row.depth));
    if !(lines > 0 && rest.is_empty()) {
        let follow = format!("{indent}  ");
        let first = if lines == 0 {
            format!("{indent}● ")
        } else {
            follow.clone()
        };
        out.lines(rest, &first, &follow, Some(Face::Dim), None);
    }
    if *cut {
        out.put(
            &format!("{indent}  … cut at {} MiB\n", MAX_TEXT >> 20),
            Some(Face::Dim),
        );
    }
    out
}

/// `row` as it shows, its paths relative to `directory`.
pub fn render(row: &Row, directory: &Path) -> Rendered {
    let mut out = Rendered::default();
    let indent = "  ".repeat(usize::from(row.depth));
    let follow = format!("{indent}  ");
    match &row.kind {
        Kind::Prompt(text) => {
            let first = format!("{indent}› ");
            out.lines(text, &first, &follow, Some(Face::Dim), Some(Face::Prompt));
        }
        Kind::Reply { .. } => out = tail(row, 0, 0),
        Kind::Thinking {
            text,
            millis,
            streaming,
        } => {
            let head = match (streaming, millis) {
                (true, _) => "∴ Thinking…".to_owned(),
                (false, Some(millis)) => format!("∴ Thought for {}", seconds(*millis)),
                (false, None) => "∴ Thought".to_owned(),
            };
            out.put(&format!("{indent}{head}\n"), Some(Face::Dim));
            if !row.folded && !text.is_empty() {
                let body = format!("{indent}    ");
                out.lines(text, &body, &body, None, Some(Face::Dim));
            }
        }
        Kind::Tools(tools) => match tools.as_slice() {
            [tool] => tool_row(&mut out, tool, row.folded, &indent, directory),
            _ => group_row(&mut out, tools, row.folded, &indent, directory),
        },
        Kind::Note(text) => out.lines(text, &follow, &follow, None, Some(Face::Dim)),
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
            out.put(&format!("  {}\n\n", footer(result)), Some(face));
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

/// One call: its line, then, unfolded, its input and its result.
fn tool_row(out: &mut Rendered, tool: &Tool, folded: bool, indent: &str, directory: &Path) {
    let opens = tool
        .result
        .as_ref()
        .is_some_and(|result| !result.text.is_empty() || !result.patch.is_empty())
        || shown_input(tool).is_some();
    let mark = match (&tool.state, opens, folded) {
        (ToolState::Streaming | ToolState::Running, ..) => "◌ ",
        (_, false, _) => "· ",
        (_, true, true) => "▸ ",
        (_, true, false) => "▾ ",
    };
    out.put(indent, None);
    out.put(mark, Some(Face::Dim));
    out.put(&one_line(&tool.name), Some(Face::Tool));
    let target = target(tool, directory);
    if !target.is_empty() {
        out.put(" ", None);
        out.put(&target, Some(Face::Target));
    }
    if tool.input.cut {
        out.put(" · input cut", Some(Face::Error));
    }
    if tool.apart {
        out.put(" · outside the project", Some(Face::Error));
    }
    let (suffix, face) = suffix(tool);
    out.put(&suffix, Some(face));
    out.put("\n", None);
    if !folded {
        call_body(out, tool, &format!("{indent}    "));
    }
}

/// An unfolded call's input and result, each line after `body`.
fn call_body(out: &mut Rendered, tool: &Tool, body: &str) {
    if let Some(input) = shown_input(tool) {
        out.lines(&input, body, body, None, Some(Face::Dim));
    }
    let Some(result) = &tool.result else {
        return;
    };
    if !result.patch.is_empty() {
        diff(out, &result.patch, body);
        return;
    }
    let face = Some(if result.error { Face::Error } else { Face::Dim });
    match edges(result) {
        Shown::Whole(text) => out.lines(text, body, body, None, face),
        Shown::Edges { head, note, tail } => {
            out.lines(head, body, body, None, face);
            out.lines(&note, body, body, None, Some(Face::Dim));
            out.lines(tail, body, body, None, face);
        }
    }
}

/// Calls of one tool that share a row: `Read 3 files · a.rs, b.rs, c.rs`, then, unfolded,
/// each call as its own line under it, folded or not on its own.
fn group_row(out: &mut Rendered, tools: &[Tool], folded: bool, indent: &str, directory: &Path) {
    let count =
        |state: fn(&ToolState) -> bool| tools.iter().filter(|tool| state(&tool.state)).count();
    let running = count(|state| matches!(state, ToolState::Streaming | ToolState::Running));
    out.put(indent, None);
    let mark = match (running > 0, folded) {
        (true, _) => "◌ ",
        (false, true) => "▸ ",
        (false, false) => "▾ ",
    };
    out.put(mark, Some(Face::Dim));
    let name = tools.first().map_or("", |tool| tool.name.as_str());
    out.put(&one_line(name), Some(Face::Tool));
    let noun = if name == "Read" { "files" } else { "patterns" };
    out.put(&format!(" {} {noun}", tools.len()), Some(Face::Dim));
    let mut targets = String::new();
    for target in tools.iter().map(|tool| target(tool, directory)) {
        if target.is_empty() {
            continue;
        }
        if !targets.is_empty() {
            targets.push_str(", ");
        }
        targets.push_str(&target);
        if targets.len() > MAX_TARGET {
            break;
        }
    }
    if !targets.is_empty() {
        out.put(" · ", Some(Face::Dim));
        out.put(&one_line(&targets), Some(Face::Target));
    }
    let apart = tools.iter().filter(|tool| tool.apart).count();
    for (n, what, face) in [
        (apart, "outside the project", Face::Error),
        (
            count(|state| matches!(state, ToolState::Denied(_))),
            "denied",
            Face::Error,
        ),
        (
            count(|state| *state == ToolState::Failed),
            "failed",
            Face::Error,
        ),
        (
            count(|state| *state == ToolState::Unfinished),
            "unfinished",
            Face::Dim,
        ),
        (running, "running", Face::Dim),
    ] {
        if n > 0 {
            out.put(&format!(" · {n} {what}"), Some(face));
        }
    }
    out.put("\n", None);
    if folded {
        return;
    }
    let inner = format!("{indent}  ");
    for (slot, tool) in tools.iter().enumerate() {
        let start = out.text.len();
        tool_row(out, tool, tool.folded, &inner, directory);
        out.calls.push((slot, start..out.text.len()));
    }
}

/// What follows a tool's line: what came of it.
fn suffix(tool: &Tool) -> (String, Face) {
    match &tool.state {
        ToolState::Streaming => (format!(" · {} bytes…", tool.streamed), Face::Dim),
        ToolState::Running => {
            let progress = tool
                .progress
                .as_deref()
                .map(|text| format!(" · {}", one_line(text)))
                .unwrap_or_default();
            (format!("{progress} · running"), Face::Dim)
        }
        ToolState::Denied(message) => (format!(" · denied: {}", one_line(message)), Face::Error),
        ToolState::Failed => (" · error".to_owned(), Face::Error),
        ToolState::Unfinished => (" · unfinished".to_owned(), Face::Dim),
        ToolState::Done => match &tool.result {
            Some(result) if !result.patch.is_empty() => {
                let (added, removed) = result.changed;
                (format!("  +{added} −{removed}"), Face::Dim)
            }
            Some(result) if tool.name == "Read" => {
                (format!(" · {} lines", result.lines), Face::Dim)
            }
            _ => (String::new(), Face::Dim),
        },
    }
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
        "TodoWrite" => input
            .items
            .map(|items| format!("{items} items"))
            .unwrap_or_default(),
        _ => path.or_else(|| text.map(str::to_owned)).unwrap_or_default(),
    };
    one_line(&shown)
}

/// The input an unfolded call shows above its result: a command its line did not show whole,
/// or an unknown tool's input.
fn shown_input(tool: &Tool) -> Option<String> {
    if tool.name == "Bash" {
        let command = tool.input.text.as_deref()?;
        let long = command.contains('\n') || command.len() > MAX_TARGET;
        return long.then(|| format!("$ {command}"));
    }
    tool.input.detail.clone()
}

fn diff(out: &mut Rendered, patch: &[Hunk], body: &str) {
    for hunk in patch {
        out.put(body, None);
        let header = format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        );
        out.put(&header, Some(Face::Hunk));
        out.put("\n", None);
        for line in &hunk.lines {
            let face = match line.as_bytes().first() {
                Some(b'+') => Face::Added,
                Some(b'-') => Face::Removed,
                _ => Face::Dim,
            };
            out.put(body, None);
            out.put(line, Some(face));
            out.put("\n", None);
        }
    }
}

/// What a result shows.
#[derive(Debug, PartialEq, Eq)]
pub enum Shown<'a> {
    Whole(&'a str),
    /// Its first and last lines, and a line saying what is left out between them.
    Edges {
        head: &'a str,
        note: String,
        tail: &'a str,
    },
}

/// A result's text, or past [`LONG_RESULT`] lines or [`MAX_SHOWN_RESULT`] bytes its first and
/// last [`RESULT_EDGE`] lines, each end at most half the bytes. It walks only what it shows.
/// A text cut as it came says so too.
pub fn edges(result: &ToolResult) -> Shown<'_> {
    let text = &result.text;
    if result.lines <= LONG_RESULT && text.len() <= MAX_SHOWN_RESULT && !result.cut {
        return Shown::Whole(text);
    }
    let half = MAX_SHOWN_RESULT / 2;
    // Only what is shown is walked: a newline past `half` from either end changes nothing.
    let near_start = &text[..text.floor_char_boundary(half)];
    let head_end = near_start
        .match_indices('\n')
        .nth(RESULT_EDGE - 1)
        .map_or(near_start.len(), |(at, _)| at);
    let head = capped(&text[..head_end], half).0;
    let body = text.strip_suffix('\n').unwrap_or(text);
    let near = body.ceil_char_boundary(body.len().saturating_sub(half));
    let tail_start = body[near..]
        .rmatch_indices('\n')
        .nth(RESULT_EDGE - 1)
        .map_or(0, |(at, _)| near + at + 1)
        .max(head.len() + usize::from(text[head.len()..].starts_with('\n')))
        .max(text.ceil_char_boundary(text.len().saturating_sub(half)));
    let tail = &text[tail_start..];
    let skipped = result
        .lines
        .saturating_sub(head.lines().count() + tail.lines().count());
    let mib = MAX_TEXT >> 20;
    let note = match (skipped, result.cut) {
        (0, false) => "… cut: RET shows it all …".to_owned(),
        (0, true) => format!("… cut at {mib} MiB as it came …"),
        (skipped, false) => format!("… {skipped} more lines: RET shows them all …"),
        (skipped, true) => format!("… {skipped} more lines, cut at {mib} MiB as it came …"),
    };
    Shown::Edges { head, note, tail }
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
