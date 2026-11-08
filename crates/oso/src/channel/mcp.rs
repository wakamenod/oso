//! `oso mcp` (ADR 0031, ADR 0085): the stdio bridge an agent such as Claude Code starts, which
//! speaks the Model Context Protocol's JSON-RPC on its stdin and stdout and hands each tool
//! call to the agent channel of the oso instance whose channel is on.
//!
//! Must not depend on: `app`, `dispatch` or the editor; the bridge is a client of the channel,
//! like any other, and the person lets each of its connections in.
//!
//! - One request per line of stdin, at most [`MAX_LINE`] bytes, and one response per line of
//!   stdout, escaped as the channel's are. It says what goes wrong on stderr only.
//! - It connects on the first tool call, only to an oso that runs this program, and asks the
//!   channel's `version` first. A connection that breaks, as when oso restarts, is made again
//!   on the next call, which asks the person again.
//! - stdin is read on a thread of its own: when it ends, or the agent cancels the call that
//!   waits, the connection closes, so that the person's question closes and nothing the agent
//!   gave up on runs later.
//! - Tools: `state`, `text`, `commands`, `look`, `change` and `open`. Looking and changing are
//!   separate tools, so that the agent's own permission rules can allow one and ask for the
//!   other.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Map, Value, json};

use super::protocol::PROTOCOL_VERSION;
use super::wire::{MAX_ID, MAX_LINE, escape_unsafe, parse_response, read_bounded_line, short};

/// The protocol versions of MCP this bridge speaks, the newest first.
const MCP_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the bridge tells the agent about itself when it starts.
const INSTRUCTIONS: &str = "Drives the person's oso editor through its agent channel. The person turns the channel on with M-x agent-channel-start and lets each connection in; until then, tool calls fail and say so. A refusal that says the person is busy means: wait, and try again. Commands that need a person at the keyboard are never run.";

/// A connection to the channel.
trait Link {
    /// Sends `method` with `params`, and waits for the reply.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed>;

    /// What closes the connection from another thread, so that a call waiting on it ends.
    fn closer(&self) -> Option<Box<dyn Fn() + Send>> {
        None
    }
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failed {
    /// The channel answered with an error.
    Refused(String),
    /// The connection broke, or there is none: the next call connects again.
    Broken(String),
}

impl Failed {
    fn message(self) -> String {
        match self {
            Self::Refused(why) | Self::Broken(why) => why,
        }
    }
}

/// How the bridge connects.
type Connect = Box<dyn FnMut() -> Result<Box<dyn Link>, String>>;

/// What the stdin thread shares with the bridge: whether a call waits on the connection, and
/// what closes it.
#[derive(Default)]
struct Interrupt {
    waiting: AtomicBool,
    /// The agent's stdin ended: it has gone, and no call is made for it any more.
    gone: AtomicBool,
    close: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Interrupt {
    /// Closes the connection if a call waits on it.
    fn fire(&self) {
        if self.waiting.load(Ordering::Acquire)
            && let Some(close) = &*self.close.lock().unwrap_or_else(PoisonError::into_inner)
        {
            close();
        }
    }

    /// The agent has gone: a call that waits ends, and none starts.
    fn end(&self) {
        self.gone.store(true, Ordering::Release);
        self.fire();
    }

    fn set_close(&self, close: Option<Box<dyn Fn() + Send>>) {
        *self.close.lock().unwrap_or_else(PoisonError::into_inner) = close;
    }
}

/// The bridge: what it has connected to, and how to connect.
struct Bridge {
    connect: Connect,
    link: Option<Box<dyn Link>>,
    interrupt: Arc<Interrupt>,
}

impl Bridge {
    /// A bridge that connects with `connect` on its first tool call.
    fn new(connect: Connect) -> Self {
        Self {
            connect,
            link: None,
            interrupt: Arc::default(),
        }
    }

    /// The response line to one request line, or `None` for a notification.
    fn respond(&mut self, line: &str) -> Option<String> {
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(e) => return Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let Some(request) = request.as_object() else {
            return Some(error(&Value::Null, -32600, "a request is a JSON object"));
        };
        let id = request.get("id").cloned();
        if let Some(Value::String(id)) = &id
            && id.len() > MAX_ID
        {
            let message = format!("an id is at most {MAX_ID} bytes");
            return Some(error(&Value::Null, -32600, &message));
        }
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return Some(error(
                &id.unwrap_or(Value::Null),
                -32600,
                "a request needs a method",
            ));
        };
        // A notification gets no response, and runs nothing.
        let id = id?;
        let params = request.get("params").and_then(Value::as_object);
        let empty = Map::new();
        let params = params.unwrap_or(&empty);
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => self.call_tool(params),
            _ => Err((-32601, format!("no method {:?}", short(method)))),
        };
        Some(match result {
            Ok(result) => {
                escape_unsafe(json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string())
            }
            Err((code, message)) => error(&id, code, &message),
        })
    }

