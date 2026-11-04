//! The control socket server: background threads that read requests, hand
//! them to the UI thread and write the replies.
//!
//! Must not depend on: `app`, winit or editor state, which only the UI thread
//! touches.
//!
//! The UI thread never touches the socket. One thread accepts connections,
//! and each client gets its own thread, which parses requests and waits for
//! the UI's reply on a channel.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use oso_core::key::KeyPress;
use oso_platform::control::{ControlListener, ControlStream};
use serde_json::{Value, json};

use super::DEV_BUILD_MARKER;
use super::protocol::{self, MAX_LINE, Method};

/// The most clients connected at once.
pub const MAX_CLIENTS: usize = 4;

/// How long a client thread waits for the UI to reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a client may stay connected without sending a request. An idle
/// client is disconnected, so it cannot hold one of the slots forever.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long writing a response may block on a client that does not read.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the accept loop pauses after an unexpected error, so that a
/// persistent one (out of file descriptors) cannot spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// The UI's answer to a request: a JSON result or an error message.
pub type Reply = Result<Value, String>;

/// Work for the UI thread.
#[derive(Debug)]
pub enum UiOp {
    /// Handle these key presses, in order.
    SendKeys(Vec<KeyPress>),
    /// Capture the current frame.
    Screenshot,
    /// Report the editor state.
    GetState,
    /// Report whole rows of the selected window's buffer.
    GetText { start_row: usize, rows: usize },
    /// Run a registered command by name.
    RunCommand { name: String, arg: Option<isize> },
    /// Show a buffer in the selected window.
    ShowBuffer(u64),
}

/// A request for the UI thread, with the channel for its reply. Dropping
/// the sender without replying is reported to the client as an error.
#[derive(Debug)]
pub struct UiRequest {
    pub op: UiOp,
    pub reply: mpsc::Sender<Reply>,
}

/// Starts the accept loop on a background thread. `send` hands a request to
/// the UI thread and returns false once the UI is gone.
pub fn spawn<S>(listener: ControlListener, send: S) -> io::Result<()>
where
    S: Fn(UiRequest) -> bool + Clone + Send + 'static,
{
    spawn_with_idle_timeout(listener, send, IDLE_TIMEOUT)
}

/// [`spawn`], with clients disconnected after `idle` without a request.
fn spawn_with_idle_timeout<S>(listener: ControlListener, send: S, idle: Duration) -> io::Result<()>
where
    S: Fn(UiRequest) -> bool + Clone + Send + 'static,
{
    thread::Builder::new()
        .name("oso-control-accept".into())
        .spawn(move || accept_loop(&listener, &send, idle))?;
    Ok(())
}

fn accept_loop<S>(listener: &ControlListener, send: &S, idle: Duration)
where
    S: Fn(UiRequest) -> bool + Clone + Send + 'static,
{
    let active = Arc::new(AtomicUsize::new(0));
    loop {
        let stream = match listener.accept() {
            Ok(stream) => stream,
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                log::warn!("control socket: {e}");
                continue;
            }
            Err(e) => {
                log::warn!("control socket: accept failed: {e}");
                thread::sleep(ACCEPT_BACKOFF);
                continue;
            }
        };
        let Some(slot) = Slot::take(&active) else {
            refuse(stream, "too many control clients are connected");
            continue;
        };
        let send = send.clone();
        let spawned = thread::Builder::new()
            .name("oso-control-client".into())
            .spawn(move || {
                let _slot = slot;
                if let Err(e) = serve(stream, &send, idle) {
                    log::debug!("control socket: client ended: {e}");
                }
            });
        if let Err(e) = spawned {
            log::warn!("control socket: cannot start a client thread: {e}");
        }
    }
}

/// One of the [`MAX_CLIENTS`] client slots, given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(active: &Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_CLIENTS).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(active)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Answers a client that cannot be served with one error, then hangs up.
fn refuse(mut stream: ControlStream, message: &str) {
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let _ = writeln!(stream, "{}", protocol::failure(&Value::Null, message));
}

