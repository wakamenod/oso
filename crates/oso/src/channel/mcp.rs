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
//!   channel's `version` first, with the session `OSO_SESSION` names, if it names one
//!   (ADR 0121). A connection that breaks, as when oso restarts, is made again
//!   on the next call, which asks the person again. It looks through every instance's socket
//!   only while it knows none to connect to, and at most once each [`PROBE_EVERY`]: each look
//!   counts toward each instance's connections a minute.
//! - stdin is read on a thread of its own ([`stdin`]): when it ends, or the agent cancels the
//!   call that waits, the connection closes, so that the person's question closes and nothing
//!   the agent gave up on runs later.
//! - JSON-RPC as MCP uses it: ids are strings or numbers, params are objects, there are no
//!   batches, and responses the agent sends are ignored.
//! - Tools: `state`, `text`, `commands`, `look`, `change`, `open` and `cancel` (ADR 0093).
//!   Looking and changing are
//!   separate tools, so that the agent's own permission rules can allow one and ask for the
//!   other.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Map, Value, json};

use super::protocol::PROTOCOL_VERSION;
use super::wire::{MAX_ID, MAX_LINE, escape_unsafe, parse_response, read_bounded_line, short};

/// The protocol versions of MCP this bridge speaks, the newest first. 2025-03-26 is not one:
/// it requires taking batches, which this bridge does not.
const MCP_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];

/// How long the bridge waits before it looks through every instance's socket again.
const PROBE_EVERY: Duration = Duration::from_secs(5);

/// What the bridge tells the agent about itself when it starts.
const INSTRUCTIONS: &str = "Drives the person's oso editor through its agent channel. The person turns the channel on with M-x agent-channel-start and lets each connection in; until then, tool calls fail and say so. A refusal that says the person is busy means: wait, and try again. A change that waits for the person can be taken back with cancel. Commands that need a person at the keyboard are never run. Inside a session oso hosts, the session tools start agent sessions and worktrees and remove finished ones: start parallel work with start_worktree_session, never with EnterWorktree, git worktree add or claude in a shell. A tool the person refused, or that oso refused, is a decision, not an obstacle: do not reach the same effect another way, such as git worktree add or remove --force, claude in a shell, or git push; tell the person instead. After 20 such calls the tools wait until the person types in your session.";

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
    /// The agent cancelled the call: it gets no response, and the next call connects again.
    Cancelled,
}

impl Failed {
    fn message(self) -> String {
        match self {
            Self::Refused(why) | Self::Broken(why) => why,
            Self::Cancelled => "the agent cancelled the call".into(),
        }
    }
}

/// How the bridge connects.
type Connect = Box<dyn FnMut() -> Result<Box<dyn Link>, String>>;

/// The bridge: what it has connected to, and how to connect.
struct Bridge {
    connect: Connect,
    link: Option<Box<dyn Link>>,
    interrupt: Arc<Interrupt>,
    /// The hosted session this bridge says it serves, from `OSO_SESSION`, which oso believes
    /// only if it started this bridge's parent as that session (ADR 0121).
    session: Option<String>,
}

/// The variable that names a hosted session to its bridge (ADR 0121, "The bridge and the
/// channel"): oso sets it in the session's environment and in the MCP server's own.
pub(crate) const SESSION_VARIABLE: &str = "OSO_SESSION";

impl Bridge {
    /// A bridge that connects with `connect` on its first tool call.
    fn new(connect: Connect) -> Self {
        Self {
            connect,
            link: None,
            interrupt: Arc::default(),
            session: None,
        }
    }

    /// The bridge names `session` when it connects.
    fn for_session(mut self, session: Option<String>) -> Self {
        self.session = session;
        self
    }

