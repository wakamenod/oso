//! A native session's tool rows as text and faces (ADR 0165, "The rows' look"): a call's one
//! line, its mark, its badge, its name and what it works on, then, unfolded, its input and
//! its result or its change; and calls that share a row.
//!
//! Must not depend on: the theme, the UI or files; plain functions over a row.
//!
//! - A row's mark is in its row's first column, where no text of Claude's or of a tool's
//!   stands: what an unfolded call shows is four columns further in, and a nested row, a
//!   subagent's or an open row's call, has oso's [`GUIDE`] there.
//! - On a row and in what it shows, what Claude or a tool says is dim, and oso's own words are
//!   not, so that neither a command nor a result can spell an outcome as oso says it.
//! - A change shows at most [`MAX_DIFF_SHOWN`] lines, each cut at [`MAX_DIFF_LINE`] bytes, and
//!   a result at most [`RESULT_LINES`]; either at most [`MAX_SHOWN_RESULT`] bytes of text.
//!   `RET` shows it whole. Only what is shown is walked, and a change's hunks counted.

use std::path::Path;

use oso_core::editor::Approval;

use super::{Face, GUIDE, MAX_TARGET, Rendered, capped, one_line, target};
use crate::sessions::native::rows::{Tool, ToolState};
use crate::sessions::native::wire::{Hunk, MAX_TEXT, ToolResult};

/// The most lines of a result a row shows, its first.
pub(in crate::sessions::native) const RESULT_LINES: usize = 12;
/// The most of a result or a change a row shows, in bytes.
pub(in crate::sessions::native) const MAX_SHOWN_RESULT: usize = 8 << 10;
/// The most lines of a change a row shows, its first.
pub(in crate::sessions::native) const MAX_DIFF_SHOWN: usize = 40;
/// The most of a change's line a row shows, in bytes.
pub(in crate::sessions::native) const MAX_DIFF_LINE: usize = 1 << 10;

/// One call: its line, then, unfolded, its input and its result.
pub(super) fn tool_row(
    out: &mut Rendered,
    tool: &Tool,
    folded: bool,
    indent: &str,
    directory: &Path,
) {
    let opens = tool
        .result
        .as_ref()
        .is_some_and(|result| !result.text.is_empty() || !result.patch.is_empty())
        || shown_input(tool).is_some();
    let (mark, face) = match (&tool.state, opens, folded) {
        (ToolState::Streaming | ToolState::Running, ..) => ("… ", Face::Dim),
        (ToolState::Failed | ToolState::Denied(_), ..) => ("✗ ", Face::Error),
        (_, false, _) => ("· ", Face::Dim),
        (_, true, true) => ("▸ ", Face::Dim),
        (_, true, false) => ("▾ ", Face::Dim),
    };
    head(out, indent, (mark, face), &tool.name);
    let target = target(tool, directory);
    if !target.is_empty() {
        out.put(" · ", Some(Face::Dim));
        out.put(&squeezed(&target), Some(Face::Dim));
    }
    if tool.input.cut {
        out.put(" · input cut", Some(Face::Error));
    }
    if tool.apart {
        out.put(" · outside the project", Some(Face::Error));
    }
    let progress = tool
        .progress
        .as_deref()
        .filter(|_| tool.state == ToolState::Running && !tool.waiting && tool.approved.is_none());
    if let Some(progress) = progress {
        out.put(" · ", Some(Face::Dim));
        out.put(&squeezed(&one_line(progress)), Some(Face::Dim));
    }
    let (suffix, face) = suffix(tool, folded);
    out.put(&suffix, face);
    out.put("\n", None);
    if !folded {
        call_body(out, tool, &format!("{indent}    "));
    }
}

/// How a tool row starts: its mark in its first column, its badge and its name.
fn head(out: &mut Rendered, indent: &str, (mark, face): (&str, Face), name: &str) {
    out.put(indent, Some(Face::Dim));
    out.put(mark, Some(face));
    out.put(badge(name), Some(Face::Tool));
    out.put(" ", Some(Face::Tool));
    out.put(&one_line(name), Some(Face::Tool));
}

/// The badge of the tool `name`: one plain character, one cell wide.
fn badge(name: &str) -> &'static str {
    match name {
        "Read" => "≡",
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => "✎",
        "Bash" => "$",
        "Grep" | "Glob" => "/",
        "Agent" | "Task" => "@",
        "WebFetch" | "WebSearch" => "~",
        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList" | "TaskGet" => "☑",
        "AskUserQuestion" => "?",
        _ => "*",
    }
}

