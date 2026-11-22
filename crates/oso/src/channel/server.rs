//! The agent channel's server (ADR 0085): background threads that take connections, hold
//! each one until the person lets it in, and then hand its requests to the UI thread.
//!
//! Must not depend on: `app`, winit or editor state, which only the UI thread touches.
//!
//! - A connection that sends no request within [`FIRST_REQUEST`] is closed, unasked and
//!   uncounted: probes, such as the stale-socket cleanup, connect and send nothing.
//! - Its first request asks the person, through the UI thread, and is answered only once
//!   they agree. One connection waits at a time, at most [`MAX_ASKS`] ask a minute, and none
//!   for [`REFUSED_FOR`] after the person says no. The next may ask by the time the UI hears
//!   that a waiting client went.
//! - Every connection counts toward [`MAX_ACCEPTS`] a minute, and at most [`MAX_CLIENTS`]
//!   are open at once, so no process can hold the slots or churn connections unseen.
//! - A first request that is `version` naming a session asks the UI thread first whether the
//!   connection is that hosted session's bridge (ADR 0121). One is let in without the
//!   question and outside [`MAX_CLIENTS`]: each session holds at most one, which the UI
//!   keeps to. Anything else goes on as above.
//! - Once let in, a connection sends at most [`MAX_RATE`] requests a second, one at a time,
//!   with no idle limit.
//! - A reply waits for the UI for [`REPLY_TIMEOUT`], except where its method holds it longer
//!   (`held`): a worktree job's for as long as the job is given, and a session tool's that
//!   asks, as the connection's question, until the person answers or the client goes.
//! - The same loop serves the sessions' socket ([`Policy::Sessions`], ADR 0165, step 2): it
//!   lets in only a connection whose first request claims a hosted session and that the UI
//!   finds to be that session's bridge, never asks the person, and answers only `version` and
//!   `permission`, which the agent channel refuses in turn.

use std::io::{self, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlListener, ControlStream, Peer};
use serde_json::{Value, json};

use super::protocol::{self, Method, Op, PROTOCOL_VERSION, sessions::Hold};
use super::wire::{MAX_LINE, bounded, failure, read_line_within, success};

/// The most connections open at once that have sent a request.
pub const MAX_CLIENTS: usize = 2;

/// The most connections open at once that have sent nothing yet. They have slots of their
/// own, so that connections that never speak cannot keep an agent out.
pub const MAX_SILENT: usize = 4;

/// How often closing silent connections is noted for the person, at most.
const SILENT_NOTED_EVERY: Duration = Duration::from_secs(60);

/// How long a new connection has to send its first request.
pub const FIRST_REQUEST: Duration = Duration::from_secs(2);

/// The most connections accepted a minute, whether they ask or not.
pub const MAX_ACCEPTS: usize = 60;

/// The most connections that ask the person a minute.
pub const MAX_ASKS: usize = 10;

/// How long new connections are refused after the person says no.
pub const REFUSED_FOR: Duration = Duration::from_secs(60);

/// The most requests a second from one connection.
pub const MAX_RATE: f64 = 30.0;

/// How long a request waits for the UI to reply. The person's answer to a connection is not
/// bound by it.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the UI holds back a reply that waits for work a command started, a save or an
/// undo, before it says the work goes on: well within [`REPLY_TIMEOUT`].
pub const ANSWER_WITHIN: Duration = REPLY_TIMEOUT.saturating_sub(Duration::from_secs(2));

/// How long writing a response may block on a client that does not read.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a waiting thread looks whether the channel stopped or its client went.
const POLL: Duration = Duration::from_millis(200);

/// How the accept loop pauses after an unexpected error, so that a persistent one cannot spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// One connection, for the UI thread to tell them apart: its number in the channel's
/// `run`, which the app counts up each time it starts the channel, so that nothing of a
/// stopped run passes for a connection of the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Connection {
    pub run: u64,
    pub number: u64,
}

/// What the UI replies when the person says no to a connection, the one refusal that holds
/// off new connections for [`REFUSED_FOR`].
pub const REFUSED_BY_THE_PERSON: &str = "the person did not let this agent in";

/// How many more lines a waiting connection may send before it is closed.
const MAX_WAITING_LINES: usize = 3;

/// The UI's answer: a JSON result or an error message.
pub type Reply = Result<Value, String>;

