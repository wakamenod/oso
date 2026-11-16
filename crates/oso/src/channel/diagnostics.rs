//! The agent channel's `diagnostics` (ADR 0138, What agents may do, step 11): the language
//! servers' diagnostics of a buffer, or of its whole project, as the editor holds them, read like
//! `text`.
//!
//! Must not depend on: sockets, threads or `app`; the caller hands over the servers' reports of
//! files no buffer holds, and this is a plain function of them and the editor.
//!
//! - A buffer's own come first, by place, then each other file's by path and line, open in a
//!   buffer or not, from every server, as `M-g d` lists them; hints are left out, as the counts
//!   leave them out. At most [`MAX_ENTRIES`], with `more` saying how many were left out.
//! - An entry gives the path, the line and the column from 1, the severity, the message, the
//!   source and the code, and the server that reported it. An open buffer's column is in
//!   characters; a file no buffer holds gives the server's own, with `column_unit` saying how it
//!   counts, as turning it into characters would read the file.
//! - A message is a server's text, which may be written to steer the agent: at most
//!   [`MESSAGE_BYTES`], cut at a character, its line breaks kept, and every other control, bidi or
//!   invisible character written as `\u{…}`, as a sent path's are (ADR 0121).
//! - Never of a buffer that may hold secrets, or one withheld from agents, as `text` refuses
//!   them; such buffers are left out of a project's too. The servers' reports of files whose
//!   names may hold secrets were dropped before they were kept (step 6a).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::diagnostic::{Diagnostics, Severity};
use oso_core::display_map::cell::is_invisible;
use oso_core::editor::Editor;
use oso_lsp::position::Encoding;
use oso_lsp::protocol::Published;
use serde_json::{Value, json};

use super::protocol::DiagnosticsOf;
use super::server::Reply;
use super::state::unreadable;

/// The most entries one call gives (ADR 0138).
pub const MAX_ENTRIES: usize = 500;

/// The most bytes of a message an entry gives (ADR 0138).
pub const MESSAGE_BYTES: usize = 2 << 10;

/// Why a buffer that may hold secrets gives none, or one withheld from agents.
const WITHHELD: &str = "the buffer may hold secrets, so its diagnostics stay in the editor";
const PERSONS: &str = "only the person reads that buffer, so its diagnostics stay in the editor";

/// A server's report of a file no buffer holds, with the encoding of its positions and the
/// server's name.
pub struct FileReport<'a> {
    pub path: &'a Path,
    pub report: &'a Published,
    pub encoding: Encoding,
    pub server: &'a str,
}

/// Where an entry comes from.
enum Source<'a> {
    Open(&'a Buffer, &'a Diagnostics),
    File(&'a FileReport<'a>),
}

