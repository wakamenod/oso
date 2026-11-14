//! The dev control socket's methods (ADR 0023), over the JSON Lines framing it shares with
//! the agent channel (`channel::wire`).
//!
//! Must not depend on: sockets, threads or editor state; it only parses and
//! formats messages.
//!
//! - Request: `{"id": 1, "method": "send-keys", "params": {"keys": "C-x"}}`.

use std::ops::Range;
use std::path::PathBuf;

use oso_core::key::{KeyPress, KeySequence};
use oso_platform::window::{MouseAction, MouseButton, Position, Wheel};
use serde_json::{Map, Value};

use crate::channel::wire::short;
pub use crate::channel::wire::{
    BadRequest, MAX_LINE, escaped_len, failure, request, response_result, success,
};

use super::handoff::carry::CarriedPiece;
use super::handoff::{self, State};

/// The protocol version reported by `version`. Bump it on incompatible
/// changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// The most keys one `send-keys` request may send.
pub const MAX_KEYS: usize = 4096;

/// The longest command name `run-command` accepts, in bytes. Names are
/// echoed in errors, so this keeps responses small.
const MAX_COMMAND_NAME: usize = 256;

/// The longest preedit `ime` takes, in bytes. An input method's is a phrase or a
/// sentence, and each one is segmented once when it comes (ADR 0069).
const MAX_PREEDIT: usize = 64 * 1024;

/// The largest delta `wheel` takes on either axis, in notches or pixels, far beyond any one
/// event of a real wheel or trackpad.
const MAX_WHEEL: f64 = 1e6;

/// The largest count `run-command` passes on, in either direction, as the agent channel's.
const MAX_COMMAND_ARG: isize = oso_core::command::MAX_REMOTE_COUNT;

/// How the message of an `exit` refused for its generation starts, which tells
/// a refusal, after which the instance surely runs on, from a request that went
/// unanswered, after which it may yet exit.
pub const EXIT_REFUSED: &str = "exit refused";

/// The longest message `dev-message` takes.
pub const MAX_DEV_MESSAGE: usize = 1024;

/// The UI's answer to a request: a JSON result or an error message.
pub type Reply = Result<Value, String>;

/// A method and its parsed parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Method {
    /// The protocol version and build information, which needs no UI.
    Version,
    /// Work for the UI thread.
    Ui(UiOp),
}

/// A method the UI thread answers.
#[derive(Debug, Clone, PartialEq)]
pub enum UiOp {
    /// Handle these key presses, in order.
    SendKeys(Vec<KeyPress>),
    /// Capture the current frame as a PNG.
    Screenshot,
    /// Report the editor state, with the selected window's visible text.
    GetState,
    /// Report whole rows of the selected window's buffer.
    GetText { start_row: usize, rows: usize },
    /// Run a registered command, with a numeric argument if it takes one.
    RunCommand { name: String, arg: Option<isize> },
    /// Show a buffer in the selected window.
    ShowBuffer(u64),
    /// Report the generation, or why this instance's text would not survive a
    /// restart.
    CheckHandover,
    /// Report what a new instance needs to take over.
    ExportState,
    /// Take over from the instance that exported this state.
    ImportState(Box<State>),
    /// Give the piece of an item of the latest export from `offset` (ADR 0090).
    ExportText {
        export: u64,
        item: usize,
        offset: usize,
    },
    /// Stage a piece of an item for the import that follows.
    ImportText(CarriedPiece),
    /// Say this in the echo area and `*Messages*`: what `oso dev` has to tell, such as
    /// build errors.
    DevMessage(String),
    /// Exit: without asking, or only if the generation is still this one.
    Exit { generation: Option<u64> },
    /// Performance samples for benchmarks (ADR 0025).
    TakeTimings,
    /// Visits a file in the selected window, as `oso FILE` does at startup,
    /// so that a benchmark can time it with the window up.
    OpenFile(PathBuf),
    /// Panic on the UI thread, outside the command boundary, with a message that quotes
    /// the selected buffer's text, for the emergency save's end-to-end flows (ADR 0062).
    Panic,
    /// End the event loop with nobody having agreed to, as the OS does.
    EndUnasked,
    /// Report this as the OS's input method would, as no flow can drive a real one
    /// (ADR 0069).
    Ime(ImeReport),
    /// Read or set the clipboard in memory, as another app would (ADR 0074).
    Clipboard(ClipboardOp),
    /// Scroll as the mouse wheel or the trackpad would, with the pointer there (ADR 0112).
    Wheel { delta: Wheel, at: Option<WheelAt> },
    /// Press or release a mouse button, or move the pointer, there, with Shift held or not
    /// (ADR 0120, step 4b-2).
    Mouse {
        action: MouseAction,
        at: WheelAt,
        shift: bool,
    },
}

/// Where `wheel` puts the pointer: a place on the surface, in physical pixels, or the middle
/// of a cell of the frame, which the app turns into one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelAt {
    Pixel(Position),
    Cell { row: usize, column: usize },
}

/// What `clipboard` does with the clipboard in memory. Its `Debug` gives lengths only.
#[derive(Clone, PartialEq, Eq)]
pub enum ClipboardOp {
    /// Report what it holds.
    Get,
    /// Put text there, marked concealed or not.
    Set { text: String, concealed: bool },
}

