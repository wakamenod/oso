//! `oso mcp` (ADR 0031, ADR 0085): the stdio bridge an agent such as Claude Code starts, which
//! speaks the Model Context Protocol's JSON-RPC on its stdin and stdout and hands each tool
//! call to the agent channel of the oso instance whose channel is on.
//!
//! Must not depend on: `app`, `dispatch` or the editor; the bridge is a client of the channel,
//! like any other, and the person lets each of its connections in.
//!
//! - One request per line of stdin, at most [`MAX_LINE`] bytes, and one response per line of
//!   stdout, escaped as the channel's are. It logs to stderr only.
//! - It connects on the first tool call, asks the channel's `version` first, and refuses one
//!   it does not know. A connection that breaks, as when oso restarts, is made again on the
//!   next call, which asks the person again.
//! - Tools: `state`, `text`, `commands`, `look`, `change` and `open`. Looking and changing are
//!   separate tools, so that the agent's own permission rules can allow one and ask for the
//!   other.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};

use oso_platform::control::{ControlDir, ControlStream, Socket};
use serde_json::{Map, Value, json};

use super::protocol::PROTOCOL_VERSION;
use super::wire::{MAX_LINE, escape_unsafe};

/// The protocol versions of MCP this bridge speaks, the newest first.
const MCP_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// What the bridge tells the agent about itself when it starts.
const INSTRUCTIONS: &str = "Drives the person's oso editor through its agent channel. The person turns the channel on with M-x agent-channel-start and lets each connection in; until then, tool calls fail and say so. A refusal that says the person is busy means: wait, and try again. Commands that need a person at the keyboard are never run.";

/// A connection to the channel: a request and its result, or its error message.
pub trait Link {
    /// Sends `method` with `params`, and waits for the reply.
    ///
    /// # Errors
    ///
    /// The channel's error message, or what broke the connection, as [`Broken`].
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed>;
}

/// Why a call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failed {
    /// The channel answered with an error.
    Refused(String),
    /// The connection broke, or there is none: the next call connects again.
    Broken(String),
}

/// The bridge: what it has connected to, and how to connect.
pub struct Bridge<C> {
    connect: C,
    link: Option<Box<dyn Link>>,
}

impl<C> Bridge<C>
where
    C: FnMut() -> Result<Box<dyn Link>, String>,
{
    /// A bridge that connects with `connect` on its first tool call.
    pub fn new(connect: C) -> Self {
        Self {
            connect,
            link: None,
        }
    }

    /// The response line to one request line, or `None` for a notification.
    pub fn handle(&mut self, line: &str) -> Option<String> {
        let request: Value = match serde_json::from_str(line) {
            Ok(request) => request,
            Err(e) => return Some(error(&Value::Null, -32700, &format!("parse error: {e}"))),
        };
        let Some(request) = request.as_object() else {
            return Some(error(&Value::Null, -32600, "a request is a JSON object"));
        };
        let id = request.get("id").cloned();
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            return Some(error(
                &id.unwrap_or(Value::Null),
                -32600,
                "a request needs a method",
            ));
        };
        // A notification gets no response.
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
            Err(Failed::Refused(why) | Failed::Broken(why)) => tool_result(&why, true),
        })
    }

    /// Calls the channel, connecting first if there is no connection.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        if self.link.is_none() {
            self.link = Some((self.connect)().map_err(Failed::Broken)?);
        }
        let link = self
            .link
            .as_mut()
            .ok_or_else(|| Failed::Broken("no connection".into()))?;
        let result = link.call(method, params);
        if matches!(result, Err(Failed::Broken(_))) {
            self.link = None;
        }
        result
    }
}