/// The diagnostics `of` asks for: of a buffer, or of its project with `files`, the servers'
/// reports of files no buffer holds.
pub fn diagnostics(editor: &Editor, files: &[FileReport<'_>], of: DiagnosticsOf) -> Reply {
    let (id, buffer) = match of.buffer {
        Some(id) => editor
            .buffers()
            .find(|(own, buffer)| own.get() == id && !buffer.agent_pending())
            .ok_or_else(|| format!("no buffer {id}"))?,
        None => {
            let (window, buffer) = editor.tiled();
            (window.buffer(), buffer)
        }
    };
    refuse(buffer)?;
    let mut sources: Vec<(String, Source<'_>)> = Vec::new();
    let own = buffer.diagnostics().map(|set| Source::Open(buffer, set));
    let root = if of.project {
        let root = buffer
            .project()
            .and_then(|project| editor.project(project))
            .map(|project| project.root().to_path_buf())
            .ok_or("the buffer is in no project")?;
        project_sources(editor, (id, &root), files, &mut sources);
        Some(root)
    } else {
        None
    };
    sources.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut entries = Vec::new();
    let mut total = 0;
    for source in own.iter().chain(sources.iter().map(|(_, source)| source)) {
        total += push(source, &mut entries);
    }
    Ok(json!({
        "buffer": id.get(),
        "project": root.map(|root| root.to_string_lossy().into_owned()),
        "entries": entries,
        "more": total - entries.len(),
    }))
}

/// Refuses a buffer whose diagnostics the channel may not read.
fn refuse(buffer: &Buffer) -> Result<(), String> {
    if buffer.is_secret() {
        Err(WITHHELD.into())
    } else if unreadable(buffer) {
        Err(PERSONS.into())
    } else {
        Ok(())
    }
}

/// The other open buffers of the project at `root` and the reports of its files no buffer
/// holds, into `sources`, by their paths, but buffer `id`'s, which comes first.
fn project_sources<'a>(
    editor: &'a Editor,
    (id, root): (BufferId, &Path),
    files: &'a [FileReport<'a>],
    sources: &mut Vec<(String, Source<'a>)>,
) {
    for (other, buffer) in editor.buffers() {
        if other == id || unreadable(buffer) || buffer.agent_pending() {
            continue;
        }
        if let (Some(path), Some(set)) = (buffer.path(), buffer.diagnostics())
            && path.starts_with(root)
        {
            sources.push((
                path.to_string_lossy().into_owned(),
                Source::Open(buffer, set),
            ));
        }
    }
    // The root as a server's URI reads back, as the servers' paths are spelled.
    let server_root: PathBuf = oso_lsp::uri::from_path(root)
        .and_then(|uri| oso_lsp::uri::to_path(&uri))
        .unwrap_or_else(|| root.to_path_buf());
    for file in files {
        if file.path.starts_with(&server_root) {
            let path = file.path.to_string_lossy().into_owned();
            sources.push((path, Source::File(file)));
        }
    }
}

/// Pushes `source`'s entries, hints left out, while there is room, and gives how many it has.
fn push(source: &Source<'_>, entries: &mut Vec<Value>) -> usize {
    match source {
        Source::Open(buffer, set) => {
            let counts = set.counts();
            let text = buffer.text();
            let path = buffer.path().map_or_else(
                || buffer.name().to_owned(),
                |path| path.to_string_lossy().into(),
            );
            for diagnostic in set.iter().filter(|d| d.severity() != Severity::Hint) {
                if entries.len() == MAX_ENTRIES {
                    break;
                }
                let start = diagnostic.range().start;
                let point = text.offset_to_point(start);
                let line_start = text.line_start(point.row);
                // O(log n): a slice shares the rope's chunks, however long the line.
                let column = text.slice(line_start..start).char_count();
                entries.push(json!({
                    "path": path,
                    "line": point.row + 1,
                    "column": column + 1,
                    "severity": severity(diagnostic.severity()),
                    "message": for_agent(diagnostic.message()),
                    "source": for_agent(diagnostic.source()),
                    "code": for_agent(diagnostic.code()),
                    "server": set.server(),
                }));
            }
            [counts.errors, counts.warnings, counts.information]
                .into_iter()
                .map(|count| usize::try_from(count).unwrap_or(usize::MAX))
                .sum()
        }
        Source::File(file) => {
            let mut kept: Vec<_> = (file.report.diagnostics.iter())
                .filter(|reported| reported.severity != Severity::Hint)
                .collect();
            let count = kept.len();
            kept.sort_by_key(|reported| (reported.start.line, reported.start.character));
            let path = file.path.to_string_lossy();
            let unit = match file.encoding {
                Encoding::Utf8 => "utf-8",
                Encoding::Utf16 => "utf-16",
            };
            for reported in kept {
                if entries.len() == MAX_ENTRIES {
                    break;
                }
                entries.push(json!({
                    "path": path,
                    "line": reported.start.line as usize + 1,
                    "column": reported.start.character as usize + 1,
                    "column_unit": unit,
                    "severity": severity(reported.severity),
                    "message": for_agent(&reported.message),
                    "source": for_agent(&reported.source),
                    "code": for_agent(&reported.code),
                    "server": file.server,
                }));
            }
            count
        }
    }
}

/// The word an entry gives a severity.
fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
    }
}

/// A server's text as the agent gets it: at most [`MESSAGE_BYTES`] of it, cut at a character,
/// its line breaks kept, and every other control, bidi or invisible character as `\u{…}`.
pub(crate) fn for_agent(text: &str) -> String {
    let kept = &text[..text.floor_char_boundary(MESSAGE_BYTES)];
    let mut out = String::with_capacity(kept.len());
    for c in kept.chars() {
        if (c.is_control() && c != '\n') || is_invisible(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
