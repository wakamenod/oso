//! `oso mcp`'s tools (ADR 0085): what each takes, whether it only reads, and the channel's
//! method each becomes; and what `initialize` says.
//!
//! Must not depend on: the socket or stdio; these only describe and map.

use serde_json::{Map, Value, json};

use super::super::wire::short;
use super::{INSTRUCTIONS, MCP_VERSIONS};

/// The channel's method and parameters for the tool `name`.
pub(super) fn channel_request(
    name: &str,
    arguments: &Map<String, Value>,
) -> Result<(&'static str, Value), String> {
    let only = |names: &[&str]| match arguments.keys().find(|key| !names.contains(&key.as_str())) {
        Some(other) => Err(format!(
            "{} takes no argument {:?}",
            short(name),
            short(other)
        )),
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
pub(super) fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// The result of `initialize`: the version asked for if this bridge speaks it, else its
/// newest.
pub(super) fn initialize(params: &Map<String, Value>) -> Value {
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
pub(super) fn tools() -> Value {
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
            "description": "A count or a number for a command that takes one, text for one that reads text, or the name of a buffer `state` lists for `switch-to-buffer`.",
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
