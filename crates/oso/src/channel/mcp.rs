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
//! - Tools: `state`, `text`, `diagnostics` (ADR 0138, step 11), `commands`, `look`, `change`,
//!   `open` and `cancel` (ADR 0093), and with `--permission`, which only a native session's
//!   MCP configuration gives, `permission` (ADR 0165, step 2): the session's
//!   `--permission-prompt-tool`, asked on the sessions' socket, where the person is never asked
//!   about the connection; it connects there as `initialize` comes.
//!   Looking and changing are
//!   separate tools, so that the agent's own permission rules can allow one and ask for the
//!   other.
//! - Once the person agreed that this bridge's session moves into a space, the bridge stops
//!   its own parent after that session's turn has ended (`moving`, ADR 0151, step 2).

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::time::Duration;

use oso_platform::control::Socket;

mod follows;
use follows::follower;
pub use follows::write_follows;
use serde_json::{Map, Value, json};

use super::protocol::PROTOCOL_VERSION;
use super::wire::{MAX_ID, MAX_LINE, escape_unsafe, short};

/// The protocol versions of MCP this bridge speaks, the newest first. 2025-03-26 is not one:
/// it requires taking batches, which this bridge does not.
const MCP_VERSIONS: [&str; 2] = ["2025-06-18", "2024-11-05"];

/// How long the bridge waits before it looks through every instance's socket again.
const PROBE_EVERY: Duration = Duration::from_secs(5);

/// What the bridge tells the agent about itself when it starts.
const INSTRUCTIONS: &str = "Drives the person's oso editor through its agent channel. The person turns the channel on with M-x agent-channel-start and lets each connection in; until then, tool calls fail and say so. A refusal that says the person is busy means: wait, and try again. A change that waits for the person can be taken back with cancel. Commands that need a person at the keyboard are never run. Inside a session oso hosts, the session tools start agent sessions and worktrees and remove finished ones: start parallel work with start_worktree_session, never with EnterWorktree, git worktree add or claude in a shell. A tool the person refused, or that oso refused, is a decision, not an obstacle: do not reach the same effect another way, such as git worktree add or remove --force, claude in a shell, or git push; tell the person instead.";

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

/// Which of the bridge's connections a call goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    /// The agent channel's.
    Channel,
    /// The sessions' socket's, for `permission` only (ADR 0165, step 2).
    SessionSocket,
}

/// How to connect to one socket, and the connection once made.
struct Line {
    connect: Connect,
    link: Option<Box<dyn Link>>,
}

impl Line {
    fn new(connect: Connect) -> Self {
        Self {
            connect,
            link: None,
        }
    }
}

/// The bridge: what it has connected to, and how to connect.
struct Bridge {
    /// The agent channel's line.
    channel: Line,
    /// The sessions' socket's, once `--permission` gave the bridge its `permission` tool (ADR
    /// 0165, step 2). A call for it never goes on the channel's.
    permission: Option<Line>,
    interrupt: Arc<Interrupt>,
    /// The hosted session this bridge says it serves, from `OSO_SESSION`, which oso believes
    /// only if it started this bridge's parent as that session (ADR 0121).
    session: Option<String>,
    /// The request whose move into a space the person agreed to, once its response is out
    /// (ADR 0151, step 2).
    moved: Option<Value>,
}

/// The variable that names a hosted session to its bridge (ADR 0121, "The bridge and the
/// channel"): oso sets it in the session's environment and in the MCP server's own.
pub(crate) const SESSION_VARIABLE: &str = "OSO_SESSION";

impl Bridge {
    /// A bridge that connects with `connect` on its first tool call.
    fn new(connect: Connect) -> Self {
        Self {
            channel: Line::new(connect),
            permission: None,
            interrupt: Arc::default(),
            session: None,
            moved: None,
        }
    }