/// An unfolded call's input and result, each line after `body`.
fn call_body(out: &mut Rendered, tool: &Tool, body: &str) {
    if let Some(input) = shown_input(tool) {
        out.lines(&input, body, Some(Face::Dim), Some(Face::Dim));
    }
    let Some(result) = &tool.result else {
        return;
    };
    if !result.patch.is_empty() {
        diff(out, result, body);
        return;
    }
    let face = Some(if result.error { Face::Error } else { Face::Dim });
    let (head, note) = first_lines(result);
    out.lines(head, body, Some(Face::Dim), face);
    if let Some(note) = note {
        out.lines(&note, body, Some(Face::Dim), None);
    }
}

/// Calls of one tool that share a row: `Read ×3 · a.rs, b.rs, c.rs`, then, unfolded, each
/// call as its own line under it, folded or not on its own.
pub(super) fn group_row(
    out: &mut Rendered,
    tools: &[Tool],
    folded: bool,
    indent: &str,
    directory: &Path,
) {
    let count = |is: fn(&Tool) -> bool| tools.iter().filter(|tool| is(tool)).count();
    let running = count(|t| matches!(t.state, ToolState::Streaming | ToolState::Running));
    let failed = count(|t| t.state == ToolState::Failed);
    let denied = count(|t| matches!(t.state, ToolState::Denied(_)));
    let mark = match (running > 0, failed + denied > 0, folded) {
        (true, ..) => ("… ", Face::Dim),
        (_, true, _) => ("✗ ", Face::Error),
        (_, _, true) => ("▸ ", Face::Dim),
        (_, _, false) => ("▾ ", Face::Dim),
    };
    let name = tools.first().map_or("", |tool| tool.name.as_str());
    head(out, indent, mark, name);
    out.put(&format!(" ×{}", tools.len()), Some(Face::Tool));
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
        out.put(&squeezed(&one_line(&targets)), Some(Face::Dim));
    }
    for (n, what, face) in [
        (count(|t| t.apart), "outside the project", Some(Face::Error)),
        (denied, "denied", Some(Face::Error)),
        (failed, "failed", Some(Face::Error)),
        (
            count(|t| t.state == ToolState::Unfinished),
            "unfinished",
            None,
        ),
        (
            count(|t| t.waiting && t.state == ToolState::Running),
            "waiting for you",
            Some(Face::Waiting),
        ),
    ] {
        if n > 0 {
            out.put(&format!(" · {n} {what}"), face);
        }
    }
    out.put("\n", None);
    if folded {
        return;
    }
    let inner = format!("{indent}{GUIDE}");
    for (slot, tool) in tools.iter().enumerate() {
        let start = out.text.len();
        tool_row(out, tool, tool.folded, &inner, directory);
        out.calls.push((slot, start..out.text.len()));
    }
}

/// What follows a tool's line: what came of it. An unfolded change says its counts above its
/// lines instead.
fn suffix(tool: &Tool, folded: bool) -> (String, Option<Face>) {
    if tool.waiting && tool.state == ToolState::Running {
        return (" · waiting for you".to_owned(), Some(Face::Waiting));
    }
    // The person's approval, on its card (part 3d), until it fails.
    if let Some((edited, after)) = tool.approved
        && matches!(tool.state, ToolState::Running | ToolState::Done)
    {
        let edited = if edited { ", edited" } else { "" };
        let after = match after {
            Approval::Manual => "",
            Approval::AcceptEdits => ", accept edits",
            Approval::Auto => ", auto",
        };
        return (format!(" · approved{edited}{after}"), None);
    }
    match &tool.state {
        ToolState::Streaming => (format!(" · {} bytes", tool.streamed), None),
        ToolState::Running => (String::new(), None),
        ToolState::Denied(message) => (
            format!(" · denied: {}", one_line(message)),
            Some(Face::Error),
        ),
        ToolState::Failed => (" · error".to_owned(), Some(Face::Error)),
        ToolState::Unfinished => (" · unfinished".to_owned(), None),
        ToolState::Done => match &tool.result {
            Some(result) if !result.patch.is_empty() && folded => {
                let (added, removed) = result.changed;
                (format!("  +{added} −{removed}"), None)
            }
            Some(result) if tool.name == "Read" => (format!("  {}", counted(result.lines)), None),
            _ => (String::new(), None),
        },
    }
}

/// `text`, on one line, with its runs of spaces as one, so that none pads what follows to a
/// row of its own.
fn squeezed(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        let blank = c.is_whitespace();
        if !(blank && space) {
            line.push(if blank { ' ' } else { c });
        }
        space = blank;
    }
    line
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

