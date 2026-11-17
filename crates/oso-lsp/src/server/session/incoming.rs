//! What a session thread does with each frame its server sends (ADR 0138, Bounds): the
//! flood count, the JSON's bounds, answers to oso's requests, answers to the server's, and its
//! notifications. An `Err` says why the message stops the server.
//!
//! Must not depend on: the process, or how the pipes' threads read; this reads what they
//! handed over, and gives the reader its room back.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use oso_core::buffer::Buffer;
use serde_json::{Value, json};

use crate::framing::Frame;
use crate::json::{self, JsonError};
use crate::limits::MESSAGE_BYTES_MAX;
use crate::position::Encoding;
use crate::protocol::{self, Incoming, Outcome, Published};
use crate::uri;

use super::super::Diagnosed;
use super::super::documents::Diagnose;
use super::super::pipes::FRAME_COST;
use super::run::Run;
use super::{Asked, Session};

impl Session {
    /// One frame from the server. `Err` says why it stops the server.
    pub(super) fn frame(&mut self, run: &mut Run, frame: Frame) -> Result<(), String> {
        let now = Instant::now();
        let bytes = match frame {
            Frame::Dropped { len } => {
                run.credit.release(FRAME_COST);
                run.flooded(now, len)?;
                self.reporter.log(format!(
                    "dropped a message of {len} bytes, over {MESSAGE_BYTES_MAX}"
                ));
                return Ok(());
            }
            Frame::Message(bytes) => bytes,
        };
        run.credit.release(FRAME_COST + bytes.len());
        run.flooded(now, bytes.len() as u64)?;
        let value = match json::parse(&bytes) {
            Ok(value) => value,
            Err(JsonError::TooManyValues) => {
                self.reporter.log(format!(
                    "dropped a message of {} bytes with too many values",
                    bytes.len()
                ));
                return Ok(());
            }
            Err(error) => return Err(format!("sent a message that is not JSON ({error})")),
        };
        let incoming =
            protocol::classify(value).map_err(|_| "sent a message that is not JSON-RPC")?;
        match incoming {
            Incoming::Answer { id, outcome } => self.answer(run, id, outcome, now)?,
            Incoming::Stray => self.reporter.log("an answer to no request".into()),
            Incoming::Request { id, method, params } => {
                let answer = protocol::answer_for(
                    &method,
                    &params,
                    self.start.section.as_deref(),
                    self.start.options.as_ref().unwrap_or(&Value::Null),
                );
                self.reporter.log(match &answer {
                    Ok(_) => format!("asked {method}, answered"),
                    Err((code, _)) => format!("asked {method}, answered with error {code}"),
                });
                run.send(&protocol::answer(&id, answer))?;
            }
            Incoming::Unanswerable { method } => self
                .reporter
                .log(format!("asked {method} with an id oso cannot answer")),
            Incoming::Notification { method, params } => {
                self.notification(run, &method, &params, now)
            }
        }
        Ok(())
    }

    /// An answer to request `id`.
    pub(super) fn answer(
        &mut self,
        run: &mut Run,
        id: i64,
        outcome: Outcome,
        now: Instant,
    ) -> Result<(), String> {
        let Some((asked, deadline)) = run.pending.remove(&id) else {
            self.reporter
                .log(format!("an answer to no request (id {id})"));
            return Ok(());
        };
        match (asked, outcome) {
            (Asked::Initialize, Outcome::Result(result)) => {
                let encoding = protocol::chosen_encoding(&result)
                    .ok_or("chose a position encoding oso did not offer")?;
                run.send(&protocol::notification("initialized", json!({})))?;
                run.ready = true;
                run.encoding = encoding;
                run.sync = protocol::sync(&result);
                let completing = protocol::completing(&result);
                run.resolves = completing.resolves;
                run.acting = protocol::acting(&result);
                self.reporter.completing(completing);
                self.open_documents(run)?;
                let name = protocol::server_name(&result)
                    .map(|name| format!(" as {name}"))
                    .unwrap_or_default();
                self.reporter.log(format!(
                    "initialized{name} in {} ms, positions in {}",
                    now.duration_since(run.started).as_millis(),
                    match encoding {
                        Encoding::Utf8 => "UTF-8",
                        Encoding::Utf16 => "UTF-16",
                    }
                ));
                self.reporter.state(run.state());
            }
            (Asked::Initialize, Outcome::Error { code, message }) => {
                return Err(format!("refused initialize: {message} ({code})"));
            }
            (Asked::Shutdown, _) => run.exit(now),
            (Asked::Ask(ask), outcome) => self.answered(run, (id, ask), outcome, deadline, now),
        }
        Ok(())
    }

