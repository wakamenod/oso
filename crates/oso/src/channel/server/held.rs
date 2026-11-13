//! Waiting for the UI's reply to one request, as long as its method holds it (ADR 0121): a
//! usual request for [`REPLY_TIMEOUT`], a worktree job for as long as its bounded calls take,
//! and a session tool that asks the person for as long as they take, as the connection's own
//! question waits. While it waits for the person, the client going or the channel stopping
//! ends the wait, so that the UI closes the question and runs nothing the agent gave up on.
//!
//! Must not depend on: editor state; it only waits on channels.

use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use oso_platform::control::ControlStream;

use super::super::protocol::{self, sessions::Hold};
use super::super::wire::failure;
use super::{Line, MAX_WAITING_LINES, POLL, REPLY_TIMEOUT, Reply, Shared};

/// How a wait ended.
pub(super) enum Waited {
    /// The UI replied, or the wait ended with this message.
    Replied(Result<Reply, &'static str>),
    /// The client went, or the channel stopped, while the person had not answered.
    Gone,
}

/// Waits for the reply on `replied` as `hold` says. Only [`Hold::Person`] reads `lines`
/// meanwhile: each line the client sends is answered "wait", up to [`MAX_WAITING_LINES`].
pub(super) fn wait(
    replied: &mpsc::Receiver<Reply>,
    hold: Hold,
    lines: &mpsc::Receiver<Line>,
    writer: &mut ControlStream,
    shared: &Shared,
) -> Waited {
    let within = match hold {
        Hold::Usual => REPLY_TIMEOUT,
        Hold::Job(within) => within,
        Hold::Person => return for_the_person(replied, lines, writer, shared),
    };
    Waited::Replied(match replied.recv_timeout(within) {
        Ok(reply) => Ok(reply),
        Err(mpsc::RecvTimeoutError::Timeout) => Err("the editor did not reply in time"),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("the editor dropped the request"),
    })
}

fn for_the_person(
    replied: &mpsc::Receiver<Reply>,
    lines: &mpsc::Receiver<Line>,
    writer: &mut ControlStream,
    shared: &Shared,
) -> Waited {
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
        match lines.try_recv() {
            Ok(Line::Text(_)) if extra >= MAX_WAITING_LINES => return Waited::Gone,
            Ok(Line::Text(text)) => {
                extra += 1;
                let id = protocol::parse(&text).map_or_else(|bad| bad.id, |request| request.id);
                let waiting = failure(&id, "wait: the person has not answered yet");
                if writeln!(writer, "{waiting}").is_err() {
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