/// Serves one client until it hangs up, sends a line that is too long, or
/// sends nothing for `idle`.
pub fn serve<S>(stream: ControlStream, send: &S, idle: Duration) -> io::Result<()>
where
    S: Fn(UiRequest) -> bool,
{
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_read_timeout(Some(idle))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = Vec::new();
    loop {
        line.clear();
        let limit = u64::try_from(MAX_LINE + 1).expect("the limit fits in u64");
        let n = match (&mut reader).take(limit).read_until(b'\n', &mut line) {
            Ok(n) => n,
            // Idle for too long: a normal end, not an error.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if n == 0 {
            return Ok(());
        }
        let ended = line.last() == Some(&b'\n');
        if !ended && line.len() > MAX_LINE {
            let message = format!("request line is longer than {MAX_LINE} bytes");
            writeln!(writer, "{}", protocol::failure(&Value::Null, &message))?;
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if !text.is_empty() {
            writeln!(writer, "{}", respond(text, send))?;
        }
        if !ended {
            // The client closed the connection after a final line.
            return Ok(());
        }
    }
}

/// The response line for one request line.
fn respond<S>(line: &str, send: &S) -> String
where
    S: Fn(UiRequest) -> bool,
{
    let request = match protocol::parse(line) {
        Ok(request) => request,
        Err(bad) => return protocol::failure(&bad.id, &bad.message),
    };
    let op = match request.method {
        Method::Version => {
            return protocol::success(&request.id, version());
        }
        Method::SendKeys(keys) => UiOp::SendKeys(keys),
        Method::Screenshot => UiOp::Screenshot,
        Method::GetState => UiOp::GetState,
        Method::GetText { start_row, rows } => UiOp::GetText { start_row, rows },
        Method::RunCommand { name, arg } => UiOp::RunCommand { name, arg },
        Method::ShowBuffer(buffer) => UiOp::ShowBuffer(buffer),
    };
    let (reply, replied) = mpsc::channel();
    if !send(UiRequest { op, reply }) {
        return protocol::failure(&request.id, "the editor is shutting down");
    }
    match replied.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(result)) => bounded(&request.id, protocol::success(&request.id, result)),
        Ok(Err(message)) => protocol::failure(&request.id, &message),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            protocol::failure(&request.id, "the editor did not reply in time")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            protocol::failure(&request.id, "the editor dropped the request")
        }
    }
}

/// `line`, or an error in its place if a client could not read it: `oso
/// ctl` and the protocol stop at [`MAX_LINE`] (ADR 0032).
fn bounded(id: &Value, line: String) -> String {
    if line.len() > MAX_LINE {
        let message = format!(
            "the response takes {} bytes, more than the {MAX_LINE} a line may hold; ask for less",
            line.len()
        );
        return protocol::failure(id, &message);
    }
    line
}