impl std::fmt::Debug for ClipboardOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => f.write_str("Get"),
            Self::Set { text, concealed } => f
                .debug_struct("Set")
                .field("len", &text.len())
                .field("concealed", concealed)
                .finish(),
        }
    }
}

/// What `ime` reports in place of the OS's input method. Its `Debug` gives lengths
/// only: the text is what a person would type (ADR 0028).
#[derive(Clone, PartialEq, Eq)]
pub enum ImeReport {
    /// The text being composed, and the caret or the segment being converted, in
    /// bytes. An empty text ends the composition, as `Clear` does.
    Preedit {
        text: String,
        range: Option<Range<usize>>,
    },
    /// A commit, which the OS sends after an empty preedit.
    Commit(String),
    /// The composition ends with no commit.
    Clear,
    /// The input method turned off, as when the input source changes.
    Disabled,
}

impl std::fmt::Debug for ImeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preedit { text, range } => f
                .debug_struct("Preedit")
                .field("bytes", &text.len())
                .field("range", range)
                .finish(),
            Self::Commit(text) => f
                .debug_struct("Commit")
                .field("bytes", &text.len())
                .finish(),
            Self::Clear => f.write_str("Clear"),
            Self::Disabled => f.write_str("Disabled"),
        }
    }
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Echoed in the response.
    pub id: Value,
    pub method: Method,
}

/// Parses one request line.
pub fn parse(line: &str) -> Result<Request, BadRequest> {
    let parts = crate::channel::wire::envelope(line)?;
    let bad = |id: &Value, message: String| BadRequest::new(id, message);
    let (id, method, params) = (parts.id.clone(), parts.method.as_str(), &parts.params);
    let op = match method {
        "version" => {
            return Ok(Request {
                id,
                method: Method::Version,
            });
        }
        "screenshot" => UiOp::Screenshot,
        "get-state" => UiOp::GetState,
        "take-timings" => UiOp::TakeTimings,
        "open-file" => UiOp::OpenFile(parse_path(params).map_err(|m| bad(&id, m))?),
        "send-keys" => UiOp::SendKeys(parse_keys(params).map_err(|m| bad(&id, m))?),
        "run-command" => parse_run_command(params).map_err(|m| bad(&id, m))?,
        "get-text" => UiOp::GetText {
            start_row: parse_size(params, "get-text", "start_row").map_err(|m| bad(&id, m))?,
            rows: parse_size(params, "get-text", "rows").map_err(|m| bad(&id, m))?,
        },
        "show-buffer" => UiOp::ShowBuffer(parse_buffer(params).map_err(|m| bad(&id, m))?),
        "check-handover" => UiOp::CheckHandover,
        "dev-panic" => UiOp::Panic,
        "dev-end-unasked" => UiOp::EndUnasked,
        "ime" => UiOp::Ime(parse_ime(params).map_err(|m| bad(&id, m))?),
        "clipboard" => UiOp::Clipboard(parse_clipboard(params).map_err(|m| bad(&id, m))?),
        "wheel" => parse_wheel(params).map_err(|m| bad(&id, m))?,
        "mouse" => parse_mouse(params).map_err(|m| bad(&id, m))?,
        "export-state" => UiOp::ExportState,
        "export-text" | "import-text" | "dev-message" => {
            parse_for_oso_dev(method, params).map_err(|m| bad(&id, m))?
        }
        "import-state" => {
            let state = params
                .get("state")
                .ok_or_else(|| bad(&id, "import-state needs a parameter \"state\"".into()))?;
            UiOp::ImportState(Box::new(handoff::parse(state).map_err(|m| bad(&id, m))?))
        }
        "exit" => UiOp::Exit {
            generation: parse_generation(&parts.rest, params).map_err(|m| bad(&id, m))?,
        },
        _ => return Err(parts.unknown()),
    };
    Ok(Request {
        id,
        method: Method::Ui(op),
    })
}

/// Why `method` is refused, quoting only its start.
fn unknown(method: &str) -> String {
    format!("unknown method {:?}", short(method))
}

/// The methods only `oso dev` sends: those that carry text through a restart, and
/// `dev-message` (ADR 0090).
fn parse_for_oso_dev(method: &str, params: &Map<String, Value>) -> Result<UiOp, String> {
    match method {
        "export-text" => parse_export_text(params),
        "import-text" => parse_import_text(params),
        "dev-message" => parse_dev_message(params),
        other => Err(unknown(other)),
    }
}

/// The parameters of `export-text`.
fn parse_export_text(params: &Map<String, Value>) -> Result<UiOp, String> {
    Ok(UiOp::ExportText {
        export: parse_u64(params, "export-text", "export")?,
        item: parse_size(params, "export-text", "item")?,
        offset: parse_size(params, "export-text", "offset")?,
    })
}

/// The parameters of `import-text`.
fn parse_import_text(params: &Map<String, Value>) -> Result<UiOp, String> {
    let Some(Value::String(text)) = params.get("text") else {
        return Err("import-text needs a string parameter \"text\"".into());
    };
    Ok(UiOp::ImportText(CarriedPiece {
        export: parse_u64(params, "import-text", "export")?,
        item: parse_size(params, "import-text", "item")?,
        offset: parse_size(params, "import-text", "offset")?,
        text: text.clone(),
    }))
}

/// A non-negative integer parameter `name` of `method`.
fn parse_u64(params: &Map<String, Value>, method: &str, name: &str) -> Result<u64, String> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{method} needs a non-negative integer parameter {name:?}"))
}