/// What a connection brings the UI thread.
#[derive(Debug)]
pub enum Event {
    /// Its first request came: ask the person whether this process may drive the editor. An
    /// `Ok` reply lets it in. When the peer's parent started is read here, off the UI thread:
    /// an outside session that asks to move is known by it (ADR 0151).
    Asks {
        peer: Peer,
        parent_started: Option<std::time::SystemTime>,
    },
    /// Its first request names a hosted session, as the bridge gave it: is this process that
    /// session's bridge (ADR 0121)? An `Ok(true)` reply lets it in without asking, as that
    /// session's; anything else and it asks as any connection.
    Claims {
        peer: Peer,
        session: String,
        /// When the peer's parent started, as the system says, read here, off the UI
        /// thread: a carried session's `claude` must match it (ADR 0120, step 11).
        parent_started: Option<std::time::SystemTime>,
    },
    /// A request from a connection the person let in.
    Op(Op),
    /// It closed, or the channel stopped.
    Gone,
    /// Connections were closed for sending nothing, at most once a minute, for `*Agent*`.
    Silent,
}

/// A connection's event, with the channel for the reply, if it wants one.
#[derive(Debug)]
pub struct Request {
    pub connection: Connection,
    pub event: Event,
    pub reply: Option<mpsc::Sender<Reply>>,
}

/// Which socket a server serves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Policy {
    /// The agent channel's: the person lets each connection in, and every method but
    /// `permission` is answered.
    #[default]
    Channel,
    /// The sessions': only a hosted session's bridge, and only `version` and `permission`.
    Sessions,
}

/// What the agent channel answers `permission` with.
const NOT_ON_THE_CHANNEL: &str = "permission is asked on the sessions' socket, not the channel";

/// What the sessions' socket answers anything but its own methods with.
const ONLY_PERMISSION: &str = "this socket answers only a native session's permission";

/// What the sessions' socket answers a connection that is no hosted session's bridge with.
const ONLY_BRIDGES: &str = "this socket takes only the bridges of the sessions oso hosts";

/// What the server's threads share.
#[derive(Debug, Default)]
pub struct Shared {
    /// The channel stopped: every thread ends.
    pub stop: AtomicBool,
    /// Which socket this is.
    policy: Policy,
    /// Which run of the channel this is.
    run: u64,
    next: AtomicU64,
    open: AtomicUsize,
    silent: AtomicUsize,
    gate: Mutex<Gate>,
}

impl Shared {
    /// The threads' shared state for the channel's run `run`.
    pub fn new(run: u64) -> Self {
        Self {
            run,
            ..Self::default()
        }
    }

    /// The threads' shared state for the sessions' socket's run `run`.
    pub fn sessions(run: u64) -> Self {
        Self {
            run,
            policy: Policy::Sessions,
            ..Self::default()
        }
    }
}

/// Starts the accept loop on a background thread. `send` hands a request to the UI thread
/// and returns false once the UI is gone.
pub fn spawn<S>(listener: ControlListener, shared: Arc<Shared>, send: S) -> io::Result<()>
where
    S: Fn(Request) -> bool + Clone + Send + 'static,
{
    let name = match shared.policy {
        Policy::Channel => "oso-agent-accept",
        Policy::Sessions => "oso-session-accept",
    };
    thread::Builder::new()
        .name(name.into())
        .spawn(move || accept_loop(&listener, &shared, &send))?;
    Ok(())
}

fn accept_loop<S>(listener: &ControlListener, shared: &Arc<Shared>, send: &S)
where
    S: Fn(Request) -> bool + Clone + Send + 'static,
{
    loop {
        let accepted = listener.accept();
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        let stream = match accepted {
            Ok(stream) => stream,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                log::warn!("agent channel: {e}");
                continue;
            }
            Err(e) => {
                log::warn!("agent channel: accept failed: {e}");
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            }
        };
        // Who connected, read once, right away: the process may hand the socket on.
        let peer = stream.peer();
        let now = Instant::now();
        {
            let mut gate = gate(shared);
            gate.prune(now);
            if gate.accepts.len() >= MAX_ACCEPTS {
                drop(gate);
                refuse(stream, "too many connections this minute");
                continue;
            }
            gate.accepts.push_back(now);
        }
        let connection = Connection {
            run: shared.run,
            number: shared.next.fetch_add(1, Ordering::Relaxed),
        };
        let (shared, send) = (Arc::clone(shared), send.clone());
        let spawned = thread::Builder::new()
            .name("oso-agent-client".into())
            .spawn(move || {
                let Some(silent) = Slot::take(&shared.silent, MAX_SILENT) else {
                    refuse(
                        stream,
                        "too many connections wait to send their first request",
                    );
                    return;
                };
                if let Err(e) = serve(stream, peer, connection, silent, &shared, &send) {
                    log::debug!("agent channel: a client ended: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start a client thread: {e}");
        }
    }
}

/// Answers a client that cannot be served with one error, then hangs up.
fn refuse(mut stream: ControlStream, message: &str) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = writeln!(stream, "{}", failure(&Value::Null, message));
}

/// What the reading thread finds.
enum Line {
    Text(String),
    TooLong,
    End,
}