    /// A notification from the server.
    pub(super) fn notification(
        &mut self,
        run: &mut Run,
        method: &str,
        params: &Value,
        now: Instant,
    ) {
        match method {
            "$/progress" => {
                if let Some((token, progress)) = protocol::progress(params) {
                    run.progress(token, progress);
                    self.reporter.state(run.state());
                }
            }
            "window/showMessage" => {
                if let Some((severity, text)) = protocol::message(params) {
                    self.reporter.log(format!("message: {text}"));
                    self.reporter.show(severity, text.to_owned(), true, now);
                }
            }
            "window/logMessage" => {
                if let Some((_, text)) = protocol::message(params) {
                    self.reporter.log(format!("log: {text}"));
                }
            }
            "textDocument/publishDiagnostics" => self.published(run, params),
            _ => self.reporter.log(format!("told {method}")),
        }
    }

    /// A diagnostics report (step 6). One for a file outside the project, or whose name may
    /// hold secrets, as a compiler's message can quote the file, is dropped and only counted;
    /// the rest is kept, and goes to the UI: an open document's as its set, any other's as it
    /// is kept (6b), with those the store's bounds let go.
    fn published(&mut self, run: &mut Run, params: &Value) {
        let Some(report) = protocol::published(params) else {
            self.reporter
                .log("a diagnostics report that does not read".into());
            return;
        };
        // The root as a URI reads back, so that both are in one form: on Windows, the drive
        // letter upper case and no `\\?\` prefix.
        let root = uri::from_path(&self.start.root).and_then(|root| uri::to_path(&root));
        let judged = uri::to_path(&report.uri).filter(|path| {
            root.as_ref().is_some_and(|root| path.starts_with(root))
                && !Buffer::name_may_hold_secrets(path)
        });
        let Some(path) = judged else {
            run.diagnostics_dropped += 1;
            return;
        };
        let open = self.deliver_diagnostics(&path, &report, run.encoding);
        // The paths open, gathered once, and only if the bounds are reached.
        let documents = &self.documents;
        let opened = std::cell::OnceCell::new();
        let gone = self.diagnosed.put(path.clone(), report, |path| {
            opened.get_or_init(|| documents.paths()).contains(path)
        });
        if !open {
            self.deliver_file(path, run.encoding);
        }
        for path in gone {
            self.forget_file(path);
        }
    }

    /// Hands the UI the report kept for the file at `path`, or an empty one if none is, for a
    /// file no document open with the server holds.
    pub(super) fn deliver_file(&mut self, path: PathBuf, encoding: Encoding) {
        match self.diagnosed.get(&path) {
            Some(report) => self.reporter.diagnostics(Diagnosed::File {
                path,
                report,
                encoding,
            }),
            None => self.forget_file(path),
        }
    }

    /// Tells the UI to forget the report of the file at `path`, whose diagnostics the store let
    /// go or a document holds now: an empty one, whose encoding counts for nothing.
    pub(super) fn forget_file(&mut self, path: PathBuf) {
        self.reporter.diagnostics(Diagnosed::File {
            path,
            report: Arc::new(Published {
                uri: String::new(),
                version: None,
                diagnostics: Vec::new(),
            }),
            encoding: Encoding::Utf16,
        });
    }

    /// Hands the UI the set `report` gives the document open at `path`, if one is, and says
    /// whether one is.
    pub(super) fn deliver_diagnostics(
        &mut self,
        path: &std::path::Path,
        report: &Published,
        encoding: Encoding,
    ) -> bool {
        match self
            .documents
            .diagnose(path, report, &self.start.name, encoding)
        {
            Diagnose::For {
                id,
                revision,
                diagnostics,
            } => self.reporter.diagnostics(Diagnosed::Open {
                id,
                revision,
                diagnostics,
            }),
            Diagnose::NotOpen => return false,
            Diagnose::Stale => self.reporter.log(format!(
                "a diagnostics report for version {} is past: a newer one follows",
                report.version.unwrap_or_default()
            )),
        }
        true
    }

    pub(super) fn dropped_diagnostics(&mut self, run: &mut Run) {
        let count = std::mem::take(&mut run.diagnostics_dropped);
        self.reporter.log(format!(
            "dropped {count} diagnostics reports for files outside the project or whose names \
             may hold secrets"
        ));
    }
}