/// The channel's method and parameters for the tool `name`.
fn channel_request(
    name: &str,
    arguments: &Map<String, Value>,
) -> Result<(&'static str, Value), String> {
    let only = |names: &[&str]| match arguments.keys().find(|key| !names.contains(&key.as_str())) {
        Some(other) => Err(format!("{name} takes no argument {:?}", short(other))),
        None => Ok(Value::Object(arguments.clone())),
    };
    let run = |reach: &str| {
        only(&["name", "arg"])?;
        let mut params = arguments.clone();
        params.insert("reach".into(), json!(reach));
        Ok::<Value, String>(Value::Object(params))
    };
    Ok(match name {
        "state" => ("state", only(&["buffer_offset"])?),
        "text" => ("text", only(&["buffer", "start_row", "rows", "region"])?),
        "commands" => ("commands", only(&[])?),
        "look" => ("run", run("look")?),
        "change" => ("run", run("change")?),
        "open" => ("open", only(&["path", "line"])?),
        _ => return Err(format!("no tool {:?}", short(name))),
    })
}

/// A tool's result: its text, and whether it is an error the agent should read as such.
fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// The result of `initialize`: the version asked for if this bridge speaks it, else its
/// newest.
fn initialize(params: &Map<String, Value>) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| MCP_VERSIONS.contains(asked))
        .unwrap_or(MCP_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "oso", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// A tool: its name, what it does, the properties it takes, which of them it needs, and
/// whether it only reads or may destroy.
fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
    destructive: bool,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "openWorldHint": false,
        },
    })
}

/// The tools, those that only read first.
fn tools() -> Value {
    let mut tools = reading_tools();
    tools.extend(acting_tools());
    Value::Array(tools)
}

fn reading_tools() -> Vec<Value> {
    vec![
        tool(
            "state",
            "The editor's state: buffers, windows with their cursors and selections, the rows the selected window shows, the echo area, and open prompts. What a buffer that may hold secrets would show is left out.",
            json!({
                "buffer_offset": { "type": "integer", "minimum": 0, "description": "Where the list of buffers starts, for more than one state holds." },
            }),
            &[],
            true,
            false,
        ),
        tool(
            "text",
            "Whole rows of a buffer by its id, or the selected window's region. Never of a buffer that may hold secrets.",
            json!({
                "buffer": { "type": "integer", "description": "A buffer's id, from `state`." },
                "start_row": { "type": "integer", "minimum": 0 },
                "rows": { "type": "integer", "minimum": 0, "maximum": 2000 },
                "region": { "type": "boolean", "description": "true for the selected window's region, with nothing else." },
            }),
            &[],
            true,
            false,
        ),
        tool(
            "commands",
            "The commands this agent may run, with what each does, what argument it takes, and whether it looks or changes.",
            json!({}),
            &[],
            true,
            false,
        ),
    ]
}

fn acting_tools() -> Vec<Value> {
    let command = json!({
        "name": { "type": "string", "description": "A command's name, as `commands` lists it." },
        "arg": {
            "type": ["integer", "string"],
            "description": "A count or a number for a command that takes one, or text for one that reads text.",
        },
    });
    vec![
        tool(
            "look",
            "Run a command that moves, marks, shows or opens, and changes no text or file.",
            command.clone(),
            &["name"],
            false,
            false,
        ),
        tool(
            "change",
            "Run a command that edits text or saves, such as `insert` with text. Saving a file this agent opened asks the person first.",
            command,
            &["name"],
            false,
            true,
        ),
        tool(
            "open",
            "Open a file that exists, by its absolute path, at a line if given. A file that may hold secrets stays closed.",
            json!({
                "path": { "type": "string", "description": "An absolute path." },
                "line": { "type": "integer", "minimum": 1 },
            }),
            &["path"],
            false,
            false,
        ),
    ]
}

/// A JSON-RPC error response line.
fn error(id: &Value, code: i64, message: &str) -> String {
    escape_unsafe(
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            .to_string(),
    )
}

/// A name a client sent, cut short to echo.
fn short(name: &str) -> String {
    name.chars().take(64).collect()
}

/// The channel of a running oso, over its socket.
struct Channel {
    writer: ControlStream,
    reader: BufReader<ControlStream>,
    next_id: u64,
}