    /// The bridge has the `permission` tool, which asks on a connection `connect` makes.
    fn with_permission(mut self, connect: Connect) -> Self {
        self.permission = Some(Line::new(connect));
        self
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
            "initialize" => {
                // Before the session's first turn, so that the session's slot is its bridge's
                // before any command the session runs could claim it (ADR 0165, step 2).
                self.connect_early(Which::SessionSocket);
                Ok(initialize(params))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools(self.permission.is_some()) })),
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
        if name == permission::TOOL && self.permission.is_some() {
            return Ok(self.ask_permission(id, arguments));
        }
        let (method, params) = channel_request(name, arguments).map_err(|why| (-32602, why))?;
        Ok(match self.call(id, Which::Channel, method, params) {
            Ok(result) => {
                if moving::agreed(name, &result) {
                    self.moved = Some(id.clone());
                }
                Some(tool_result(&result.to_string(), false))
            }
            Err(Failed::Cancelled) => None,
            Err(failed) => Some(tool_result(&failed.message(), true)),
        })
    }

    /// Calls the channel for the request `id`, connecting first if there is no connection,
    /// while the stdin thread may close it.
    fn call(
        &mut self,
        id: &Value,
        which: Which,
        method: &str,
        params: Value,
    ) -> Result<Value, Failed> {
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
        let result = self.call_waiting(which, method, params);
        let cut = {
            let mut calls = self.interrupt.lock();
            calls.in_flight = None;
            std::mem::take(&mut calls.cut)
        };
        // A cancel that lands as the call ends may have closed the connection all the same.
        if (cut || matches!(result, Err(Failed::Broken(_))))
            && let Some(line) = self.line(which)
        {
            line.link = None;
        }
        self.interrupt.lock().close = None;
        if cut {
            return Err(Failed::Cancelled);
        }
        result
    }

    /// Calls `method` on `which`'s line, connecting first if it has no connection. What closes
    /// it is the call's, so that a cancel closes the connection the call waits on.
    fn call_waiting(&mut self, which: Which, method: &str, params: Value) -> Result<Value, Failed> {
        let interrupt = Arc::clone(&self.interrupt);
        self.link_up(which, true)?;
        let link = (self.line(which).and_then(|line| line.link.as_mut()))
            .ok_or_else(|| Failed::Broken("no connection".into()))?;
        {
            let mut calls = interrupt.lock();
            // The agent may have gone, or cancelled the call, while it connected: nothing
            // would close a call that started waiting now.
            if calls.gone || calls.cut {
                return Err(Failed::Broken("the call ended while it connected".into()));
            }
            calls.close = link.closer();
        }
        link.call(method, params)
    }

    /// Connects `which`'s line now, if it has none, as `initialize` does: what fails is said on
    /// stderr, and the first call tries again.
    fn connect_early(&mut self, which: Which) {
        if self.line(which).is_none() {
            return;
        }
        if let Err(failed) = self.link_up(which, false) {
            say(&format!("connecting early: {}", failed.message()));
        }
    }

    /// Gives `which`'s line a connection, if it has none: a new one asks `version` first, naming
    /// the bridge's session, and is dropped if it is not this bridge's. `in_call` if a call
    /// waits on it, which the agent may cancel meanwhile.
    fn link_up(&mut self, which: Which, in_call: bool) -> Result<(), Failed> {
        let session = self.session.clone();
        let interrupt = Arc::clone(&self.interrupt);
        let line = self
            .line(which)
            .ok_or_else(|| Failed::Broken("this bridge asks no permission".into()))?;
        if line.link.is_some() {
            return Ok(());
        }
        let mut link = (line.connect)().map_err(Failed::Broken)?;
        if in_call {
            let mut calls = interrupt.lock();
            if calls.gone || calls.cut {
                return Err(Failed::Broken("the call ended while it connected".into()));
            }
            calls.close = link.closer();
        }
        let params = match &session {
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
        line.link = Some(link);
        Ok(())
    }

    /// `which`'s line: the sessions' socket's only for a bridge with `permission`.
    fn line(&mut self, which: Which) -> Option<&mut Line> {
        match which {
            Which::Channel => Some(&mut self.channel),
            Which::SessionSocket => self.permission.as_mut(),
        }
    }
}

/// A JSON-RPC error response line.
fn error(id: &Value, code: i64, message: &str) -> String {
    escape_unsafe(
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            .to_string(),
    )
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
    let mut asks = false;
    #[cfg(feature = "dev")]
    let mut requests: Option<std::path::PathBuf> = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--pid") => match args.next().and_then(|n| n.to_str()?.parse().ok()) {
                Some(n) => pid = Some(n),
                None => return usage("--pid needs a process ID"),
            },
            // A native session's bridge, whose `claude` asks the person through it.
            Some("--permission") => asks = true,
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
    let mut bridge =
        Bridge::new(Box::new(move || connect(&mut target, Socket::Agent))).for_session(session);
    // Only the instance `--pid` names hosts the session the bridge asks for.
    if asks {
        let Some(pid) = pid else {
            return usage("--permission needs --pid");
        };
        let mut target = Target::new(Some(pid));
        bridge = bridge.with_permission(Box::new(move || connect(&mut target, Socket::Session)));
    }
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
    say(&format!("{why}\nusage: oso mcp [--pid PID [--permission]]"));
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
        // The agent has the reply that tells it to end its turn (ADR 0151, step 2).
        if let Some(id) = bridge.moved.take() {
            bridge.stop_parent_once_idle(&id);
        }
    }
}

mod connecting;
mod moving;
mod permission;
#[cfg(test)]
pub(crate) use permission::TOOL as PERMISSION_TOOL;
mod session_tools;
mod stdin;
mod tools;

/// The tools `tools/list` gives, for the sessions module's test of their hints (ADR 0121).
#[cfg(test)]
pub(crate) fn listed_tools() -> Value {
    tools::tools(true)
}

use connecting::{Target, connect};
use stdin::{Input, Interrupt, QUEUED, is_id, spawn_reader};
use tools::{channel_request, initialize, tool_result, tools};

#[cfg(test)]
mod tests;