    /// The response line to one request line, or `None` for a notification, a response the
    /// agent sent, or a request the agent cancelled.
    fn respond(&mut self, line: &str) -> Option<String> {
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(e) => return Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if request.is_array() {
            return Some(error(&Value::Null, -32600, "batches are not supported"));
        }
        let Some(request) = request.as_object() else {
            return Some(error(&Value::Null, -32600, "a request is a JSON object"));
        };
        let Some(method) = request.get("method") else {
            // A response to a request of ours: the bridge sends none, so it answers nothing.
            if request.contains_key("result") || request.contains_key("error") {
                return None;
            }
            let id = request
                .get("id")
                .filter(|id| is_id(id))
                .unwrap_or(&Value::Null);
            return Some(error(id, -32600, "a request needs a method"));
        };
        let id = request.get("id");
        if let Some(id) = id
            && !is_id(id)
        {
            let message = format!("an id is a number, or a string of at most {MAX_ID} bytes");
            return Some(error(&Value::Null, -32600, &message));
        }
        let Some(method) = method.as_str() else {
            return Some(error(
                id.unwrap_or(&Value::Null),
                -32600,
                "a method is a string",
            ));
        };
        // A notification gets no response, and runs nothing.
        let id = id?.clone();
        if Interrupt::take_cancelled(&mut self.interrupt.lock(), &id) {
            return None;
        }
        let empty = Map::new();
        let params = match request.get("params") {
            None => &empty,
            Some(Value::Object(params)) => params,
            Some(_) => return Some(error(&id, -32602, "params are an object")),
        };
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => match self.call_tool(&id, params) {
                Ok(None) => return None,
                Ok(Some(result)) => Ok(result),
                Err(failed) => Err(failed),
            },
            _ => Err((-32601, format!("no method {:?}", short(method)))),
        };
        Some(match result {
            Ok(result) => {
                escape_unsafe(json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string())
            }
            Err((code, message)) => error(&id, code, &message),
        })
    }

    /// Runs one tool, as the channel's method its arguments say. `None` if the agent cancelled
    /// it.
    fn call_tool(
        &mut self,
        id: &Value,
        params: &Map<String, Value>,
    ) -> Result<Option<Value>, (i64, String)> {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return Err((-32602, "tools/call needs a tool's \"name\"".into()));
        };
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None => &empty,
            Some(Value::Object(arguments)) => arguments,
            Some(_) => return Err((-32602, "a tool's arguments are an object".into())),
        };
        let (method, params) = channel_request(name, arguments).map_err(|why| (-32602, why))?;
        Ok(match self.call(id, method, params) {
            Ok(result) => Some(tool_result(&result.to_string(), false)),
            Err(Failed::Cancelled) => None,
            Err(failed) => Some(tool_result(&failed.message(), true)),
        })
    }

    /// Calls the channel for the request `id`, connecting first if there is no connection,
    /// while the stdin thread may close it.
    fn call(&mut self, id: &Value, method: &str, params: Value) -> Result<Value, Failed> {
        {
            let mut calls = self.interrupt.lock();
            if calls.gone {
                return Err(Failed::Broken("the agent has gone".into()));
            }
            // Checked with the call set in flight, so a cancel lands on one or the other.
            if Interrupt::take_cancelled(&mut calls, id) {
                return Err(Failed::Cancelled);
            }
            calls.in_flight = Some(id.clone());
            calls.cut = false;
        }
        let result = self.call_waiting(method, params);
        let cut = {
            let mut calls = self.interrupt.lock();
            calls.in_flight = None;
            std::mem::take(&mut calls.cut)
        };
        // A cancel that lands as the call ends may have closed the connection all the same.
        if cut || matches!(result, Err(Failed::Broken(_))) {
            self.link = None;
            self.interrupt.lock().close = None;
        }
        if cut {
            return Err(Failed::Cancelled);
        }
        result
    }

    /// A new connection asks the channel's `version` first, and is dropped if it is not this
    /// bridge's.
    fn call_waiting(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        if self.link.is_none() {
            let mut link = (self.connect)().map_err(Failed::Broken)?;
            {
                let mut calls = self.interrupt.lock();
                // The agent may have gone, or cancelled the call, while it connected: nothing
                // would close a call that started waiting now.
                if calls.gone || calls.cut {
                    return Err(Failed::Broken("the call ended while it connected".into()));
                }
                calls.close = link.closer();
            }
            let params = match &self.session {
                Some(session) => json!({ "session": session }),
                None => json!({}),
            };
            let version = link.call("version", params)?;
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

/// Which instance the bridge connects to, and when it last looked through them all.
struct Target {
    /// The instance `--pid` named, or the one connected to last.
    pid: Option<u32>,
    /// Whether `pid` came from `--pid`: the bridge then never looks for another.
    given: bool,
    /// When the bridge last looked through every instance's socket and found none to use, and
    /// what it told the agent.
    probed: Option<(Instant, String)>,
}

impl Target {
    fn new(pid: Option<u32>) -> Self {
        Self {
            pid,
            given: pid.is_some(),
            probed: None,
        }
    }

    /// Connects with `connect` to the instance it knows, or else to the only one whose channel
    /// is on. It looks for that one at most once each [`PROBE_EVERY`], and in between tells
    /// the agent what it found last. What goes wrong is said in full on stderr, and to the
    /// agent in fixed words.
    fn connect<S>(
        &mut self,
        now: Instant,
        mut connect: impl FnMut(Option<u32>) -> io::Result<(u32, S)>,
    ) -> Result<(u32, S), String> {
        if let Some(pid) = self.pid {
            match connect(Some(pid)) {
                Ok(found) => return Ok(found),
                Err(e) => {
                    say(&format!("connecting to pid {pid}: {e}"));
                    if self.given {
                        return Err(told(&e, true));
                    }
                    // It has gone, as when oso restarts.
                    self.pid = None;
                }
            }
        }
        if let Some((at, told)) = &self.probed
            && now.saturating_duration_since(*at) < PROBE_EVERY
        {
            return Err(told.clone());
        }
        match connect(None) {
            Ok((pid, stream)) => {
                self.pid = Some(pid);
                self.probed = None;
                Ok((pid, stream))
            }
            Err(e) => {
                say(&format!("connecting: {e}"));
                let told = told(&e, false);
                self.probed = Some((now, told.clone()));
                Err(told)
            }
        }
    }
}

/// What the agent is told when connecting fails with `e`; `given` if `--pid` named the
/// instance. No process IDs or paths.
fn told(e: &io::Error, given: bool) -> String {
    if e.kind() == io::ErrorKind::InvalidInput && !given {
        "several oso instances have the agent channel on; the person starts oso mcp with --pid to choose one".into()
    } else if given {
        "the oso instance chosen with --pid does not have the agent channel on; the person turns it on with M-x agent-channel-start".into()
    } else {
        "no oso instance has the agent channel on; the person turns it on with M-x agent-channel-start".into()
    }
}

/// Connects to the agent channel of the instance `target` says. Only an oso that runs this
/// program, and is the process that owns the socket, is spoken to: another process of the
/// same user could listen in its place.
fn connect(target: &mut Target) -> Result<Box<dyn Link>, String> {
    let (found, stream) = target.connect(Instant::now(), |pid| {
        ControlDir::open_existing()?.connect(Socket::Agent, pid)
    })?;
    let peer = stream.peer();
    let is_oso = peer.runs_this_program
        && peer
            .process
            .as_ref()
            .is_some_and(|process| process.pid() == found);
    if !is_oso {
        say(&format!(
            "the agent socket of pid {found} is served by a process that is not this oso"
        ));
        if !target.given {
            target.pid = None;
        }
        return Err(
            "the agent channel's socket is served by something other than oso; ask the person"
                .into(),
        );
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
    let mut target = Target::new(pid);
    // A session's id is short ASCII; anything else names none.
    let session = std::env::var(SESSION_VARIABLE)
        .ok()
        .filter(|id| id.len() <= super::protocol::MAX_SESSION);
    let mut bridge = Bridge::new(Box::new(move || connect(&mut target))).for_session(session);
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
        let response = match lines.recv() {
            Ok(Input::Line(text)) if text.is_empty() => continue,
            Ok(Input::Line(text)) => bridge.respond(&text),
            Ok(Input::TooLong) => {
                let message = format!("a request line is longer than {MAX_LINE} bytes");
                Some(error(&Value::Null, -32700, &message))
            }
            Ok(Input::End) | Err(_) => return 0,
        };
        let dropped = bridge.interrupt.take_dropped().into_iter().map(|id| {
            let message =
                format!("{QUEUED} requests wait already; send this again once one is answered");
            error(&id, -32000, &message)
        });
        for response in response.into_iter().chain(dropped) {
            if writeln!(output, "{response}")
                .and_then(|()| output.flush())
                .is_err()
            {
                return 0;
            }
        }
    }
}

mod session_tools;
mod stdin;
mod tools;

/// The tools `tools/list` gives, for the sessions module's test of their hints (ADR 0121).
#[cfg(test)]
pub(crate) fn listed_tools() -> Value {
    tools::tools()
}

use stdin::{Input, Interrupt, QUEUED, is_id, spawn_reader};
use tools::{channel_request, initialize, tool_result, tools};

#[cfg(test)]
mod tests;
