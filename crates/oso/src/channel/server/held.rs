//! Waiting for the UI's reply to one request, as long as its method holds it (ADR 0121): a
//! usual request for [`REPLY_TIMEOUT`], a worktree job for as long as the app gives it, and a
//! session tool that asks the person, or the connection's own question, for as long as the
//! person takes. Every wait but the usual one watches the client and the channel meanwhile:
//! the client going or the channel stopping ends it, so that the UI closes the question and
//! runs nothing the agent gave up on.
//!
//! Must not depend on: editor state; it only waits on channels.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use oso_platform::control::ControlStream;

use super::super::protocol::{self, sessions::Hold};
use super::super::wire::failure;
use super::{Line, MAX_WAITING_LINES, POLL, REPLY_TIMEOUT, Reply, Shared};

/// How a wait ended.
pub(super) enum Waited {
    /// The UI replied, or the wait ended with this message.
    Replied(Result<Reply, &'static str>),
    /// The client went, or the channel stopped.
    Gone,
}

/// What the client is told of a line it sends while the person has not answered.
pub(super) const PERSON: &str = "wait: the person has not answered yet";
/// What it is told of one it sends while a job runs.
const JOB: &str = "wait: the last call has not finished";

/// Waits for the reply on `replied` as `hold` says.
pub(super) fn wait(
    replied: &mpsc::Receiver<Reply>,
    hold: Hold,
    lines: &mpsc::Receiver<Line>,
    writer: &mut ControlStream,
    shared: &Shared,
) -> Waited {
    match hold {
        Hold::Usual => Waited::Replied(match replied.recv_timeout(REPLY_TIMEOUT) {
            Ok(reply) => Ok(reply),
            Err(mpsc::RecvTimeoutError::Timeout) => Err("the editor did not reply in time"),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("the editor dropped the request"),
        }),
        Hold::Job(within) => watching(replied, Some(within), JOB, (lines, writer, shared)),
        Hold::Person => watching(replied, None, PERSON, (lines, writer, shared)),
    }
}

/// Waits for the reply on `replied`, at most `within` if given, while watching the client and
/// the channel. Each line the client sends meanwhile is answered `busy`, up to
/// [`MAX_WAITING_LINES`]; more ends the wait.
pub(super) fn watching(
    replied: &mpsc::Receiver<Reply>,
    within: Option<Duration>,
    busy: &str,
    (lines, writer, shared): (&mpsc::Receiver<Line>, &mut ControlStream, &Shared),
) -> Waited {
    let deadline = within.map(|within| Instant::now() + within);
    let mut extra = 0;
    loop {
        match replied.recv_timeout(POLL) {
            Ok(reply) => return Waited::Replied(Ok(reply)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Waited::Replied(Err("the editor dropped the request"));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if shared.stop.load(Ordering::Acquire) {
            return Waited::Gone;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Waited::Replied(Err("the editor did not reply in time"));
        }
        match lines.try_recv() {
            Ok(Line::Text(_)) if extra >= MAX_WAITING_LINES => return Waited::Gone,
            Ok(Line::Text(text)) => {
                extra += 1;
                let id = protocol::parse(&text).map_or_else(|bad| bad.id, |request| request.id);
                if writeln!(writer, "{}", failure(&id, busy)).is_err() {
                    return Waited::Gone;
                }
            }
            Ok(Line::TooLong | Line::End) | Err(mpsc::TryRecvError::Disconnected) => {
                return Waited::Gone;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}