/// A change: how many lines it adds and removes, then its first lines, each after its number:
/// an added or a kept line's in the file as it is now, a removed one's as it was.
fn diff(out: &mut Rendered, result: &ToolResult, body: &str) {
    let (added, removed) = result.changed;
    out.put(body, Some(Face::Dim));
    let counts = format!("Added {}, removed {}", counted(added), counted(removed));
    out.put(&counts, None);
    out.put("\n", None);
    // At most this many hunks show a line.
    let hunks = result.patch.iter().take(MAX_DIFF_SHOWN);
    let digits = hunks.clone().map(last_number).max().unwrap_or(0);
    let digits = digits.to_string().len().max(4);
    let total: usize = result.patch.iter().map(|hunk| hunk.lines.len()).sum();
    let (mut shown, mut bytes) = (0, MAX_SHOWN_RESULT);
    for (at, hunk) in hunks.enumerate() {
        if shown == MAX_DIFF_SHOWN || bytes == 0 {
            break;
        }
        if at > 0 {
            out.put(&format!("{body}{:>digits$}\n", "⋮"), Some(Face::Dim));
        }
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            if shown == MAX_DIFF_SHOWN || bytes == 0 {
                break;
            }
            shown += 1;
            let sign = line.chars().next().unwrap_or(' ');
            let (face, number) = match sign {
                '+' => (Some(Face::Added), Some(&mut new)),
                '-' => (Some(Face::Removed), Some(&mut old)),
                ' ' => {
                    old = old.saturating_add(1);
                    (None, Some(&mut new))
                }
                // Such as `\ No newline at end of file`, which is no line of the file.
                _ => (Some(Face::Dim), None),
            };
            let number = number.map_or(String::new(), |number| {
                let shown = number.to_string();
                *number = number.saturating_add(1);
                shown
            });
            let (sign, rest) = match face {
                Some(Face::Dim) => (' ', line.as_str()),
                _ => (sign, line.get(1..).unwrap_or("")),
            };
            let (kept, cut) = capped(rest, MAX_DIFF_LINE.min(bytes));
            bytes -= kept.len();
            // A changed line's face is on its indent, which its band is drawn by, and on its
            // sign and text; its number is not in it, so that no text can pass for one.
            let numbered = format!("{number:>digits$} ");
            let signed = format!("{sign} ");
            let (numbered, signed) = match (kept.is_empty(), sign) {
                (true, ' ') => (numbered.trim_end(), ""),
                (true, _) => (numbered.as_str(), signed.trim_end()),
                _ => (numbered.as_str(), signed.as_str()),
            };
            out.put(body, face.or(Some(Face::Dim)));
            out.put(numbered, Some(Face::Dim));
            out.put(signed, face);
            out.put(kept, face);
            if cut {
                out.put("…", face);
            }
            out.put("\n", None);
        }
    }
    if total > shown {
        let more = more(total - shown);
        out.put(body, Some(Face::Dim));
        out.put(&more, None);
        out.put("\n", None);
    }
}

/// The largest line number `hunk` shows.
fn last_number(hunk: &Hunk) -> u64 {
    let lines = u64::try_from(hunk.lines.len()).unwrap_or(u64::MAX);
    hunk.old_start.max(hunk.new_start).saturating_add(lines)
}

/// `1 line`, `2 lines`.
fn counted(lines: usize) -> String {
    format!("{lines} line{}", if lines == 1 { "" } else { "s" })
}

/// What says that `lines` more are not shown.
fn more(lines: usize) -> String {
    format!("… {} more: RET shows all …", counted(lines))
}

/// A result's first [`RESULT_LINES`] lines, at most [`MAX_SHOWN_RESULT`] bytes of them, and
/// what says that there is more, or that its text was cut as it came. It walks only what it
/// shows.
pub(in crate::sessions::native) fn first_lines(result: &ToolResult) -> (&str, Option<String>) {
    let text = &result.text;
    let near = capped(text, MAX_SHOWN_RESULT).0;
    let end = (near.match_indices('\n').nth(RESULT_LINES - 1)).map_or(near.len(), |(at, _)| at);
    let head = &text[..end];
    if matches!(&text[end..], "" | "\n") && !result.cut {
        return (head, None);
    }
    let skipped = result.lines.saturating_sub(head.lines().count());
    let mib = MAX_TEXT >> 20;
    let note = match (skipped, result.cut) {
        (0, false) => "… cut: RET shows it all …".to_owned(),
        (0, true) => format!("… cut at {mib} MiB as it came …"),
        (skipped, false) => more(skipped),
        (skipped, true) => format!("… {} more, cut at {mib} MiB as it came …", counted(skipped)),
    };
    (head, Some(note))
}