    /// Runs one tool, as the channel's method its arguments say.
    fn call_tool(&mut self, params: &Map<String, Value>) -> Result<Value, (i64, String)> {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Err((-32602, "tools/call needs a tool's \"name\"".into()));
        };
        let empty = Map::new();
        let arguments = params
            .get("arguments")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let (method, params) = channel_request(name, arguments).map_err(|why| (-32602, why))?;
        Ok(match self.call(method, params) {
            Ok(result) => tool_result(&result.to_string(), false),
            Err(failed) => tool_result(&failed.message(), true),
        })
    }

    /// Calls the channel, connecting first if there is no connection, while the stdin thread
    /// may close it.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        self.interrupt.waiting.store(true, Ordering::Release);
        let result = self.call_waiting(method, params);
        self.interrupt.waiting.store(false, Ordering::Release);
        if matches!(result, Err(Failed::Broken(_))) {
            self.link = None;
            self.interrupt.set_close(None);
        }
        result
    }

    /// A new connection asks the channel's `version` first, and is dropped if it is not this
    /// bridge's.
    fn call_waiting(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        if self.interrupt.gone.load(Ordering::Acquire) {
            return Err(Failed::Broken("the agent has gone".into()));
        }
        if self.link.is_none() {
            let mut link = (self.connect)().map_err(Failed::Broken)?;
            self.interrupt.set_close(link.closer());
            let version = link.call("version", json!({}))?;
            if version["protocol"] != PROTOCOL_VERSION {
                return Err(Failed::Broken(format!(
                    "this oso speaks the channel's protocol {}, and this oso mcp {PROTOCOL_VERSION}",
                    version["protocol"]
                )));
            }
            self.link = Some(link);
        }
        let link = self
            .link
            .as_mut()
            .ok_or_else(|| Failed::Broken("no connection".into()))?;
        link.call(method, params)
    }
}

/// A JSON-RPC error response line.
fn error(id: &Value, code: i64, message: &str) -> String {
    escape_unsafe(
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            .to_string(),
    )
}

/// The channel of a running oso, over its socket.
struct SocketLink {
    writer: ControlStream,
    reader: BufReader<ControlStream>,
    next_id: u64,
}

impl Link for SocketLink {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        let broken = |e: io::Error| Failed::Broken(format!("the connection to oso broke: {e}"));
        writeln!(self.writer, "{request}").map_err(broken)?;
        let mut line = Vec::new();
        // The person may take as long as they like to let this connection in.
        if read_bounded_line(&mut self.reader, &mut line).map_err(broken)? == 0 {
            return Err(Failed::Broken("oso closed the connection".into()));
        }
        if line.last() != Some(&b'\n') {
            return Err(Failed::Broken(
                "oso sent a line longer than a response may be".into(),
            ));
        }
        match parse_response(&String::from_utf8_lossy(&line)) {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(Failed::Refused(message)),
            Err(why) => Err(Failed::Broken(format!(
                "oso sent what is no response: {why}"
            ))),
        }
    }

    fn closer(&self) -> Option<Box<dyn Fn() + Send>> {
        let stream = self.writer.try_clone().ok()?;
        Some(Box::new(move || stream.shutdown()))
    }
}

/// Connects to the agent channel of the instance with process ID `pid`, or of the only one
/// whose channel is on. Only an oso that runs this program, and is the process that owns the
/// socket, is spoken to: another process of the same user could listen in its place. What
/// goes wrong is said in full on stderr, and to the agent without the paths.
fn connect(pid: Option<u32>) -> Result<Box<dyn Link>, String> {
    let fail = |detail: String, told: &str| {
        say(&detail);
        told.to_owned()
    };
    let off = "no oso instance has the agent channel on; the person turns it on with M-x agent-channel-start";
    let dir = ControlDir::open_existing()
        .map_err(|e| fail(format!("the control directory: {e}"), off))?;
    let (found, stream) = dir
        .connect(Socket::Agent, pid)
        .map_err(|e| fail(format!("connecting: {e}"), off))?;
    let peer = stream.peer();
    let is_oso = peer.runs_this_program
        && peer
            .process
            .as_ref()
            .is_some_and(|process| process.pid() == found);
    if !is_oso {
        return Err(fail(
            format!("the agent socket of pid {found} is served by a process that is not this oso"),
            "the agent channel's socket is served by something other than oso; ask the person",
        ));
    }
    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    Ok(Box::new(SocketLink {
        writer: stream,
        reader,
        next_id: 1,
    }))
}

