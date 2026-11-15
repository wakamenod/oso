//! What commands ask of the host about the language server of the selected buffer
//! (ADR 0138, step 3): `lsp-start`, `lsp-restart` and `lsp-shutdown`; and `*LSP*`, the log of
//! what servers did, which keeps the last bytes the host says.
//!
//! Must not depend on: the host's servers, threads or I/O; a command asks, and the host takes
//! the request after the command and acts on the server.

use crate::buffer::{Buffer, BufferId};

use super::Editor;
use super::messages::Keep;

/// The name of the servers' log.
pub(crate) const SERVER_LOG: &str = "*LSP*";

/// What commands asked of the host, and the log's buffer once made.
#[derive(Debug, Default)]
pub(super) struct Servers {
    requests: Vec<ServerRequest>,
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
}

impl Editor {
    /// A command asks the host for `request`.
    pub fn request_server(&mut self, request: ServerRequest) {
        self.servers.requests.push(request);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SERVER_LOG: usize = 1 << 20;

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
