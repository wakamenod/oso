//! The control socket server: background threads that read requests, hand
//! them to the UI thread and write the replies.
//!
//! Must not depend on: `app`, winit or editor state, which only the UI thread
//! touches.
//!
//! The UI thread never touches the socket. One thread accepts connections,
//! and each client gets its own thread, which parses requests and waits for
//! the UI's reply on a channel.

use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlListener, ControlStream};
use serde_json::{Value, json};

use super::DEV_BUILD_MARKER;
use super::protocol::{self, MAX_LINE, Method, Reply, UiOp};

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

/// A request for the UI thread, with the channel for its reply. Dropping
/// the sender without replying is reported to the client as an error.
#[derive(Debug)]
pub struct UiRequest {
    pub op: UiOp,
    /// When the socket thread read the request, so that a key's latency
    /// includes any wait for a busy UI thread (ADR 0046).
    pub received: Instant,
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
/// does not finish a request within `idle`.
pub fn serve<S>(stream: ControlStream, send: &S, idle: Duration) -> io::Result<()>
where
    S: Fn(UiRequest) -> bool,
{
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = match read_line_within(&mut reader, &mut line, idle) {
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

/// Reads up to a newline into `line`, at most [`MAX_LINE`] + 1 bytes, and
/// returns how many. Times out if the whole line takes longer than `idle`, so
/// a client cannot hold its slot by sending a byte now and then.
fn read_line_within(
    reader: &mut BufReader<ControlStream>,
    line: &mut Vec<u8>,
    idle: Duration,
) -> io::Result<usize> {
    let started = Instant::now();
    loop {
        let remaining = idle
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(io::ErrorKind::TimedOut)?;
        reader.get_ref().set_read_timeout(Some(remaining))?;
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok(line.len());
        }
        let room = MAX_LINE + 1 - line.len();
        let window = &available[..available.len().min(room)];
        let newline = window.iter().position(|&byte| byte == b'\n');
        let used = newline.map_or(window.len(), |at| at + 1);
        line.extend_from_slice(&window[..used]);
        reader.consume(used);
        if newline.is_some() || line.len() > MAX_LINE {
            return Ok(line.len());
        }
    }
}

/// The response line for one request line.
fn respond<S>(line: &str, send: &S) -> String
where
    S: Fn(UiRequest) -> bool,
{
    let received = Instant::now();
    let request = match protocol::parse(line) {
        Ok(request) => request,
        Err(bad) => return bounded(&bad.id, protocol::failure(&bad.id, &bad.message)),
    };
    let op = match request.method {
        Method::Version => return protocol::success(&request.id, version()),
        Method::Ui(op) => op,
    };
    let (reply, replied) = mpsc::channel();
    if !send(UiRequest {
        op,
        received,
        reply,
    }) {
        return protocol::failure(&request.id, "the editor is shutting down");
    }
    match replied.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(result)) => bounded(&request.id, protocol::success(&request.id, result)),
        Ok(Err(message)) => bounded(&request.id, protocol::failure(&request.id, &message)),
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
    use super::{IDLE_TIMEOUT, MAX_CLIENTS, UiRequest, serve, spawn, spawn_with_idle_timeout};
    use crate::dev::protocol::{self, MAX_LINE, UiOp};
    use oso_platform::control::{ControlDir, ControlStream};
    use serde_json::{Value, json};
    use std::fs::{self, Permissions};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    /// A fake UI thread: answers keys with their count, state with a fixed
    /// value, commands with an error or their name, and text with as many
    /// bytes as `rows` asks for, whatever the limits. Drops screenshots
    /// without replying.
    fn fake_ui(request: UiRequest) -> bool {
        let result = match request.op {
            UiOp::SendKeys(keys) => Ok(json!({ "keys": keys.len() })),
            UiOp::GetState => Ok(json!({ "cursor": { "position": 3 } })),
            UiOp::TakeTimings => Ok(json!({ "key_latency_us": [] })),
            UiOp::OpenFile(_) => Ok(json!({ "opened": true })),
            UiOp::GetText { rows, .. } => Ok(json!({ "text": "x".repeat(rows) })),
            UiOp::RunCommand { name, .. } if name == "fail" => Err("fail panicked".to_owned()),
            UiOp::RunCommand { name, arg } => Ok(json!({ "command": name, "arg": arg })),
            UiOp::ShowBuffer(buffer) => Ok(json!({ "buffer": buffer })),
            UiOp::Panic
            | UiOp::EndUnasked
            | UiOp::Ime(_)
            | UiOp::Clipboard(_)
            | UiOp::ExportText { .. }
            | UiOp::ImportText(_)
            | UiOp::DevMessage(_) => Err("not in this fake".to_owned()),
            UiOp::CheckHandover | UiOp::ExportState | UiOp::ImportState(_) | UiOp::Exit { .. } => {
                Ok(json!({}))
            }
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

    /// Asserts that the server has hung up with nothing more to say. Linux reports
    /// a reset if it left bytes unread.
    fn assert_closed(reader: &mut impl Read) {
        let mut rest = Vec::new();
        if let Err(e) = reader.read_to_end(&mut rest) {
            assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
        }
        assert!(rest.is_empty(), "the connection is closed");
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
        assert_closed(&mut reader);
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
    fn disconnects_a_client_that_never_finishes_a_request() {
        let (client, server) = ControlStream::pair().unwrap();
        let limit = Duration::from_millis(200);
        let started = Instant::now();
        let served = thread::spawn(move || serve(server, &fake_ui, limit));
        let mut writer = client.try_clone().unwrap();
        // A byte now and then, sooner than the limit, and never a newline.
        thread::spawn(move || {
            while writer.write_all(b"x").is_ok() {
                thread::sleep(Duration::from_millis(50));
            }
        });
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        // A byte may have just arrived, unread, when the server hangs up.
        assert_closed(&mut BufReader::new(client));
        assert!(started.elapsed() >= limit, "not before the limit");
        assert!(served.join().unwrap().is_ok());
    }

    /// What `assert_closed` allows for. It holds only on Linux, so CI checks it.
    #[cfg(target_os = "linux")]
    #[test]
    fn closing_with_bytes_unread_resets_the_connection() {
        let (mut client, server) = ControlStream::pair().unwrap();
        client.write_all(b"x").unwrap();
        drop(server);
        let error = client.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
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
