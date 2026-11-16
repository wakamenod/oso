//! What a session thread does with each frame its server sends (ADR 0138, Bounds): the
//! flood count, the JSON's bounds, answers to oso's requests, answers to the server's, and its
//! notifications. An `Err` says why the message stops the server.
//!
//! Must not depend on: the process, or how the pipes' threads read; this reads what they
//! handed over, and gives the reader its room back.

use std::time::Instant;

use serde_json::{Value, json};

use crate::framing::Frame;
use crate::json::{self, JsonError};
use crate::limits::MESSAGE_BYTES_MAX;
use crate::position::Encoding;
use crate::protocol::{self, Incoming, Outcome};

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
        let Some((asked, _)) = run.pending.remove(&id) else {
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
            // Diagnostics come with step 6; until then they are counted and dropped.
            "textDocument/publishDiagnostics" => run.diagnostics_dropped += 1,
            _ => self.reporter.log(format!("told {method}")),
        }
    }

    pub(super) fn dropped_diagnostics(&mut self, run: &mut Run) {
        let count = std::mem::take(&mut run.diagnostics_dropped);
        self.reporter.log(format!(
            "dropped {count} diagnostics reports: oso does not show them yet"
        ));
    }
}
