//! What the session thread does with the UI's asks about documents (ADR 0138, Document sync,
//! step 4): with no server ready they are only kept; with one, the notifications they come to
//! are sent as the server's sync asks, and a document kept from it is reported.
//!
//! Must not depend on: the pipes' threads or the process; this keeps documents and sends
//! through the run's writer.

use crate::limits::{DOCUMENT_BYTES_MAX, WRITER_QUEUE_BYTES_MAX};
use crate::protocol::{self, SyncKind};

use super::super::documents::Out;
use super::super::{Change, Document};
use super::Session;
use super::run::Run;

/// An ask about a document.
pub(super) enum Ask {
    Open(Box<Document>),
    Change(Box<Change>),
    Close(u64),
    Save(u64),
}

impl Ask {
    /// The ask `event` is, if it is one.
    pub(super) fn of(event: super::Event) -> Option<Self> {
        Some(match event {
            super::Event::Open(document) => Self::Open(document),
            super::Event::Change(change) => Self::Change(change),
            super::Event::Close(id) => Self::Close(id),
            super::Event::Save(id) => Self::Save(id),
            _ => return None,
        })
    }
}

impl Session {
    /// Carries out `ask`, against `run` if a server runs. `Err` says why it stops the server.
    pub(super) fn document(&mut self, run: Option<&mut Run>, ask: Ask) -> Result<(), String> {
        let live = run
            .as_ref()
            .is_some_and(|run| run.ready && run.stopping.is_none() && run.sync.open_close);
        let (id, out) = match ask {
            Ask::Open(document) => (document.id, self.documents.open(*document, live)),
            Ask::Change(change) => {
                let incremental = run
                    .as_ref()
                    .is_none_or(|run| run.sync.change == SyncKind::Incremental);
                let splices = change.splices.as_deref();
                let encoding = run
                    .as_ref()
                    .map_or(crate::position::Encoding::Utf16, |run| run.encoding);
                let out = self.documents.change(
                    change.id,
                    change.revision,
                    &change.before,
                    change.after,
                    splices,
                    encoding,
                    incremental,
                );
                (change.id, out)
            }
            Ask::Close(id) => (id, self.documents.close(id)),
            Ask::Save(id) => (id, self.documents.save(id, live)),
        };
        match run.filter(|_| live) {
            Some(run) => {
                self.deliver(Some(run), id, out)?;
                self.send_waiting(run)
            }
            None => self.deliver(None, id, out),
        }
    }

    /// The server is ready: every document goes to it, as room comes.
    pub(super) fn open_documents(&mut self, run: &mut Run) -> Result<(), String> {
        if !run.sync.open_close {
            return Ok(());
        }
        self.documents.open_all();
        self.send_waiting(run)
    }

    /// Sends the whole texts that wait, one at a time while the writer's queue is under half
    /// its bound, so that opening many large documents, or a server that wants the whole text
    /// at each change, never fills it: a whole text that waits is the latest when it goes.
    pub(super) fn send_waiting(&mut self, run: &mut Run) -> Result<(), String> {
        if !(run.ready && run.stopping.is_none() && run.sync.open_close) {
            return Ok(());
        }
        while run.queued() < WRITER_QUEUE_BYTES_MAX / 2 {
            let Some(send) = self.documents.next_whole() else {
                break;
            };
            self.deliver(Some(run), 0, Out { send, state: None })?;
        }
        // A report kept from before a document opened, which names no version, as a check's
        // on save does: placed against the text it opened with.
        for path in self.documents.take_opened() {
            if let Some(report) = self.diagnosed.get(&path).filter(|r| r.version.is_none()) {
                self.deliver_diagnostics(&path, &report, run.encoding);
            }
        }
        Ok(())
    }

    /// Sends what `out` holds through `run`, as the server's sync asks, and reports a change of
    /// document `id`'s state.
    fn deliver(&mut self, run: Option<&mut Run>, id: u64, out: Out) -> Result<(), String> {
        if let Some(closed) = out.state {
            if let Some(reason) = closed {
                let why = match reason {
                    super::super::Closed::LoneCarriageReturn => {
                        "it holds a carriage return that ends no line".to_owned()
                    }
                    super::super::Closed::TooLarge => {
                        format!("it is larger than {} MiB", DOCUMENT_BYTES_MAX >> 20)
                    }
                };
                self.reporter.log(format!("a document is not sent: {why}"));
            }
            self.reporter.document(id, closed);
        }
        let Some(run) = run else {
            return Ok(());
        };
        for (method, params) in out.send {
            let wanted = match method {
                "textDocument/didChange" => run.sync.change != SyncKind::None,
                "textDocument/didSave" => run.sync.save,
                _ => true,
            };
            if wanted {
                run.send(&protocol::notification(method, params))?;
            }
        }
        Ok(())
    }
}