/// Says `what` on stderr: stdout is the protocol's.
fn say(what: &str) {
    let _ = writeln!(io::stderr(), "oso mcp: {what}");
}

/// Runs `oso mcp` if the command line asks for it, and returns its exit status. `core_dumps`
/// is why turning core dumps off failed, if it did: the bridge holds text it reads.
pub fn main_if_requested(core_dumps: Option<String>) -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|first| first != "mcp") {
        return None;
    }
    if let Some(why) = core_dumps {
        say(&format!("core dumps stay on: {why}"));
    }
    Some(run(args.collect()))
}

/// The options, and serving until stdin ends.
fn run(args: Vec<OsString>) -> i32 {
    let mut pid = None;
    #[cfg(feature = "dev")]
    let mut requests: Option<std::path::PathBuf> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--pid") => match args.next().and_then(|n| n.to_str()?.parse().ok()) {
                Some(n) => pid = Some(n),
                None => return usage("--pid needs a process ID"),
            },
            // The end-to-end flows give requests in a file: a child's stdin is closed.
            #[cfg(feature = "dev")]
            Some("--requests") => match args.next() {
                Some(path) => requests = Some(path.into()),
                None => return usage("--requests needs a file"),
            },
            _ => return usage(&format!("unknown option {arg:?}")),
        }
    }
    let mut bridge = Bridge::new(Box::new(move || connect(pid)));
    #[cfg(feature = "dev")]
    if let Some(path) = requests {
        return match std::fs::File::open(&path) {
            Ok(file) => serve(
                &mut bridge,
                Box::new(BufReader::new(file)),
                false,
                io::stdout().lock(),
            ),
            Err(e) => usage(&format!("cannot read {}: {e}", path.display())),
        };
    }
    serve(
        &mut bridge,
        Box::new(BufReader::new(io::stdin())),
        true,
        io::stdout().lock(),
    )
}

fn usage(why: &str) -> i32 {
    say(&format!("{why}\nusage: oso mcp [--pid PID]"));
    2
}

/// What the stdin thread reads.
enum Input {
    Line(String),
    TooLong,
    End,
}

/// Whether `line` is the agent cancelling a call, which closes the one that waits.
fn cancels(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .is_ok_and(|request| request["method"] == "notifications/cancelled")
}

/// Reads `input` on a thread of its own, line by line, and closes a waiting call's connection
/// when the agent cancels it, or, if `end_is_gone`, when `input` ends: the agent's stdin ends
/// when it goes, where a file of requests ends at once.
fn spawn_reader(
    mut input: Box<dyn BufRead + Send>,
    interrupt: Arc<Interrupt>,
    end_is_gone: bool,
) -> io::Result<mpsc::Receiver<Input>> {
    let (lines, received) = mpsc::channel();
    thread::Builder::new()
        .name("oso-mcp-stdin".into())
        .spawn(move || {
            let mut line = Vec::new();
            loop {
                line.clear();
                let read = match read_bounded_line(&mut input, &mut line) {
                    Ok(0) | Err(_) => Input::End,
                    Ok(_) if line.len() > MAX_LINE && line.last() != Some(&b'\n') => Input::TooLong,
                    Ok(_) => Input::Line(String::from_utf8_lossy(&line).trim().to_owned()),
                };
                let ends = !matches!(read, Input::Line(_));
                if ends && end_is_gone {
                    interrupt.end();
                } else if matches!(&read, Input::Line(text) if cancels(text)) {
                    interrupt.fire();
                }
                if lines.send(read).is_err() || ends {
                    return;
                }
            }
        })?;
    Ok(received)
}

/// Answers each request line of `input` on `output`, until `input` ends.
fn serve<W: Write>(
    bridge: &mut Bridge,
    input: Box<dyn BufRead + Send>,
    end_is_gone: bool,
    mut output: W,
) -> i32 {
    let lines = match spawn_reader(input, Arc::clone(&bridge.interrupt), end_is_gone) {
        Ok(lines) => lines,
        Err(e) => {
            say(&format!("cannot start reading stdin: {e}"));
            return 1;
        }
    };
    loop {
        let text = match lines.recv() {
            Ok(Input::Line(text)) => text,
            Ok(Input::TooLong) => {
                let message = format!("a request line is longer than {MAX_LINE} bytes");
                let _ = writeln!(output, "{}", error(&Value::Null, -32700, &message));
                return 1;
            }
            Ok(Input::End) | Err(_) => return 0,
        };
        if text.is_empty() {
            continue;
        }
        if let Some(response) = bridge.respond(&text)
            && writeln!(output, "{response}")
                .and_then(|()| output.flush())
                .is_err()
        {
            return 0;
        }
    }
}

mod tools;

use tools::{channel_request, initialize, tool_result, tools};

#[cfg(test)]
mod tests;