impl Link for Channel {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, Failed> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        let broken = |e: io::Error| Failed::Broken(format!("the connection to oso broke: {e}"));
        writeln!(self.writer, "{request}").map_err(broken)?;
        let mut line = Vec::new();
        // The person may take as long as they like to let this connection in.
        let read = (&mut self.reader)
            .take(u64::try_from(MAX_LINE + 1).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut line)
            .map_err(broken)?;
        if read == 0 {
            return Err(Failed::Broken("oso closed the connection".into()));
        }
        let line = String::from_utf8_lossy(&line);
        let mut response: Value = serde_json::from_str(&line)
            .map_err(|e| Failed::Broken(format!("oso sent what is no response: {e}")))?;
        if let Some(message) = response["error"]["message"].as_str() {
            return Err(Failed::Refused(message.to_owned()));
        }
        match response.get_mut("result") {
            Some(result) => Ok(result.take()),
            None => Err(Failed::Broken("oso sent a response with no result".into())),
        }
    }
}

/// Connects to the agent channel of the instance with process ID `pid`, or of the only one
/// whose channel is on, and checks that it speaks this bridge's protocol. The person's yes
/// to the connection is asked on this first request, and it waits for it.
fn connect(pid: Option<u32>) -> Result<Box<dyn Link>, String> {
    let dir = ControlDir::open_existing().map_err(|_| no_channel())?;
    let (_, stream) = dir.connect(Socket::Agent, pid).map_err(|e| {
        format!("{e}; the person turns the channel on with M-x agent-channel-start")
    })?;
    let reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut channel = Channel {
        writer: stream,
        reader,
        next_id: 1,
    };
    let version = channel
        .call("version", json!({}))
        .map_err(|failed| match failed {
            Failed::Refused(why) | Failed::Broken(why) => why,
        })?;
    if version["protocol"] != PROTOCOL_VERSION {
        return Err(format!(
            "this oso speaks the channel's protocol {}, and this oso mcp {PROTOCOL_VERSION}",
            version["protocol"]
        ));
    }
    Ok(Box::new(channel))
}

fn no_channel() -> String {
    "no oso instance has the agent channel on; the person turns it on with M-x agent-channel-start"
        .to_owned()
}

/// Runs `oso mcp` if the command line asks for it, and returns its exit status.
pub fn main_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|first| first != "mcp") {
        return None;
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
            _ => return usage(&format!("unknown option {}", arg.to_string_lossy())),
        }
    }
    let mut bridge = Bridge::new(move || connect(pid));
    #[cfg(feature = "dev")]
    if let Some(path) = requests {
        return match std::fs::File::open(&path) {
            Ok(file) => serve(&mut bridge, BufReader::new(file), io::stdout().lock()),
            Err(e) => usage(&format!("cannot read {}: {e}", path.display())),
        };
    }
    serve(&mut bridge, io::stdin().lock(), io::stdout().lock())
}

fn usage(why: &str) -> i32 {
    say(&format!("{why}\nusage: oso mcp [--pid PID]"));
    2
}

/// Says `what` on stderr: stdout is the protocol's.
fn say(what: &str) {
    let _ = writeln!(io::stderr(), "oso mcp: {what}");
}

/// Answers each request line of `input` on `output`, until `input` ends.
pub fn serve<C, R, W>(bridge: &mut Bridge<C>, mut input: R, mut output: W) -> i32
where
    C: FnMut() -> Result<Box<dyn Link>, String>,
    R: BufRead,
    W: Write,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut input)
            .take(u64::try_from(MAX_LINE + 1).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut line);
        match read {
            Ok(0) => return 0,
            Ok(_) if line.len() > MAX_LINE && line.last() != Some(&b'\n') => {
                let message = format!("a request line is longer than {MAX_LINE} bytes");
                let _ = writeln!(output, "{}", error(&Value::Null, -32700, &message));
                return 1;
            }
            Ok(_) => {}
            Err(e) => {
                say(&format!("cannot read stdin: {e}"));
                return 1;
            }
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(response) = bridge.handle(text)
            && writeln!(output, "{response}")
                .and_then(|()| output.flush())
                .is_err()
        {
            return 0;
        }
    }
}

#[cfg(test)]
mod tests;