/// Reads lines on a thread of its own, so that a connection waiting for the person still
/// notices its client going.
fn spawn_reader(stream: ControlStream, shared: Arc<Shared>) -> io::Result<mpsc::Receiver<Line>> {
    // One line ahead, and no more: a client that sends faster waits, and holds no memory.
    let (lines, received) = mpsc::sync_channel(1);
    let mut reader = BufReader::new(stream);
    thread::Builder::new()
        .name("oso-agent-read".into())
        .spawn(move || {
            let mut line = Vec::new();
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    let _ = lines.send(Line::End);
                    return;
                }
                // A part of a line read before a timeout stays in `line`.
                let found = match read_line_within(&mut reader, &mut line, POLL) {
                    Ok(0) => Line::End,
                    Ok(_) if line.last() != Some(&b'\n') && line.len() > MAX_LINE => Line::TooLong,
                    Ok(_) if line.last() != Some(&b'\n') => {
                        // The client closed after a last line with no newline.
                        let text = String::from_utf8_lossy(&line).trim().to_owned();
                        if !text.is_empty() {
                            let _ = lines.send(Line::Text(text));
                        }
                        Line::End
                    }
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&line).trim().to_owned();
                        line.clear();
                        if text.is_empty() {
                            continue;
                        }
                        Line::Text(text)
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue;
                    }
                    Err(_) => Line::End,
                };
                let end = !matches!(found, Line::Text(_));
                if lines.send(found).is_err() || end {
                    return;
                }
            }
        })?;
    Ok(received)
}

/// Serves one connection: waits for its first request, asks the person, then answers its
/// requests until it goes.
fn serve<S>(
    stream: ControlStream,
    peer: Peer,
    connection: Connection,
    silent: Slot<'_>,
    shared: &Arc<Shared>,
    send: &S,
) -> io::Result<()>
where
    S: Fn(Request) -> bool,
{
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut closing = Closing {
        stream: stream.try_clone()?,
        connection,
        admitted: false,
        send,
    };
    let lines = spawn_reader(stream.try_clone()?, Arc::clone(shared))?;
    let mut writer = stream;
    let first = lines.recv_timeout(FIRST_REQUEST);
    let Ok(Line::Text(first)) = first else {
        // A probe, or a client that says nothing: closed, unasked, and noted now and then.
        if first.is_err() {
            note_silence(shared, send);
        }
        return Ok(());
    };
    drop(silent);
    let (first_id, claim) = match protocol::parse(&first) {
        Ok(request) => match request.method {
            Method::Version { session } => (request.id, session),
            Method::Ui(_) => (request.id, None),
        },
        Err(bad) => (bad.id, None),
    };
    let admitted = let_in(
        (&first_id, claim),
        (&peer, connection),
        (&lines, &mut writer, shared),
        send,
    )?;
    let Some(_slot) = admitted else {
        return Ok(());
    };
    closing.admitted = true;
    let mut rate = Rate::new(Instant::now());
    let mut line = Some(first);
    while let Some(text) = line.take() {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let response = if rate.take(Instant::now()) {
            match respond(
                &text,
                (connection, shared.policy),
                send,
                (&lines, &mut writer, shared),
            ) {
                Some(response) => response,
                // The client went, or the channel stopped, while the person had not answered.
                None => break,
            }
        } else {
            failure(&Value::Null, "too many requests; slow down")
        };
        writeln!(writer, "{response}")?;
        line = match lines.recv() {
            Ok(Line::Text(text)) => Some(text),
            Ok(Line::TooLong) => {
                let message = format!("request line is longer than {MAX_LINE} bytes");
                writeln!(writer, "{}", failure(&Value::Null, &message))?;
                None
            }
            Ok(Line::End) | Err(_) => None,
        };
    }
    Ok(())
}

/// Lets the connection of `peer` in after its first request `first_id`, which claims the
/// hosted session `claim` if it names one: a session's bridge unasked and in no slot, which is
/// all the sessions' socket lets in; any other once the person agrees, in one of the slots.
/// `None` once it was told no, or went; else the slot it holds, if any.
fn let_in<'s, S>(
    (first_id, claim): (&Value, Option<String>),
    (peer, connection): (&Peer, Connection),
    (lines, writer, shared): (&mpsc::Receiver<Line>, &mut ControlStream, &'s Shared),
    send: &S,
) -> io::Result<Option<Option<Slot<'s>>>>
where
    S: Fn(Request) -> bool,
{
    // A hosted session's bridge holds no slot of the two: each session holds one at most.
    let bridges = claim.is_some_and(|session| claimed(peer, session, connection, send));
    if bridges {
        return Ok(Some(None));
    }
    if shared.policy == Policy::Sessions {
        writeln!(writer, "{}", failure(first_id, ONLY_BRIDGES))?;
        return Ok(None);
    }
    let Some(slot) = Slot::take(&shared.open, MAX_CLIENTS) else {
        writeln!(
            writer,
            "{}",
            failure(first_id, "too many agents are connected")
        )?;
        return Ok(None);
    };
    match ask(peer, connection, (lines, writer, shared), send) {
        Ok(()) => Ok(Some(Some(slot))),
        Err(Asked::Refused(why)) => {
            writeln!(writer, "{}", failure(first_id, &why))?;
            Ok(None)
        }
        Err(Asked::Gone) => Ok(None),
    }
}

