//! `oso mcp`'s stdin (ADR 0085): a thread of its own reads the agent's request lines, so that
//! a cancel, or the agent going, closes the call that waits on the channel.
//!
//! Must not depend on: the channel's socket; a call's connection is closed through what
//! [`Interrupt`] holds.
//!
//! - At most [`QUEUED`] lines wait for the bridge. One more is answered with an error, so that
//!   the agent cannot pile up lines while a call waits, and the thread still reads on: a cancel
//!   or the end of stdin is never stuck behind them.
//! - A cancel is handled here and goes no further. It closes the call it names if that call is
//!   in flight, and a request it names that still waits is dropped unanswered.
//! - A line longer than [`MAX_LINE`] is skipped to its newline, and answered with an error.

use std::collections::VecDeque;
use std::io::{self, BufRead};
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use serde_json::Value;

use super::super::wire::{MAX_ID, MAX_LINE, read_bounded_line};

/// How many request lines may wait for the bridge, each up to [`MAX_LINE`].
pub(super) const QUEUED: usize = 8;

/// How many requests dropped for want of room are answered at most, when the bridge is free.
const MAX_DROPPED: usize = 64;

/// What the stdin thread shares with the bridge. One lock covers it all, so that a call cannot
/// start waiting just after the agent went or cancelled it.
#[derive(Default)]
pub(super) struct Interrupt(Mutex<Calls>);

#[derive(Default)]
pub(super) struct Calls {
    /// The agent's stdin ended: it has gone, and no call is made for it any more.
    pub gone: bool,
    /// The id of the tool call that waits on the connection.
    pub in_flight: Option<Value>,
    /// The agent cancelled the call in flight: its connection is closed, and made again.
    pub cut: bool,
    /// What closes the connection, so that a call waiting on it ends.
    pub close: Option<Box<dyn Fn() + Send>>,
    /// Ids the agent cancelled that were not in flight, the latest [`QUEUED`].
    cancelled: VecDeque<Value>,
    /// Ids of requests dropped since [`QUEUED`] lines waited already.
    dropped: Vec<Value>,
}

impl Interrupt {
    pub(super) fn lock(&self) -> MutexGuard<'_, Calls> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The agent cancelled the request `id`: the call in flight is closed if it is that one,
    /// and otherwise the request is dropped when its turn comes.
    pub(super) fn cancel(&self, id: Value) {
        if !is_id(&id) {
            return;
        }
        let mut calls = self.lock();
        if calls.in_flight.as_ref() == Some(&id) {
            calls.cut = true;
            if let Some(close) = &calls.close {
                close();
            }
        } else {
            if calls.cancelled.len() == QUEUED {
                calls.cancelled.pop_front();
            }
            calls.cancelled.push_back(id);
        }
    }

    /// Whether the agent cancelled `id` before it ran; it is forgotten then.
    pub(super) fn take_cancelled(calls: &mut Calls, id: &Value) -> bool {
        let at = calls.cancelled.iter().position(|cancelled| cancelled == id);
        at.is_some_and(|at| calls.cancelled.remove(at).is_some())
    }

    /// The agent has gone: a call that waits ends, and none starts.
    pub(super) fn end(&self) {
        let mut calls = self.lock();
        calls.gone = true;
        if calls.in_flight.is_some()
            && let Some(close) = &calls.close
        {
            close();
        }
    }

    /// The ids of requests dropped for want of room, to answer now.
    pub(super) fn take_dropped(&self) -> Vec<Value> {
        std::mem::take(&mut self.lock().dropped)
    }

    fn drop_request(&self, id: Value) {
        let mut calls = self.lock();
        if calls.dropped.len() < MAX_DROPPED {
            calls.dropped.push(id);
        }
    }
}

/// Whether `id` is one a request may have: a string of at most [`MAX_ID`] bytes, or a number.
pub(super) fn is_id(id: &Value) -> bool {
    match id {
        Value::String(id) => id.len() <= MAX_ID,
        Value::Number(_) => true,
        _ => false,
    }
}

/// What the stdin thread reads.
pub(super) enum Input {
    Line(String),
    TooLong,
    End,
}

/// The id a cancel names if `line` is one, or [`Value::Null`] for a cancel that names none.
/// Only a line that mentions a cancel is parsed here.
fn cancel_of(line: &str) -> Option<Value> {
    if !line.contains("notifications/cancelled") {
        return None;
    }
    let message: Value = serde_json::from_str(line).ok()?;
    (message["method"] == "notifications/cancelled" && message.get("id").is_none())
        .then(|| message["params"]["requestId"].clone())
}

/// The id of the request `line`, if it has one a request may have.
fn id_of(line: &str) -> Option<Value> {
    let message: Value = serde_json::from_str(line).ok()?;
    let id = message.get("id")?;
    is_id(id).then(|| id.clone())
}

/// Reads up to the next newline and drops it. Returns false if `input` ended first.
fn skip_line(input: &mut dyn BufRead) -> bool {
    loop {
        let buf = match input.fill_buf() {
            Ok([]) | Err(_) => return false,
            Ok(buf) => buf,
        };
        if let Some(at) = buf.iter().position(|&b| b == b'\n') {
            input.consume(at + 1);
            return true;
        }
        let read = buf.len();
        input.consume(read);
    }
}

/// Reads `input` on a thread of its own, line by line. When the agent cancels the call that
/// waits, or, if `end_is_gone`, when `input` ends, it closes that call's connection: the
/// agent's stdin ends when it goes, where a file of requests ends at once.
pub(super) fn spawn_reader(
    mut input: Box<dyn BufRead + Send>,
    interrupt: Arc<Interrupt>,
    end_is_gone: bool,
) -> io::Result<mpsc::Receiver<Input>> {
    let (lines, received) = mpsc::sync_channel(QUEUED);
    thread::Builder::new()
        .name("oso-mcp-stdin".into())
        .spawn(move || {
            let mut line = Vec::new();
            loop {
                line.clear();
                let read = match read_bounded_line(&mut input, &mut line) {
                    Ok(0) | Err(_) => Input::End,
                    Ok(_) if line.len() > MAX_LINE && line.last() != Some(&b'\n') => {
                        if !skip_line(&mut input) {
                            // The line ran to the end of stdin: it is answered, and then the end.
                            let _ = lines.try_send(Input::TooLong);
                            Input::End
                        } else {
                            Input::TooLong
                        }
                    }
                    Ok(_) => Input::Line(String::from_utf8_lossy(&line).trim().to_owned()),
                };
                if let Input::Line(text) = &read
                    && let Some(id) = cancel_of(text)
                {
                    interrupt.cancel(id);
                    continue;
                }
                if matches!(read, Input::End) {
                    if end_is_gone {
                        interrupt.end();
                    }
                    let _ = lines.send(Input::End);
                    return;
                }
                match lines.try_send(read) {
                    Ok(()) => {}
                    Err(TrySendError::Full(Input::Line(text))) => {
                        if let Some(id) = id_of(&text) {
                            interrupt.drop_request(id);
                        }
                    }
                    // An over-long line that finds no room goes unanswered: it has no id.
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        })?;
    Ok(received)
}
