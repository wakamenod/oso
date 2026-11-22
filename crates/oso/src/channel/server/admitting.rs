//! Letting a connection in (ADR 0085, ADR 0121): a hosted session's bridge by its claim, any
//! other by the person's answer, both through the UI thread.
//!
//! Must not depend on: editor state; it only waits on channels.

use std::sync::mpsc;
use std::time::Instant;

use oso_platform::control::{ControlStream, Peer};
use serde_json::Value;

use super::held::{self, Waited};
use super::limits::Waiting;
use super::{Connection, Event, Line, REFUSED_BY_THE_PERSON, REPLY_TIMEOUT, Request, Shared};

/// Whether the UI thread finds the connection of `peer` to be the bridge of the hosted session
/// `session` it claims, within [`REPLY_TIMEOUT`]. If so, the UI counts it as let in.
pub(super) fn claimed<S>(peer: &Peer, session: String, connection: Connection, send: &S) -> bool
where
    S: Fn(Request) -> bool,
{
    let (reply, replied) = mpsc::channel();
    let claims = Request {
        connection,
        event: Event::Claims {
            peer: peer.clone(),
            session,
            parent_started: parent_started(peer),
        },
        reply: Some(reply),
    };
    send(claims)
        && matches!(
            replied.recv_timeout(REPLY_TIMEOUT),
            Ok(Ok(Value::Bool(true)))
        )
}

/// When `peer`'s parent started, as the system says, if it does.
fn parent_started(peer: &Peer) -> Option<std::time::SystemTime> {
    let parent = peer.parent.as_ref()?;
    oso_platform::process::start_time(parent.pid())
        .ok()
        .flatten()
}

/// How asking the person ended, when it did not let the connection in.
pub(super) enum Asked {
    Refused(String),
    Gone,
}

/// Asks the person, through the UI thread, whether this connection may drive the editor, if
/// it may take the turn to ask now, and waits as long as they take. However it ends, it gives
/// the turn back first, holding new connections off only after the person's `n`; then, if the
/// client went or the channel stopped, it tells the UI, so that the question closes.
pub(super) fn ask<S>(
    peer: &Peer,
    connection: Connection,
    (lines, writer, shared): (&mpsc::Receiver<Line>, &mut ControlStream, &Shared),
    send: &S,
) -> Result<(), Asked>
where
    S: Fn(Request) -> bool,
{
    let waiting =
        Waiting::take(shared, Instant::now()).map_err(|why| Asked::Refused(why.into()))?;
    let (reply, replied) = mpsc::channel();
    let asks = Request {
        connection,
        event: Event::Asks {
            peer: peer.clone(),
            parent_started: parent_started(peer),
        },
        reply: Some(reply),
    };
    if !send(asks) {
        waiting.answered(false);
        return Err(Asked::Refused("the editor is shutting down".into()));
    }
    let waited = held::watching(&replied, None, held::PERSON, (lines, writer, shared));
    // The turn is given back before the UI is told the client went, so that an agent that
    // connects once the question closed is not told another one waits. Only the person's own
    // `n` holds others off: a client that goes, or a channel that stops, is no refusal.
    let refused = matches!(&waited, Waited::Replied(Ok(Err(why))) if why == REFUSED_BY_THE_PERSON);
    waiting.answered(refused);
    match waited {
        Waited::Replied(Ok(Ok(_))) => Ok(()),
        Waited::Replied(Ok(Err(why))) => Err(Asked::Refused(why)),
        Waited::Replied(Err(why)) => Err(Asked::Refused(why.into())),
        Waited::Gone => {
            send(Request {
                connection,
                event: Event::Gone,
                reply: None,
            });
            Err(Asked::Gone)
        }
    }
}