/// The `text` parameter of `dev-message`.
fn parse_dev_message(params: &Map<String, Value>) -> Result<UiOp, String> {
    match params.get("text") {
        Some(Value::String(text)) if text.len() <= MAX_DEV_MESSAGE => {
            Ok(UiOp::DevMessage(text.clone()))
        }
        _ => Err(format!(
            "dev-message needs a string parameter \"text\" of at most {MAX_DEV_MESSAGE} bytes"
        )),
    }
}

/// The `keys` parameter of `send-keys`, in Emacs notation.
fn parse_keys(params: &Map<String, Value>) -> Result<Vec<KeyPress>, String> {
    let Some(Value::String(keys)) = params.get("keys") else {
        return Err("send-keys needs a string parameter \"keys\"".into());
    };
    let KeySequence(keys) = keys.parse().map_err(|e| format!("{e}"))?;
    if keys.is_empty() {
        return Err("send-keys needs at least one key".into());
    }
    if keys.len() > MAX_KEYS {
        return Err(format!("send-keys takes at most {MAX_KEYS} keys"));
    }
    Ok(keys)
}

/// The `buffer` parameter of `show-buffer`.
fn parse_buffer(params: &Map<String, Value>) -> Result<u64, String> {
    params
        .get("buffer")
        .and_then(Value::as_u64)
        .ok_or_else(|| "show-buffer needs a numeric parameter \"buffer\"".to_owned())
}

/// The `path` parameter of `open-file`.
fn parse_path(params: &Map<String, Value>) -> Result<PathBuf, String> {
    match params.get("path") {
        Some(Value::String(path)) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Err("open-file needs a string parameter \"path\"".into()),
    }
}

/// The optional `generation` parameter of `exit`. Anything but a number is an
/// error, null included, and so is a misspelled parameter or one outside
/// `params`, so that a caller who lost track of its generation never gets the
/// unconditional exit by accident.
fn parse_generation(
    request: &Map<String, Value>,
    params: &Map<String, Value>,
) -> Result<Option<u64>, String> {
    if request.contains_key("generation") || params.keys().any(|name| name != "generation") {
        return Err("exit takes one parameter, \"generation\", inside \"params\"".into());
    }
    params
        .get("generation")
        .map(|generation| {
            generation
                .as_u64()
                .ok_or_else(|| "exit's \"generation\" must be a non-negative integer".to_owned())
        })
        .transpose()
}

/// The parameters of `run-command`: a string `name` and an optional
/// integer `arg`.
fn parse_run_command(params: &Map<String, Value>) -> Result<UiOp, String> {
    let Some(Value::String(name)) = params.get("name") else {
        return Err("run-command needs a string parameter \"name\"".into());
    };
    if name.len() > MAX_COMMAND_NAME {
        return Err(format!(
            "run-command takes names of at most {MAX_COMMAND_NAME} bytes"
        ));
    }
    let arg = match params.get("arg") {
        None | Some(Value::Null) => None,
        Some(arg) => Some(
            arg.as_i64()
                .and_then(|arg| isize::try_from(arg).ok())
                .filter(|arg| arg.abs_diff(0) <= MAX_COMMAND_ARG.unsigned_abs())
                .ok_or_else(|| {
                    format!(
                        "run-command's \"arg\" must be an integer of at most {MAX_COMMAND_ARG} either way"
                    )
                })?,
        ),
    };
    Ok(UiOp::RunCommand {
        name: name.clone(),
        arg,
    })
}

/// The parameters of `ime`: one of `preedit` (with an optional `range` of two byte
/// offsets), `commit`, `clear` or `disabled`. A range off the text is the editor's to
/// drop, as one from the OS would be.
/// `clipboard` with no parameters reads, and with `text`, and `concealed` if it is to
/// be marked so, sets.
fn parse_clipboard(params: &Map<String, Value>) -> Result<ClipboardOp, String> {
    let concealed = match params.get("concealed") {
        None => false,
        Some(Value::Bool(concealed)) => *concealed,
        Some(_) => return Err("clipboard's \"concealed\" must be a boolean".into()),
    };
    match params.get("text") {
        None if params.is_empty() => Ok(ClipboardOp::Get),
        Some(Value::String(text)) => Ok(ClipboardOp::Set {
            text: text.clone(),
            concealed,
        }),
        _ => Err("clipboard takes a string \"text\", and \"concealed\", or nothing".into()),
    }
}

fn parse_ime(params: &Map<String, Value>) -> Result<ImeReport, String> {
    const USAGE: &str = "ime takes one of \"preedit\" (a string, with an optional \"range\" of \
                         two byte offsets), \"commit\" (a string), \"clear\" or \"disabled\" (true)";
    let offset = |value: &Value| {
        value
            .as_u64()
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(|| USAGE.to_owned())
    };
    let range = match params.get("range") {
        None | Some(Value::Null) => None,
        Some(Value::Array(pair)) => match pair.as_slice() {
            [start, end] => Some(offset(start)?..offset(end)?),
            _ => return Err(USAGE.into()),
        },
        Some(_) => return Err(USAGE.into()),
    };
    let mut named = params.iter().filter(|(name, _)| *name != "range");
    let (Some((name, value)), None) = (named.next(), named.next()) else {
        return Err(USAGE.into());
    };
    match (name.as_str(), value) {
        ("preedit", Value::String(text)) if text.len() > MAX_PREEDIT => Err(format!(
            "ime takes a preedit of at most {MAX_PREEDIT} bytes"
        )),
        ("preedit", Value::String(text)) => Ok(ImeReport::Preedit {
            text: text.clone(),
            range,
        }),
        ("commit", Value::String(text)) if range.is_none() => Ok(ImeReport::Commit(text.clone())),
        ("clear", Value::Bool(true)) if range.is_none() => Ok(ImeReport::Clear),
        ("disabled", Value::Bool(true)) if range.is_none() => Ok(ImeReport::Disabled),
        _ => Err(USAGE.into()),
    }
}

