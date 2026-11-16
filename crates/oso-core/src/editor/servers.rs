//! What commands ask of the host about the language server of the selected buffer
//! (ADR 0138, step 3): `lsp-start`, `lsp-restart` and `lsp-shutdown`; and `*LSP*`, the log of
//! what servers did, which keeps the last bytes the host says; and what the echo area says of
//! the diagnostics at point (step 6), and the list of a project's (`M-g d`, step 6b,
//! `diagnostics_list`, sharing `listed` with the list of an answer's places); the jumps an
//! answer leads to, and back (step 8, `jumps`), and the list of an answer's places (step 8b,
//! `xref_list`); and the completion popup (step 9, `popup`).
//!
//! Must not depend on: the host's servers, threads or I/O; a command asks, and the host takes
//! the request after the command and acts on the server.

mod diagnostics_list;
mod jumps;
mod listed;
mod popup;
mod xref_list;

use crate::buffer::{Buffer, BufferId};

/// Where on a line a server's place is: the host turns the server's encoding into one.
pub use super::visits::VisitColumn;
pub use diagnostics_list::{DiagnosticRow, DiagnosticRows, DiagnosticsRequest};
pub use jumps::AskAt;
pub use listed::{ListedAt, ListedHit};
pub use popup::{
    COMPLETION_POPUP_COLUMNS, COMPLETION_POPUP_ROWS, CompletionPopup, CompletionPopupAnswer,
    CompletionPopupItem, CompletionTyped,
};
pub use xref_list::{XrefKind, XrefRow, XrefRows};

use super::Editor;
use super::messages::Keep;

/// The name of the servers' log.
pub(crate) const SERVER_LOG: &str = "*LSP*";

/// What commands asked of the host, and the log's buffer once made.
#[derive(Debug, Default)]
pub(super) struct Servers {
    requests: Vec<ServerRequest>,
    /// The list of a project's diagnostics, `M-g d` (step 6b).
    pub(super) list: diagnostics_list::DiagnosticsList,
    /// The list of a server's places, `M-.`'s and `M-?`'s (step 8b).
    pub(super) xrefs: xref_list::XrefList,
    /// The completion popup at point (step 9).
    popup: Option<popup::CompletionPopup>,
    /// An item inserted before its resolve came, whose other edits may still come (step 9b).
    inserted: Option<popup::Inserted>,
    /// What the person last typed, for the host to judge whether to complete (step 9b).
    typed: Option<popup::CompletionTyped>,
    log: Option<BufferId>,
}

/// What a command asks of the host about the server of a buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerRequest {
    /// `lsp-start`: start the buffer's server, if none runs.
    Start(BufferId),
    /// `lsp-restart`: stop it if it runs, and start it again with the configuration as it is.
    Restart(BufferId),
    /// `lsp-shutdown`: stop it, and keep it stopped until `lsp-start` or `lsp-restart`.
    ShutDown(BufferId),
    /// `lsp-find-definition` (step 8).
    Definition(AskAt),
    /// `lsp-find-references` (step 8).
    References(AskAt),
    /// `completion-at-point`: ask for the completions at the primary cursor (step 9).
    Complete(AskAt),
    /// `lsp-go-to-xref`: go to the row the list of a server's places took
    /// ([`Editor::take_xref_hit`]), as an answer's jump goes (step 8b).
    GoToXref,
}

impl Editor {
    /// A command asks the host for `request`.
    pub fn request_server(&mut self, request: ServerRequest) {
        self.servers.requests.push(request);
    }

    /// Whether the innermost prompt is `M-g d`'s list or a list of places, and its rows were cut.
    pub(super) fn server_list_cut(&self) -> bool {
        self.diagnostics_cut() || self.xrefs_cut()
    }

    /// Whether the innermost prompt is `M-g d`'s list or a list of places, waiting for its rows.
    pub(super) fn server_list_pending(&self) -> bool {
        self.diagnostics_pending() || self.xrefs_pending()
    }

    /// What servers' answers left in `buffer` follows an edit's `steps`, as the cursors do:
    /// each window's history of jumps (step 8) and the completion popup's word (step 9).
    pub(super) fn answers_follow(
        &mut self,
        buffer: BufferId,
        steps: &[Vec<crate::selection::OffsetEdit>],
    ) {
        self.follow_jump_history(buffer, steps);
        self.completion_popup_follows(buffer, steps);
    }

    /// `buffer` was read again, its offsets moved through `map`: the jumps' places move, and
    /// the popup, which no longer knows its word, closes.
    pub(super) fn answers_reloaded(&mut self, buffer: BufferId, map: impl FnMut(usize) -> usize) {
        self.map_jump_history(buffer, map);
        self.completion_popup_reloaded(buffer);
    }