/// Ends a connection however `serve` returns: the socket closes for every handle, so its
/// reading thread ends too, and the UI hears that an admitted connection went.
struct Closing<'a, S: Fn(Request) -> bool> {
    stream: ControlStream,
    connection: Connection,
    admitted: bool,
    send: &'a S,
}

impl<S: Fn(Request) -> bool> Drop for Closing<'_, S> {
    fn drop(&mut self) {
        self.stream.shutdown();
        if self.admitted {
            (self.send)(Request {
                connection: self.connection,
                event: Event::Gone,
                reply: None,
            });
        }
    }
}

/// Tells the UI that a connection was closed for saying nothing, unless it was told less
/// than [`SILENT_NOTED_EVERY`] ago. A probe that connects and hangs up says nothing either,
/// but it closes before the deadline, and is not noted.
fn note_silence<S: Fn(Request) -> bool>(shared: &Shared, send: &S) {
    let now = Instant::now();
    {
        let mut gate = gate(shared);
        if gate
            .silent_noted
            .is_some_and(|at| now.duration_since(at) < SILENT_NOTED_EVERY)
        {
            return;
        }
        gate.silent_noted = Some(now);
    }
    send(Request {
        connection: Connection {
            run: shared.run,
            number: u64::MAX,
        },
        event: Event::Silent,
        reply: None,
    });
}

/// The response line for one request line of a connection the person let in, or `None` if
/// the client went, or the channel stopped, while the request waited for the person.
fn respond<S>(
    line: &str,
    (connection, policy): (Connection, Policy),
    send: &S,
    (lines, writer, shared): (&mpsc::Receiver<Line>, &mut ControlStream, &Shared),
) -> Option<String>
where
    S: Fn(Request) -> bool,
{
    let request = match protocol::parse(line) {
        Ok(request) => request,
        Err(bad) => return Some(bounded(&bad.id, failure(&bad.id, &bad.message))),
    };
    let op = match request.method {
        Method::Version { .. } => return Some(success(&request.id, version())),
        Method::Ui(op) => op,
    };
    let hold = match (&op, policy) {
        (Op::Permission(_), Policy::Sessions) => Hold::Person,
        (Op::Permission(_), Policy::Channel) => {
            return Some(failure(&request.id, NOT_ON_THE_CHANNEL));
        }
        (_, Policy::Sessions) => return Some(failure(&request.id, ONLY_PERMISSION)),
        (Op::Session(call), Policy::Channel) => call.hold(),
        (Op::MoveWait, Policy::Channel) => Hold::Job(protocol::MOVE_WAIT_WITHIN),
        (_, Policy::Channel) => Hold::Usual,
    };
    // Only a file that is there opens: the channel starts no new file (ADR 0085). Looked at
    // here, as the UI thread never waits on the file system.
    if let Op::Open { path, .. } = &op
        && let Err(why) = may_open(path)
    {
        return Some(failure(&request.id, why));
    }
    let (reply, replied) = mpsc::channel();
    let asked = Request {
        connection,
        event: Event::Op(op),
        reply: Some(reply),
    };
    if !send(asked) {
        return Some(failure(&request.id, "the editor is shutting down"));
    }
    Some(match held::wait(&replied, hold, lines, writer, shared) {
        Waited::Gone => return None,
        Waited::Replied(Ok(Ok(result))) => bounded(&request.id, success(&request.id, result)),
        Waited::Replied(Ok(Err(message))) => bounded(&request.id, failure(&request.id, &message)),
        Waited::Replied(Err(why)) => failure(&request.id, why),
    })
}

/// The result of `version`.
fn version() -> Value {
    json!({
        "protocol": PROTOCOL_VERSION,
        "oso": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    })
}

mod admitting;
mod held;
mod limits;
mod opening;

use admitting::{Asked, ask, claimed};

use held::Waited;
use limits::{Gate, Rate, Slot, gate};
pub use opening::MAX_OPEN_BYTES;
use opening::may_open;

// The tests need a socket, which only Unix has (ADR 0032).
#[cfg(all(test, unix))]
mod tests;