/// The parameters of `wheel`: `lines` (a wheel's notches) or `pixels` (a trackpad's), each
/// `[x, y]` with positive moving the content down and right, and an optional `at`, either
/// `{"x", "y"}` in physical pixels or `{"row", "column"}` of a cell.
fn parse_wheel(params: &Map<String, Value>) -> Result<UiOp, String> {
    const USAGE: &str = "wheel takes one of \"lines\" or \"pixels\" as [x, y], each at most \
                         1e6 across, and an optional \"at\" of {\"x\", \"y\"} in pixels or \
                         {\"row\", \"column\"}";
    let number = |value: Option<&Value>| {
        value
            .and_then(Value::as_f64)
            .filter(|n| n.abs() <= MAX_WHEEL)
            .ok_or(USAGE)
    };
    let pair = |value: &Value| match value.as_array().map(Vec::as_slice) {
        Some([x, y]) => Ok((number(Some(x))?, number(Some(y))?)),
        _ => Err(USAGE),
    };
    let delta = match (params.get("lines"), params.get("pixels")) {
        (Some(lines), None) => {
            let (x, y) = pair(lines)?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "at most 1e6, which f32 holds"
            )]
            Wheel::Lines {
                x: x as f32,
                y: y as f32,
            }
        }
        (None, Some(pixels)) => {
            let (x, y) = pair(pixels)?;
            Wheel::Pixels { x, y }
        }
        _ => return Err(USAGE.into()),
    };
    let at = parse_at(params, number, USAGE)?;
    if params.len() != 1 + usize::from(params.contains_key("at")) {
        return Err(USAGE.into());
    }
    Ok(UiOp::Wheel { delta, at })
}

/// Where `wheel` or `mouse` puts the pointer: `params`' "at", a pixel `{"x", "y"}` or a cell
/// `{"row", "column"}`, or none; `number` reads a coordinate, and `usage` is the error.
fn parse_at(
    params: &Map<String, Value>,
    number: impl Fn(Option<&Value>) -> Result<f64, &'static str>,
    usage: &'static str,
) -> Result<Option<WheelAt>, String> {
    let at = match params.get("at") {
        None | Some(Value::Null) => None,
        Some(Value::Object(at)) => Some(match (at.len(), at.get("x"), at.get("row")) {
            (2, Some(x), None) => WheelAt::Pixel(Position {
                x: number(Some(x))?,
                y: number(at.get("y"))?,
            }),
            (2, None, Some(row)) => {
                let cell = |value: Option<&Value>| {
                    value
                        .and_then(Value::as_u64)
                        .and_then(|n| usize::try_from(n).ok())
                        .ok_or(usage)
                };
                WheelAt::Cell {
                    row: cell(Some(row))?,
                    column: cell(at.get("column"))?,
                }
            }
            _ => return Err(usage.into()),
        }),
        Some(_) => return Err(usage.into()),
    };
    Ok(at)
}

/// `mouse`: one of "press" or "release" with "left", "middle" or "right", or "move": true,
/// "at" a pixel or a cell, and an optional "shift": true (ADR 0120, step 4b-2).
fn parse_mouse(params: &Map<String, Value>) -> Result<UiOp, String> {
    const USAGE: &str = "mouse takes one of \"press\" or \"release\" with \"left\", \"middle\" \
                         or \"right\", or \"move\": true; \"at\" of {\"x\", \"y\"} in pixels or \
                         {\"row\", \"column\"}; and an optional \"shift\": true";
    let button = |value: &Value| match value.as_str() {
        Some("left") => Ok(MouseButton::Left),
        Some("middle") => Ok(MouseButton::Middle),
        Some("right") => Ok(MouseButton::Right),
        _ => Err(USAGE),
    };
    let action = match (
        params.get("press"),
        params.get("release"),
        params.get("move"),
    ) {
        (Some(pressed), None, None) => MouseAction::Press(button(pressed)?),
        (None, Some(released), None) => MouseAction::Release(button(released)?),
        (None, None, Some(Value::Bool(true))) => MouseAction::Moved,
        _ => return Err(USAGE.into()),
    };
    let number = |value: Option<&Value>| {
        value
            .and_then(Value::as_f64)
            .filter(|n| n.abs() <= MAX_WHEEL)
            .ok_or(USAGE)
    };
    let at = parse_at(params, number, USAGE)?.ok_or(USAGE)?;
    let shift = match params.get("shift") {
        None => false,
        Some(Value::Bool(shift)) => *shift,
        Some(_) => return Err(USAGE.into()),
    };
    if params.len() != 2 + usize::from(params.contains_key("shift")) {
        return Err(USAGE.into());
    }
    Ok(UiOp::Mouse { action, at, shift })
}

