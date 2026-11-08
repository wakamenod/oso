//! The agent channel's server (ADR 0085): background threads that take connections, hold
//! each one until the person lets it in, and then hand its requests to the UI thread.
//!
//! Must not depend on: `app`, winit or editor state, which only the UI thread touches.
//!
//! - A connection that sends no request within [`FIRST_REQUEST`] is closed, unasked and
//!   uncounted: probes, such as the stale-socket cleanup, connect and send nothing.
//! - Its first request asks the person, through the UI thread, and is answered only once
//!   they agree. One connection waits at a time, at most [`MAX_ASKS`] ask a minute, and none
//!   for [`REFUSED_FOR`] after the person says no.
//! - Every connection counts toward [`MAX_ACCEPTS`] a minute, and at most [`MAX_CLIENTS`]
//!   are open at once, so no process can hold the slots or churn connections unseen.
//! - Once let in, a connection sends at most [`MAX_RATE`] requests a second, one at a time,
//!   with no idle limit.

use std::io::{self, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlListener, ControlStream, Peer};
use serde_json::{Value, json};

use super::protocol::{self, Method, Op, PROTOCOL_VERSION};
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
    /// `Ok` reply lets it in.
    Asks(Peer),
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

/// What the server's threads share.
#[derive(Debug, Default)]
pub struct Shared {
    /// The channel stopped: every thread ends.
    pub stop: AtomicBool,
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
}

/// Starts the accept loop on a background thread. `send` hands a request to the UI thread
/// and returns false once the UI is gone.
pub fn spawn<S>(listener: ControlListener, shared: Arc<Shared>, send: S) -> io::Result<()>
where
    S: Fn(Request) -> bool + Clone + Send + 'static,
{
    thread::Builder::new()
        .name("oso-agent-accept".into())
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
    let first_id = protocol::parse(&first).map_or_else(|bad| bad.id, |request| request.id);
    let Some(_slot) = Slot::take(&shared.open, MAX_CLIENTS) else {
        writeln!(
            writer,
            "{}",
            failure(&first_id, "too many agents are connected")
        )?;
        return Ok(());
    };
    if let Err(why) = gate(shared).may_ask(Instant::now()) {
        writeln!(writer, "{}", failure(&first_id, why))?;
        return Ok(());
    }
    let asked = ask(&peer, connection, &lines, &mut writer, shared, send);
    // Only the person's own `n` holds others off: a client that goes, or a channel that
    // stops, is no refusal.
    let refused = matches!(&asked, Err(Asked::Refused(why)) if why == REFUSED_BY_THE_PERSON);
    gate(shared).answered(refused, Instant::now());
    match asked {
        Ok(()) => closing.admitted = true,
        Err(Asked::Refused(why)) => {
            writeln!(writer, "{}", failure(&first_id, &why))?;
            return Ok(());
        }
        Err(Asked::Gone) => return Ok(()),
    }
    let mut rate = Rate::new(Instant::now());
    let mut line = Some(first);
    while let Some(text) = line.take() {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let response = if rate.take(Instant::now()) {
            respond(&text, connection, send)
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

/// How asking the person ended, when it did not let the connection in.
enum Asked {
    Refused(String),
    Gone,
}

/// Asks the person, through the UI thread, whether this connection may drive the editor,
/// and waits as long as they take. Tells the UI if the client goes or the channel stops
/// meanwhile, so that the question closes.
fn ask<S>(
    peer: &Peer,
    connection: Connection,
    lines: &mpsc::Receiver<Line>,
    writer: &mut ControlStream,
    shared: &Shared,
    send: &S,
) -> Result<(), Asked>
where
    S: Fn(Request) -> bool,
{
    let (reply, replied) = mpsc::channel();
    let asks = Request {
        connection,
        event: Event::Asks(peer.clone()),
        reply: Some(reply),
    };
    if !send(asks) {
        return Err(Asked::Refused("the editor is shutting down".into()));
    }
    let gone = || {
        send(Request {
            connection,
            event: Event::Gone,
            reply: None,
        });
        Err(Asked::Gone)
    };
    let mut extra = 0;
    loop {
        match replied.recv_timeout(POLL) {
            Ok(Ok(_)) => return Ok(()),
            Ok(Err(why)) => return Err(Asked::Refused(why)),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(Asked::Refused("the editor dropped the request".into()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if shared.stop.load(Ordering::Acquire) {
            return gone();
        }
        match lines.try_recv() {
            Ok(Line::Text(_)) if extra >= MAX_WAITING_LINES => return gone(),
            Ok(Line::Text(text)) => {
                extra += 1;
                let id = protocol::parse(&text).map_or_else(|bad| bad.id, |request| request.id);
                let waiting = failure(&id, "wait: the person has not answered yet");
                if writeln!(writer, "{waiting}").is_err() {
                    return gone();
                }
            }
            Ok(Line::TooLong | Line::End) | Err(mpsc::TryRecvError::Disconnected) => return gone(),
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

/// The response line for one request line of a connection the person let in.
fn respond<S>(line: &str, connection: Connection, send: &S) -> String
where
    S: Fn(Request) -> bool,
{
    let request = match protocol::parse(line) {
        Ok(request) => request,
        Err(bad) => return bounded(&bad.id, failure(&bad.id, &bad.message)),
    };
    let op = match request.method {
        Method::Version => return success(&request.id, version()),
        Method::Ui(op) => op,
    };
    let (reply, replied) = mpsc::channel();
    let asked = Request {
        connection,
        event: Event::Op(op),
        reply: Some(reply),
    };
    if !send(asked) {
        return failure(&request.id, "the editor is shutting down");
    }
    match replied.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(result)) => bounded(&request.id, success(&request.id, result)),
        Ok(Err(message)) => bounded(&request.id, failure(&request.id, &message)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            failure(&request.id, "the editor did not reply in time")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            failure(&request.id, "the editor dropped the request")
        }
    }
}

/// The result of `version`.
fn version() -> Value {
    json!({
        "protocol": PROTOCOL_VERSION,
        "oso": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    })
}

mod limits;

use limits::{Gate, Rate, Slot, gate};

// The tests need a socket, which only Unix has (ADR 0032).
#[cfg(all(test, unix))]
mod tests;
