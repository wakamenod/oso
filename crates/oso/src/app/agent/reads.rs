//! The agent channel's reads (ADR 0085): `state`, with each buffer's language server named
//! (ADR 0138, step 11), and `text` and `diagnostics`, each checked and snapshotted here and
//! answered on a thread of its own.
//!
//! Must not depend on: sockets or threads, which stay behind the channel's requests; the
//! servers are read through the servers' accessors. The judge of files it hands the job
//! touches the file system, on the job's thread only.

use std::path::Path;
use std::sync::mpsc::Sender;

use super::super::App;
use crate::channel::protocol::{DiagnosticsOf, TextOf};
use crate::channel::server::Reply;

impl App {
    /// The channel's `text`: checked here, and the rows copied on a thread of its own.
    pub(in crate::app) fn agent_text(&mut self, of: TextOf, reply: Sender<Reply>) {
        self.agent_log_often("text");
        let screen = |id, ask| self.terminal_screen(id, ask);
        match crate::channel::text::start(&self.editor, &screen, of) {
            Ok(job) => job.spawn(reply),
            Err(why) => {
                let _ = reply.send(Err(why));
            }
        }
    }

    /// The agent channel's `state`, with each buffer's language server named.
    pub(in crate::app) fn agent_state(&mut self, buffer_offset: usize) -> Reply {
        self.agent_log_often("state");
        Ok(crate::channel::state::state(
            &self.editor,
            self.dispatcher.menu_name(),
            buffer_offset,
            (&|id| self.server_name(id), self.xrefs_server()),
        ))
    }

    /// The agent channel's `diagnostics`: checked here and taken from the editor and the
    /// project's servers' reports of files no buffer holds, all shared, and answered on a
    /// thread of its own.
    pub(in crate::app) fn agent_diagnostics(&mut self, of: DiagnosticsOf, reply: Sender<Reply>) {
        self.agent_log_often("diagnostics");
        let files = |root: &Path| self.file_reports(root);
        match crate::channel::diagnostics::start(
            &self.editor,
            &files,
            crate::loader::file_may_hold_secrets,
            of,
        ) {
            Ok(job) => job.spawn(reply),
            Err(why) => {
                let _ = reply.send(Err(why));
            }
        }
    }
}