/// A required parameter that is a non-negative integer.
fn parse_size(params: &Map<String, Value>, method: &str, name: &str) -> Result<usize, String> {
    params
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("{method} needs a non-negative integer parameter {name:?}"))
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardOp, ImeReport, MAX_COMMAND_ARG, MAX_KEYS, MAX_LINE, Method, Request, UiOp,
        WheelAt, failure, parse, request, success,
    };
    use crate::channel::wire::MAX_ID;
    use oso_platform::window::{MouseAction, MouseButton, Position, Wheel};
    use serde_json::{Value, json};

    fn parsed(line: &str) -> Request {
        parse(line).unwrap_or_else(|e| panic!("{}", e.message))
    }

    fn error(line: &str) -> (Value, String) {
        let e = parse(line).unwrap_err();
        (e.id, e.message)
    }

    #[test]
    fn clipboard_reads_or_sets_and_its_debug_shows_no_text() {
        let op = |params: &str| match parse(&format!(
            r#"{{"method": "clipboard", "params": {params}}}"#
        ))
        .unwrap()
        .method
        {
            Method::Ui(UiOp::Clipboard(op)) => op,
            other => panic!("{other:?}"),
        };
        assert_eq!(op("{}"), ClipboardOp::Get);
        let set = op(r#"{"text": "hunter2", "concealed": true}"#);
        assert_eq!(
            set,
            ClipboardOp::Set {
                text: "hunter2".into(),
                concealed: true
            }
        );
        assert!(!format!("{set:?}").contains("hunter2"));
        assert!(parse(r#"{"method": "clipboard", "params": {"text": 1}}"#).is_err());
        assert!(parse(r#"{"method": "clipboard", "params": {"concealed": true}}"#).is_err());
    }

    #[test]
    fn parses_each_method() {
        assert_eq!(
            parsed(r#"{"id": 1, "method": "version"}"#).method,
            Method::Version
        );
        assert_eq!(
            parsed(r#"{"id": 2, "method": "screenshot", "params": {}}"#).method,
            Method::Ui(UiOp::Screenshot)
        );
        assert_eq!(
            parsed(r#"{"id": 3, "method": "get-state", "params": null}"#).method,
            Method::Ui(UiOp::GetState)
        );
        assert_eq!(
            parsed(r#"{"id": 5, "method": "take-timings"}"#).method,
            Method::Ui(UiOp::TakeTimings)
        );
        let keys = parsed(r#"{"id": 4, "method": "send-keys", "params": {"keys": "C-x <right>"}}"#);
        assert_eq!(
            keys.method,
            Method::Ui(UiOp::SendKeys(vec![
                "C-x".parse().unwrap(),
                "<right>".parse().unwrap()
            ]))
        );
    }

    #[test]
    fn ime_takes_one_report() {
        let ime = |params: &str| parse(&format!(r#"{{"method": "ime", "params": {params}}}"#));
        let report = |params: &str| match ime(params).unwrap().method {
            Method::Ui(UiOp::Ime(report)) => report,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            report(r#"{"preedit": "にほん", "range": [3, 6]}"#),
            ImeReport::Preedit {
                text: "にほん".into(),
                range: Some(3..6)
            }
        );
        assert_eq!(
            report(r#"{"preedit": "か"}"#),
            ImeReport::Preedit {
                text: "か".into(),
                range: None
            }
        );
        assert_eq!(
            report(r#"{"commit": "日本"}"#),
            ImeReport::Commit("日本".into())
        );
        assert_eq!(report(r#"{"clear": true}"#), ImeReport::Clear);
        assert_eq!(report(r#"{"disabled": true}"#), ImeReport::Disabled);
        for params in [
            r#"{}"#,
            r#"{"preedit": "a", "commit": "b"}"#,
            r#"{"preedit": 1}"#,
            r#"{"preedit": "a", "range": [1]}"#,
            r#"{"preedit": "a", "range": [-1, 2]}"#,
            r#"{"commit": "a", "range": [0, 0]}"#,
            r#"{"clear": false}"#,
            r#"{"range": [0, 0]}"#,
        ] {
            assert!(
                ime(params).unwrap_err().message.starts_with("ime takes"),
                "{params}"
            );
        }
        let long = json!({ "preedit": "a".repeat(super::MAX_PREEDIT + 1) }).to_string();
        assert!(ime(&long).unwrap_err().message.contains("at most"));
    }

    #[test]
    fn mouse_takes_an_action_a_place_and_shift() {
        let mouse = |params: &str| parse(&format!(r#"{{"method": "mouse", "params": {params}}}"#));
        let op = |params: &str| match mouse(params).unwrap().method {
            Method::Ui(UiOp::Mouse { action, at, shift }) => (action, at, shift),
            other => panic!("{other:?}"),
        };
        let cell = WheelAt::Cell { row: 2, column: 5 };
        assert_eq!(
            op(r#"{"press": "left", "at": {"row": 2, "column": 5}}"#),
            (MouseAction::Press(MouseButton::Left), cell, false)
        );
        assert_eq!(
            op(r#"{"release": "right", "at": {"row": 2, "column": 5}, "shift": true}"#),
            (MouseAction::Release(MouseButton::Right), cell, true)
        );
        assert_eq!(
            op(r#"{"move": true, "at": {"x": 1.5, "y": 2}}"#).0,
            MouseAction::Moved
        );
        for bad in [
            r#"{"press": "left"}"#,
            r#"{"press": "back", "at": {"row": 0, "column": 0}}"#,
            r#"{"press": "left", "release": "left", "at": {"row": 0, "column": 0}}"#,
            r#"{"move": false, "at": {"row": 0, "column": 0}}"#,
            r#"{"press": "left", "at": {"row": 0, "column": 0}, "shift": 1}"#,
            r#"{"press": "left", "at": {"row": 0, "column": 0}, "more": 1}"#,
        ] {
            assert!(mouse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn wheel_takes_a_delta_and_where_the_pointer_is() {
        let wheel = |params: &str| parse(&format!(r#"{{"method": "wheel", "params": {params}}}"#));
        let op = |params: &str| match wheel(params).unwrap().method {
            Method::Ui(UiOp::Wheel { delta, at }) => (delta, at),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            op(r#"{"lines": [0, -3]}"#),
            (Wheel::Lines { x: 0.0, y: -3.0 }, None)
        );
        assert_eq!(
            op(r#"{"pixels": [1.5, -40], "at": {"x": 12.5, "y": 300}}"#),
            (
                Wheel::Pixels { x: 1.5, y: -40.0 },
                Some(WheelAt::Pixel(Position { x: 12.5, y: 300.0 }))
            )
        );
        assert_eq!(
            op(r#"{"lines": [0, 1], "at": {"row": 3, "column": 0}}"#),
            (
                Wheel::Lines { x: 0.0, y: 1.0 },
                Some(WheelAt::Cell { row: 3, column: 0 })
            )
        );
        assert_eq!(op(r#"{"lines": [0, 1], "at": null}"#).1, None);
        for params in [
            r#"{}"#,
            r#"{"lines": [0, 1], "pixels": [0, 1]}"#,
            r#"{"lines": [1]}"#,
            r#"{"lines": [0, "1"]}"#,
            r#"{"lines": 3}"#,
            r#"{"pixels": [0, 1e7]}"#,
            r#"{"lines": [0, 1], "at": {"x": 1}}"#,
            r#"{"lines": [0, 1], "at": {"x": 1, "row": 2}}"#,
            r#"{"lines": [0, 1], "at": {"row": -1, "column": 0}}"#,
            r#"{"lines": [0, 1], "at": {"row": 1.5, "column": 0}}"#,
            r#"{"lines": [0, 1], "at": {"x": 1, "y": 2, "row": 3}}"#,
            r#"{"lines": [0, 1], "at": [1, 2]}"#,
            r#"{"lines": [0, 1], "rows": 3}"#,
        ] {
            assert!(
                wheel(params)
                    .unwrap_err()
                    .message
                    .starts_with("wheel takes"),
                "{params}"
            );
        }
    }

    #[test]
    fn open_file_needs_a_path() {
        assert_eq!(
            parsed(r#"{"method": "open-file", "params": {"path": "/tmp/a"}}"#).method,
            Method::Ui(UiOp::OpenFile("/tmp/a".into()))
        );
        for params in [r#"{}"#, r#"{"path": 1}"#, r#"{"path": ""}"#] {
            let line = format!(r#"{{"method": "open-file", "params": {params}}}"#);
            assert!(error(&line).1.contains("\"path\""), "{params}");
        }
    }

    #[test]
    fn echoes_any_scalar_id() {
        assert_eq!(parsed(r#"{"id": "a", "method": "version"}"#).id, json!("a"));
        assert_eq!(
            parsed(r#"{"id": null, "method": "version"}"#).id,
            json!(null)
        );
        assert_eq!(parsed(r#"{"method": "version"}"#).id, json!(null));
        assert_eq!(parsed(r#"{"id": 1.5, "method": "version"}"#).id, json!(1.5));
    }

    #[test]
    fn rejects_malformed_requests() {
        assert!(error("not json").1.starts_with("invalid JSON"));
        assert_eq!(error("[1]").1, "the request is not a JSON object");
        assert_eq!(
            error(r#"{"id": [1], "method": "version"}"#),
            (json!(null), "id must be a number, a string or null".into())
        );
        assert_eq!(
            error(r#"{"id": 7}"#),
            (json!(7), "method must be a string".into())
        );
        assert_eq!(
            error(r#"{"id": 7, "method": "version", "params": [1]}"#),
            (json!(7), "params must be an object".into())
        );
        assert_eq!(
            error(r#"{"id": 7, "method": "no-such-method"}"#),
            (json!(7), "unknown method \"no-such-method\"".into())
        );
    }

    #[test]
    fn errors_stay_short_whatever_the_request() {
        // JSON escapes for U+0001, which errors print as `\u{1}`.
        let long = "\\u0001".repeat(MAX_LINE / 8);
        let method = error(&format!(r#"{{"id": 1, "method": "{long}"}}"#)).1;
        assert!(method.starts_with("unknown method"), "{method}");
        assert!(method.len() < 600, "{} bytes", method.len());
        let keys = error(&format!(
            r#"{{"id": 1, "method": "send-keys", "params": {{"keys": "{long}"}}}}"#
        ))
        .1;
        assert!(keys.starts_with("invalid key"), "{keys}");
        assert!(keys.len() < 400, "{} bytes", keys.len());
        let id = "a".repeat(MAX_ID + 1);
        assert_eq!(
            error(&format!(r#"{{"id": "{id}", "method": "version"}}"#)),
            (json!(null), format!("id is longer than {MAX_ID} bytes"))
        );
        let id = "a".repeat(MAX_ID);
        assert!(parse(&format!(r#"{{"id": "{id}", "method": "version"}}"#)).is_ok());
    }

    #[test]
    fn rejects_bad_keys() {
        let line =
            |params: &str| format!(r#"{{"id": 1, "method": "send-keys", "params": {params}}}"#);
        assert!(error(&line("{}")).1.contains("\"keys\""));
        assert!(error(&line(r#"{"keys": 1}"#)).1.contains("\"keys\""));
        assert_eq!(
            error(&line(r#"{"keys": " "}"#)).1,
            "send-keys needs at least one key"
        );
        assert!(error(&line(r#"{"keys": "C-foo"}"#)).1.contains("C-foo"));
        let many = vec!["a"; MAX_KEYS + 1].join(" ");
        assert!(
            error(&line(&format!(r#"{{"keys": "{many}"}}"#)))
                .1
                .contains("at most")
        );
        let most = vec!["a"; MAX_KEYS].join(" ");
        assert!(parse(&line(&format!(r#"{{"keys": "{most}"}}"#))).is_ok());
    }

    #[test]
    fn parses_run_command() {
        let line = |params: &str| format!(r#"{{"method": "run-command", "params": {params}}}"#);
        assert_eq!(
            parsed(&line(r#"{"name": "forward-char"}"#)).method,
            Method::Ui(UiOp::RunCommand {
                name: "forward-char".into(),
                arg: None
            })
        );
        assert_eq!(
            parsed(&line(r#"{"name": "next-line", "arg": -3}"#)).method,
            Method::Ui(UiOp::RunCommand {
                name: "next-line".into(),
                arg: Some(-3)
            })
        );
        assert!(error(&line("{}")).1.contains("\"name\""));
        assert!(error(&line(r#"{"name": 1}"#)).1.contains("\"name\""));
        assert!(
            error(&line(r#"{"name": "x", "arg": "2"}"#))
                .1
                .contains("integer")
        );
        assert!(
            error(&line(r#"{"name": "x", "arg": 1.5}"#))
                .1
                .contains("integer")
        );
        for arg in [MAX_COMMAND_ARG, -MAX_COMMAND_ARG] {
            assert!(parse(&line(&format!(r#"{{"name": "x", "arg": {arg}}}"#))).is_ok());
        }
        for arg in [
            MAX_COMMAND_ARG + 1,
            -MAX_COMMAND_ARG - 1,
            isize::MAX,
            isize::MIN,
        ] {
            let message = error(&line(&format!(r#"{{"name": "x", "arg": {arg}}}"#))).1;
            assert!(message.contains("at most"), "{message}");
        }
        let long = "a".repeat(MAX_LINE / 2);
        let long = error(&line(&format!(r#"{{"name": "{long}"}}"#))).1;
        assert!(long.len() < 100, "{long}");
    }

    #[test]
    fn parses_get_text_and_show_buffer() {
        let line =
            |method: &str, params: &str| format!(r#"{{"method": "{method}", "params": {params}}}"#);
        assert_eq!(
            parsed(&line("get-text", r#"{"start_row": 5, "rows": 2}"#)).method,
            Method::Ui(UiOp::GetText {
                start_row: 5,
                rows: 2
            })
        );
        assert!(
            error(&line("get-text", r#"{"rows": 2}"#))
                .1
                .contains("start_row")
        );
        assert!(
            error(&line("get-text", r#"{"start_row": -1, "rows": 2}"#))
                .1
                .contains("start_row")
        );
        assert!(
            error(&line("get-text", r#"{"start_row": 0}"#))
                .1
                .contains("rows")
        );
        assert_eq!(
            parsed(&line("show-buffer", r#"{"buffer": 3}"#)).method,
            Method::Ui(UiOp::ShowBuffer(3))
        );
        assert!(error(&line("show-buffer", "{}")).1.contains("\"buffer\""));
    }

    #[test]
    fn parses_the_handoff_methods() {
        assert_eq!(
            parsed(r#"{"method": "export-state"}"#).method,
            Method::Ui(UiOp::ExportState)
        );
        let state = json!({
            "version": crate::dev::handoff::STATE_VERSION,
            "buffers": [{ "id": 0, "path": null }],
            "windows": [{
                "buffer": 0,
                "primary": 0,
                "selections": [{ "cursor": 0, "goal": null }],
                "scroll": { "start": 0, "left": 0 },
            }],
            "layout": { "window": 0 },
            "selected": 0,
            "pending_keys": "",
            "frame": null,
            "export": 1,
            "items": [],
            "kills": [],
            "histories": [],
        });
        let line = request(1, "import-state", Some(json!({ "state": state })));
        assert!(matches!(
            parsed(&line).method,
            Method::Ui(UiOp::ImportState(_))
        ));
        assert!(
            error(r#"{"method": "import-state", "params": {}}"#)
                .1
                .contains("\"state\"")
        );
        // A bad state is refused here, off the UI thread.
        assert!(
            error(r#"{"method": "import-state", "params": {"state": {"version": 0}}}"#)
                .1
                .contains("version")
        );
        assert_eq!(
            parsed(r#"{"method": "check-handover"}"#).method,
            Method::Ui(UiOp::CheckHandover)
        );
    }

    #[test]
    fn parses_the_methods_that_carry_text_and_messages() {
        let line =
            |method: &str, params: &str| format!(r#"{{"method": "{method}", "params": {params}}}"#);
        assert_eq!(
            parsed(&line(
                "export-text",
                r#"{"export": 2, "item": 1, "offset": 5}"#
            ))
            .method,
            Method::Ui(UiOp::ExportText {
                export: 2,
                item: 1,
                offset: 5
            })
        );
        let Method::Ui(UiOp::ImportText(piece)) = parsed(&line(
            "import-text",
            r#"{"export": 2, "item": 0, "offset": 0, "text": "é\n"}"#,
        ))
        .method
        else {
            panic!("not import-text");
        };
        assert_eq!(
            (piece.export, piece.item, piece.text.as_str()),
            (2, 0, "é\n")
        );
        assert!(
            error(&line(
                "import-text",
                r#"{"export": 2, "item": 0, "offset": 0}"#
            ))
            .1
            .contains("\"text\"")
        );
        assert!(
            error(&line(
                "export-text",
                r#"{"export": 2, "item": -1, "offset": 0}"#
            ))
            .1
            .contains("item")
        );
        assert_eq!(
            parsed(&line("dev-message", r#"{"text": "oso dev: build failed"}"#)).method,
            Method::Ui(UiOp::DevMessage("oso dev: build failed".into()))
        );
        let long = json!({ "text": "x".repeat(super::MAX_DEV_MESSAGE + 1) }).to_string();
        assert!(error(&line("dev-message", &long)).1.contains("at most"));
    }

    #[test]
    fn a_text_takes_no_more_than_its_escaped_length_in_a_line() {
        let text = "a\"\\\u{1}\n\u{85}\u{202e}é\u{e0041}😀";
        let line = success(&json!(1), json!({ "text": text }));
        let quoted = line.len() - r#"{"id":1,"result":{"text":""}}"#.len();
        assert!(quoted <= super::escaped_len(text), "{line}");
    }

    #[test]
    fn exit_is_unconditional_only_without_a_generation() {
        assert_eq!(
            parsed(r#"{"method": "exit"}"#).method,
            Method::Ui(UiOp::Exit { generation: None })
        );
        let line = request(1, "exit", Some(json!({ "generation": u64::MAX })));
        assert_eq!(
            parsed(&line).method,
            Method::Ui(UiOp::Exit {
                generation: Some(u64::MAX)
            })
        );
        // A caller that lost its generation must not end up exiting anyway.
        for bad in [json!(null), json!("1"), json!(-1), json!(1.5)] {
            let line = request(1, "exit", Some(json!({ "generation": bad })));
            assert!(error(&line).1.contains("generation"), "{line}");
        }
        for misplaced in [
            r#"{"method": "exit", "params": {"generaton": 5}}"#,
            r#"{"method": "exit", "generation": 5}"#,
            r#"{"method": "exit", "generation": 5, "params": {"generation": 5}}"#,
        ] {
            assert!(error(misplaced).1.contains("generation"), "{misplaced}");
        }
        assert_eq!(
            parsed(r#"{"method": "exit", "params": null}"#).method,
            Method::Ui(UiOp::Exit { generation: None }),
            "as for any method, null params are none"
        );
    }

    #[test]
    fn reads_response_lines() {
        use super::response_result;
        assert_eq!(
            response_result(r#"{"id":1,"result":{"a":1}}"#),
            Ok(json!({ "a": 1 }))
        );
        assert_eq!(
            response_result(r#"{"id":1,"result":null}"#),
            Ok(json!(null))
        );
        assert_eq!(
            response_result(r#"{"id":1,"error":{"message":"no"}}"#),
            Err("no".to_owned())
        );
        assert!(
            response_result(r#"{"id":1}"#).is_err(),
            "not a confirmation"
        );
        assert!(response_result("not json").is_err());
    }

    #[test]
    fn responses_are_single_json_lines() {
        let ok = success(&json!(1), json!({"a": "x\ny"}));
        assert!(!ok.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&ok).unwrap(),
            json!({"id": 1, "result": {"a": "x\ny"}})
        );
        let err = failure(&json!("q"), "bad\nthing");
        assert!(!err.contains('\n'));
        assert_eq!(
            serde_json::from_str::<Value>(&err).unwrap(),
            json!({"id": "q", "error": {"message": "bad\nthing"}})
        );
    }

    #[test]
    fn what_a_terminal_or_reader_should_not_see_raw_is_escaped() {
        // CSI, DEL, a bidi override, a tag character and a zero-width space.
        let text = "a\u{9b}31m\u{7f}\u{202e}\u{e0041}\u{200b}日本語\\\"🍣";
        let line = success(&json!(1), json!({ "text": text }));
        assert!(line.is_ascii() || line.contains("日本語"));
        assert!(!line.contains('\u{9b}'), "{line}");
        assert!(!line.contains('\u{202e}'), "{line}");
        assert!(line.contains("\\u009b"), "{line}");
        assert!(line.contains("\\udb40\\udc41"), "a surrogate pair: {line}");
        assert!(line.contains("日本語"), "ordinary text stays: {line}");
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["result"]["text"],
            text,
            "it parses to the same text"
        );
        let error = failure(&json!(1), "bad \u{9b}");
        assert!(!error.contains('\u{9b}'), "{error}");
    }

    #[test]
    fn requests_round_trip() {
        let line = request(9, "send-keys", Some(json!({"keys": "RET"})));
        assert_eq!(parsed(&line).id, json!(9));
        assert_eq!(
            parse(&request(1, "version", None)).unwrap().method,
            Method::Version
        );
    }
}