    /// What commands asked about servers since the host last looked, oldest first.
    pub fn take_server_requests(&mut self) -> Vec<ServerRequest> {
        std::mem::take(&mut self.servers.requests)
    }

    /// Adds `rows` to `*LSP*`, made when first needed, in one edit, each after `label` in
    /// brackets, escaped as `*Messages*`' rows are, keeping its last `max_bytes` in whole rows.
    pub fn server_log<'a>(
        &mut self,
        label: &str,
        rows: impl IntoIterator<Item = &'a str>,
        max_bytes: usize,
    ) {
        let label = super::messages::row_text(label);
        let mut text = String::new();
        for row in rows {
            text.push('[');
            text.push_str(&label);
            text.push_str("] ");
            // Escaped as `*Messages*`' rows are, so that no control or bidi character in
            // a server's text reaches a copy or the channel as it is.
            text.push_str(&super::messages::row_text(row));
            text.push('\n');
        }
        if text.is_empty() {
            return;
        }
        let id = match self.servers.log {
            Some(id) if self.buffer(id).is_some() => id,
            _ => {
                // A server's text can quote what it read, files that may hold secrets among
                // them: the agent channel never reads it.
                let mut buffer = Buffer::internal(SERVER_LOG);
                buffer.withhold_from_agents();
                let id = self.add_buffer(buffer);
                self.servers.log = Some(id);
                id
            }
        };
        self.add_rows(id, &text, Keep::Bytes(max_bytes));
    }

    /// Whether a window shows `*LSP*`, so that rows added to it want a frame.
    pub fn server_log_shown(&self) -> bool {
        self.servers
            .log
            .is_some_and(|id| self.windows().any(|window| window.buffer() == id))
    }

    /// The buffer `*LSP*`, once something was logged there.
    pub(super) fn server_log_buffer(&self) -> Option<BufferId> {
        self.servers.log
    }

    /// Moves the selected window's primary cursor, alone, to the start of the next diagnostic
    /// after it, `forward`, or the previous before it, on a display cluster's start, as
    /// `C-M-n` and `C-M-p` do (Q15): hints are skipped, no mark is pushed, and past either end
    /// it wraps, which `Ok(true)` says. `Err` says why it did not move.
    pub fn to_diagnostic(&mut self, forward: bool) -> Result<bool, &'static str> {
        let (window, buffer) = self.current();
        if buffer.is_secret() {
            return Err("No server for a buffer that may hold secrets");
        }
        let set = buffer.diagnostics().ok_or("No diagnostics")?;
        let display = buffer.display();
        let offset = window.selections.primary().cursor;
        let (mut from, mut wrapped, mut target) = (offset, false, offset);
        // A start inside the cursor's own cluster clips back to it: the next one is looked for.
        for _ in 0..=set.len() {
            let (diagnostic, wraps) = set.step(from, forward).ok_or("No diagnostics")?;
            wrapped |= wraps;
            let start = diagnostic.range().start;
            target = crate::motion::cluster_start(&display, start);
            if target != offset || (wraps && from != offset) {
                break;
            }
            from = start;
        }
        let selections = &mut self.current_mut().0.selections;
        selections.keep_primary();
        selections.move_primary(target);
        Ok(wrapped)
    }

    /// What the echo area says of the diagnostics at the selected window's primary cursor, as
    /// Flymake does with ElDoc (Q14): the server's name, then the most severe one's first line,
    /// and `+2` when two more cover that place. Hints too (Q16). `None` where none is, and in a
    /// buffer that may hold secrets or that no server follows, which has none.
    pub fn diagnostic_at_point(&self) -> Option<String> {
        let (window, buffer) = self.current();
        let diagnostics = buffer.diagnostics()?;
        let offset = window.selections.primary().cursor;
        let mut shown = None;
        let mut covering = 0usize;
        for diagnostic in diagnostics.at(offset) {
            covering += 1;
            if shown.is_none_or(|(severity, _)| diagnostic.severity() < severity) {
                shown = Some((diagnostic.severity(), diagnostic.first_line()));
            }
        }
        let (_, line) = shown?;
        // Runs of blanks are one, so that no text the server pads out passes as oso's own.
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut text = format!("{}: {line}", diagnostics.server());
        if covering > 1 {
            text.push_str(&format!(" +{}", covering - 1));
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SERVER_LOG: usize = 1 << 20;

    #[test]
    fn the_message_at_point_names_the_server_and_the_most_severe_with_the_rest_counted() {
        use crate::diagnostic::{Diagnostics, New, Severity, Tags};
        let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", "let x = 1;\n"));
        let id = editor.window().buffer();
        editor.buffer_mut(id).unwrap().set_followed(true);
        let new = |range, severity, message| New {
            range,
            severity,
            tags: Tags::default(),
            source: "rustc",
            code: "",
            message,
        };
        let set = Diagnostics::new(
            "rust-analyzer",
            [
                new(0..5, Severity::Hint, "a hint"),
                new(4..6, Severity::Error, "mismatched types\nexpected u8"),
                new(4..9, Severity::Warning, "unused"),
                new(8..9, Severity::Hint, "only a hint"),
            ],
            11,
        );
        let buffer = editor.buffer_mut(id).unwrap();
        let revision = buffer.revision();
        assert!(buffer.take_diagnostics(revision, set));
        let at = |editor: &mut Editor, offset| {
            editor.current_mut().0.selections = crate::selection::SelectionSet::new(offset);
            editor.diagnostic_at_point()
        };
        assert_eq!(at(&mut editor, 2).as_deref(), Some("rust-analyzer: a hint"));
        assert_eq!(
            at(&mut editor, 4).as_deref(),
            Some("rust-analyzer: mismatched types +2")
        );
        assert_eq!(
            at(&mut editor, 8).as_deref(),
            Some("rust-analyzer: unused +1")
        );
        assert_eq!(at(&mut editor, 10), None);
        assert!(editor.buffer_mut(id).unwrap().treat_as_secret());
        assert_eq!(
            at(&mut editor, 4),
            None,
            "none in a buffer that may hold secrets"
        );
    }

    #[test]
    fn a_move_lands_on_a_cluster_s_start_and_passes_one_inside_the_cursor_s() {
        use crate::diagnostic::{Diagnostics, New, Severity, Tags};
        // `e` with a combining accent: a range from the accent starts inside the cluster.
        let text = "ae\u{301}xyz";
        let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
        let id = editor.window().buffer();
        let new = |range| New {
            range,
            severity: Severity::Error,
            tags: Tags::default(),
            source: "",
            code: "",
            message: "m",
        };
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.set_followed(true);
        let revision = buffer.revision();
        let set = Diagnostics::new("fake", [new(2..4), new(5..6)], text.len());
        assert!(buffer.take_diagnostics(revision, set));
        let cursor = |editor: &Editor| editor.window().selections.primary().cursor;
        assert_eq!(editor.to_diagnostic(true), Ok(false));
        assert_eq!(cursor(&editor), 1, "the cluster's start, not the accent's");
        assert_eq!(editor.to_diagnostic(true), Ok(false));
        assert_eq!(
            cursor(&editor),
            5,
            "past the one inside the cursor's cluster"
        );
        assert_eq!(editor.to_diagnostic(true), Ok(true));
        assert_eq!(cursor(&editor), 1, "wrapped");
    }

    #[test]
    fn requests_are_taken_oldest_first() {
        let mut editor = Editor::default();
        let id = editor.add_terminal_buffer("*t*");
        editor.request_server(ServerRequest::Start(id));
        editor.request_server(ServerRequest::ShutDown(id));
        assert_eq!(
            editor.take_server_requests(),
            [ServerRequest::Start(id), ServerRequest::ShutDown(id)]
        );
        assert!(editor.take_server_requests().is_empty());
    }

    #[test]
    fn the_log_keeps_its_last_bytes_in_whole_rows() {
        let mut editor = Editor::default();
        editor.server_log("ra p", ["first\nline"], MAX_SERVER_LOG);
        let id = editor
            .buffer_named(SERVER_LOG)
            .expect("a person goes to it");
        assert_eq!(
            editor.buffer(id).unwrap().text().to_string(),
            "[ra p] first\\nline\n"
        );
        assert!(editor.buffer(id).unwrap().withheld_from_agents());
        assert!(!editor.server_log_shown());
        let row = "x".repeat(1017);
        for _ in 0..(MAX_SERVER_LOG / 1024 + 5) {
            editor.server_log("ra p", [row.as_str()], MAX_SERVER_LOG);
        }
        let text = editor.buffer(id).unwrap().text().to_string();
        assert!(text.len() <= MAX_SERVER_LOG, "{}", text.len());
        assert!(text.len() > MAX_SERVER_LOG - 2048);
        assert!(text.starts_with("[ra p] x") && text.ends_with('\n'));
        assert!(text.lines().all(|line| line.len() == 1024), "whole rows");
        // A batch larger than the bound keeps its last whole rows, in one edit.
        let rows: Vec<String> = (0..20_000)
            .map(|n| format!("row {n:05} {}", "y".repeat(90)))
            .collect();
        editor.server_log("ra p", rows.iter().map(String::as_str), MAX_SERVER_LOG);
        let text = editor.buffer(id).unwrap().text().to_string();
        assert!(text.len() <= MAX_SERVER_LOG);
        assert!(text.starts_with("[ra p] row "), "{}", &text[..40]);
        assert!(text.ends_with(&format!("row 19999 {}\n", "y".repeat(90))));
        assert!(!text.contains('x'), "the old rows went");
    }
}