/// The result of `version`.
fn version() -> Value {
    json!({
        "protocol": protocol::PROTOCOL_VERSION,
        "oso": env!("CARGO_PKG_VERSION"),
        "build": DEV_BUILD_MARKER,
        "pid": std::process::id(),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        IDLE_TIMEOUT, MAX_CLIENTS, UiOp, UiRequest, serve, spawn, spawn_with_idle_timeout,
    };
    use crate::dev::protocol::{self, MAX_LINE};
    use oso_platform::control::{ControlDir, ControlStream};
    use serde_json::{Value, json};
    use std::fs::{self, Permissions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::Duration;

    /// A fake UI thread: answers keys with their count, state with a fixed
    /// value, commands with an error or their name, and text with as many
    /// bytes as `rows` asks for, whatever the limits. Drops screenshots
    /// without replying.
    fn fake_ui(request: UiRequest) -> bool {
        let result = match request.op {
            UiOp::SendKeys(keys) => Ok(json!({ "keys": keys.len() })),
            UiOp::GetState => Ok(json!({ "cursor": { "position": 3 } })),
            UiOp::GetText { rows, .. } => Ok(json!({ "text": "x".repeat(rows) })),
            UiOp::RunCommand { name, .. } if name == "fail" => Err("fail panicked".to_owned()),
            UiOp::RunCommand { name, arg } => Ok(json!({ "command": name, "arg": arg })),
            UiOp::ShowBuffer(buffer) => Ok(json!({ "buffer": buffer })),
            UiOp::Screenshot => return true,
        };
        let _ = request.reply.send(result);
        true
    }

    /// Serves a fake UI on one end of a socket pair, and returns the other.
    fn connected() -> (BufReader<ControlStream>, ControlStream) {
        let (client, server) = ControlStream::pair().unwrap();
        thread::spawn(move || serve(server, &fake_ui, IDLE_TIMEOUT));
        let reader = BufReader::new(client.try_clone().unwrap());
        (reader, client)
    }

    fn call(
        reader: &mut BufReader<ControlStream>,
        writer: &mut ControlStream,
        line: &str,
    ) -> Value {
        writeln!(writer, "{line}").unwrap();
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn answers_requests_in_order_on_one_connection() {
        let (mut reader, mut writer) = connected();
        let version = call(
            &mut reader,
            &mut writer,
            &protocol::request(1, "version", None),
        );
        assert_eq!(version["id"], 1);
        assert_eq!(version["result"]["protocol"], 1);
        assert_eq!(version["result"]["build"], crate::dev::DEV_BUILD_MARKER);

        let keys = call(
            &mut reader,
            &mut writer,
            &protocol::request(2, "send-keys", Some(json!({ "keys": "<right> <right>" }))),
        );
        assert_eq!(keys, json!({ "id": 2, "result": { "keys": 2 } }));

        let state = call(
            &mut reader,
            &mut writer,
            &protocol::request(3, "get-state", None),
        );
        assert_eq!(state["result"]["cursor"]["position"], 3);
    }

    #[test]
    fn reports_errors_and_keeps_the_connection() {
        let (mut reader, mut writer) = connected();
        let bad = call(&mut reader, &mut writer, "{not json");
        assert!(
            bad["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("invalid JSON")
        );
        let unknown = call(&mut reader, &mut writer, r#"{"id": 5, "method": "nope"}"#);
        assert_eq!(unknown["id"], 5);
        assert!(
            unknown["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown method")
        );
        // A request the UI drops is answered, not left hanging.
        let dropped = call(
            &mut reader,
            &mut writer,
            &protocol::request(6, "screenshot", None),
        );
        assert_eq!(
            dropped["error"]["message"],
            "the editor dropped the request"
        );
        let still = call(
            &mut reader,
            &mut writer,
            &protocol::request(7, "version", None),
        );
        assert_eq!(still["id"], 7);
    }

    #[test]
    fn commands_reach_the_ui_and_their_errors_come_back() {
        let (mut reader, mut writer) = connected();
        let run = |name: &str, arg: Value| {
            protocol::request(1, "run-command", Some(json!({ "name": name, "arg": arg })))
        };
        let ok = call(&mut reader, &mut writer, &run("forward-char", json!(2)));
        assert_eq!(ok["result"], json!({ "command": "forward-char", "arg": 2 }));
        let failed = call(&mut reader, &mut writer, &run("fail", json!(null)));
        assert_eq!(failed["error"]["message"], "fail panicked");
    }

    #[test]
    fn a_response_past_the_line_limit_becomes_an_error() {
        let (mut reader, mut writer) = connected();
        let text = |rows: usize| {
            protocol::request(1, "get-text", Some(json!({ "start_row": 0, "rows": rows })))
        };
        let fits = MAX_LINE - 100;
        let ok = call(&mut reader, &mut writer, &text(fits));
        assert_eq!(ok["result"]["text"].as_str().unwrap().len(), fits);
        let too_long = call(&mut reader, &mut writer, &text(MAX_LINE));
        assert_eq!(too_long["id"], 1);
        let message = too_long["error"]["message"].as_str().unwrap();
        assert!(message.contains("ask for less"), "{message}");
        // The connection survives it.
        let still = call(
            &mut reader,
            &mut writer,
            &protocol::request(2, "version", None),
        );
        assert_eq!(still["id"], 2);
    }

    #[test]
    fn a_closed_ui_is_reported() {
        let (client, server) = ControlStream::pair().unwrap();
        thread::spawn(move || serve(server, &|_| false, IDLE_TIMEOUT));
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut writer = client;
        let response = call(
            &mut reader,
            &mut writer,
            &protocol::request(1, "get-state", None),
        );
        assert_eq!(response["error"]["message"], "the editor is shutting down");
    }

    #[test]
    fn hangs_up_on_a_line_longer_than_the_limit() {
        let (mut reader, mut writer) = connected();
        let writing = thread::spawn(move || {
            // The server stops reading at the limit, so the rest may fail.
            let _ = writer.write_all(&vec![b'x'; MAX_LINE + 1]);
            let _ = writer.write_all(b"\n");
        });
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("longer than")
        );
        // Closed: Linux reports a reset, since the server left bytes unread.
        let mut rest = Vec::new();
        match reader.read_to_end(&mut rest) {
            Ok(_) => assert!(rest.is_empty(), "the connection is closed"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset),
        }
        writing.join().unwrap();
    }

    #[test]
    fn accepts_a_line_at_the_limit() {
        let (mut reader, mut writer) = connected();
        let request = protocol::request(1, "version", None);
        let padded = format!("{request}{}", " ".repeat(MAX_LINE - request.len()));
        assert_eq!(padded.len(), MAX_LINE);
        let response = call(&mut reader, &mut writer, &padded);
        assert_eq!(response["id"], 1);
    }

    #[test]
    fn disconnects_an_idle_client() {
        let (client, server) = ControlStream::pair().unwrap();
        let served = thread::spawn(move || serve(server, &fake_ui, Duration::from_millis(100)));
        // The client sends nothing; the server hangs up on its own.
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut rest = Vec::new();
        BufReader::new(client).read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
        assert!(
            served.join().unwrap().is_ok(),
            "an idle client is a normal end"
        );
    }

    #[test]
    fn idle_clients_give_their_slots_back() {
        let (dir, scratch) = control_dir();
        let listener = dir.bind().unwrap();
        let cleanup = listener.cleanup_guard();
        spawn_with_idle_timeout(listener, fake_ui, Duration::from_secs(1)).unwrap();

        // Every slot is taken by a client that never sends anything.
        let idle: Vec<_> = (0..MAX_CLIENTS)
            .map(|_| dir.connect(None).unwrap().1)
            .collect();
        thread::sleep(Duration::from_millis(50));
        let (_, mut extra) = dir.connect(None).unwrap();
        let _ = writeln!(extra, "{}", protocol::request(1, "version", None));
        let mut response = String::new();
        let _ = BufReader::new(extra).read_line(&mut response);
        assert!(response.contains("too many control clients"), "{response}");

        // Once they time out, a new client is served.
        thread::sleep(Duration::from_millis(1500));
        let (_, client) = dir.connect(None).unwrap();
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut writer = client;
        let response = call(
            &mut reader,
            &mut writer,
            &protocol::request(2, "version", None),
        );
        assert_eq!(response["id"], 2);

        drop(idle);
        drop(cleanup);
        let _ = fs::remove_dir_all(scratch);
    }

    /// A fresh control directory for one test.
    fn control_dir() -> (ControlDir, PathBuf) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("osos-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
        (ControlDir::at(path.join("c")).unwrap(), path)
    }

    #[test]
    fn serves_over_a_real_socket_and_limits_clients() {
        let (dir, scratch) = control_dir();
        let listener = dir.bind().unwrap();
        let cleanup = listener.cleanup_guard();
        spawn(listener, fake_ui).unwrap();

        // Fill every slot; each client proves it is served.
        let mut clients = Vec::new();
        for id in 0..MAX_CLIENTS {
            let (_, client) = dir.connect(None).unwrap();
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let mut writer = client;
            let id = u64::try_from(id).unwrap();
            let response = call(
                &mut reader,
                &mut writer,
                &protocol::request(id, "version", None),
            );
            assert_eq!(response["id"], id);
            clients.push((reader, writer));
        }
        // One more is refused with an error and closed.
        let (_, extra) = dir.connect(None).unwrap();
        let mut response = String::new();
        BufReader::new(extra).read_line(&mut response).unwrap();
        assert!(response.contains("too many control clients"), "{response}");

        // A slot frees up when a client leaves.
        drop(clients.pop());
        let mut served = false;
        for _ in 0..50 {
            // Until the slot is free, the connection may be refused and
            // closed, so errors here only mean "try again".
            let (_, mut client) = dir.connect(None).unwrap();
            let request = protocol::request(9, "version", None);
            let _ = writeln!(client, "{request}");
            let mut response = String::new();
            let _ = BufReader::new(client).read_line(&mut response);
            if response.contains(r#""id":9"#) {
                served = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(served, "a freed slot is reused");

        drop(cleanup);
        let _ = fs::remove_dir_all(scratch);
    }
}
