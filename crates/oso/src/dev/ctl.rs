//! `oso ctl`: the control socket client (ADR 0023). It sends one request to
//! a running dev instance and prints the JSON response.
//!
//! Must not depend on: `app`, winit or editor state; it runs without a window
//! and talks only to the socket.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::time::Duration;

use oso_platform::control::ControlDir;
use serde_json::{Value, json};

use super::protocol::{self, MAX_LINE};

const USAGE: &str = "\
usage: oso ctl [--pid PID] <method> [ARG]

Sends one request to a running oso dev instance and prints the response.
Without --pid, the only running instance is used.

methods:
  version           protocol version and build information
  get-state         buffers, windows, selections, pending keys, surface size and
                    the selected window's visible text
  get-text START ROWS
                    whole rows of the selected window's buffer, up to a limit
  screenshot        writes a PNG of the current frame and prints its path
  send-keys KEYS    sends keys in Emacs notation, such as \"C-x C-f\" or \"<right>\"
  take-timings      key latencies since the last call, startup and load times
  open-file PATH    visits a file in the selected window; nothing in it runs
  run-command NAME [ARG]
                    runs a command by name, with a numeric argument if it takes one
  show-buffer ID    shows a buffer, by the id get-state reports
  check-handover    the generation, or why a restart would lose text
  export-state      what a new instance needs to take over: paths and positions,
                    and the generation
  import-state JSON takes over from that state, in an instance that opened nothing
                    and has no edits
  exit [GENERATION] exits the instance: without asking, unsaved text or not, or,
                    given a generation, only if nothing has changed since it
  ime JSON          reports what the OS's input method would, such as
                    '{\"preedit\": \"にほ\", \"range\": [6, 6]}', '{\"commit\": \"日本\"}',
                    '{\"clear\": true}' or '{\"disabled\": true}'

Secret-pattern files (ADR 0044): get-state and get-text return no text for them,
and screenshot fails while one is shown.

exit status: 0 on a result, 1 on an error response, 2 if no request was made";

/// How long to wait for the response. Longer than the server's own wait for
/// the UI, so its timeout error arrives first.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs `oso ctl` if the first argument is `ctl`, and returns its exit
/// status.
pub fn main_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != "ctl" {
        return None;
    }
    Some(run(args.collect()))
}

#[expect(
    clippy::disallowed_macros,
    reason = "`oso ctl` is a command-line tool: its output is stdout and stderr"
)]
fn run(args: Vec<OsString>) -> i32 {
    let invocation = match parse_args(args) {
        Ok(Some(invocation)) => invocation,
        Ok(None) => {
            println!("{USAGE}");
            return 0;
        }
        Err(message) => {
            eprintln!("oso ctl: {message}\n\n{USAGE}");
            return 2;
        }
    };
    match call(&invocation) {
        Ok(response) => {
            println!("{response}");
            let is_error = serde_json::from_str::<Value>(&response)
                .map_or(true, |value| value.get("error").is_some());
            i32::from(is_error)
        }
        Err(e) => {
            eprintln!("oso ctl: {e}");
            2
        }
    }
}

/// A parsed command line.
#[derive(Debug, PartialEq)]
struct Invocation {
    pid: Option<u32>,
    method: String,
    params: Option<Value>,
}

/// Parses the arguments after `ctl`. `None` means help was asked for.
fn parse_args(args: Vec<OsString>) -> Result<Option<Invocation>, String> {
    let mut args = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|arg| format!("argument is not UTF-8: {arg:?}"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let mut pid = None;
    let mut positional = Vec::new();
    while let Some(arg) = args.next() {
        let pid_value = match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--pid" => Some(args.next().ok_or("--pid needs a process ID")?),
            _ => arg.strip_prefix("--pid=").map(str::to_owned),
        };
        if let Some(value) = pid_value {
            let parsed = value
                .parse()
                .map_err(|_| format!("not a process ID: {value:?}"))?;
            pid = Some(parsed);
        } else if arg.starts_with("--") {
            return Err(format!("unknown option {arg:?}"));
        } else {
            positional.push(arg);
        }
    }
    let mut positional = positional.into_iter();
    let method = positional.next().ok_or("no method given")?;
    let params = parse_params(&method, &mut positional)?;
    if let Some(extra) = positional.next() {
        return Err(format!("unexpected argument {extra:?}"));
    }
    Ok(Some(Invocation {
        pid,
        method,
        params,
    }))
}

/// The parameters of `method`, from the arguments after it.
fn parse_params(
    method: &str,
    positional: &mut impl Iterator<Item = String>,
) -> Result<Option<Value>, String> {
    let params = match method {
        "get-text" => {
            let start = parse_number(method, "START", positional)?;
            let rows = parse_number(method, "ROWS", positional)?;
            Some(json!({ "start_row": start, "rows": rows }))
        }
        "show-buffer" => Some(json!({ "buffer": parse_number(method, "ID", positional)? })),
        "import-state" => {
            let state = positional.next().ok_or("import-state needs JSON")?;
            let state: Value =
                serde_json::from_str(&state).map_err(|e| format!("import-state: {e}"))?;
            Some(json!({ "state": state }))
        }
        "ime" => {
            let report = positional.next().ok_or("ime needs JSON")?;
            let report: Value = serde_json::from_str(&report).map_err(|e| format!("ime: {e}"))?;
            Some(report)
        }
        "send-keys" => {
            let keys = positional.next().ok_or("send-keys needs KEYS")?;
            Some(json!({ "keys": keys }))
        }
        "open-file" => {
            let path = positional.next().ok_or("open-file needs PATH")?;
            Some(json!({ "path": path }))
        }
        "exit" => positional
            .next()
            .map(|generation| {
                generation
                    .parse::<u64>()
                    .map(|generation| json!({ "generation": generation }))
                    .map_err(|_| format!("GENERATION is not a number: {generation:?}"))
            })
            .transpose()?,
        "run-command" => {
            let name = positional.next().ok_or("run-command needs NAME")?;
            let arg = positional
                .next()
                .map(|arg| {
                    arg.parse::<i64>()
                        .map_err(|_| format!("ARG is not a number: {arg:?}"))
                })
                .transpose()?;
            Some(json!({ "name": name, "arg": arg }))
        }
        _ => None,
    };
    Ok(params)
}

/// The next argument of `method`, called `what` in messages, as a number.
fn parse_number(
    method: &str,
    what: &str,
    positional: &mut impl Iterator<Item = String>,
) -> Result<i64, String> {
    let value = positional
        .next()
        .ok_or_else(|| format!("{method} needs {what}"))?;
    value
        .parse()
        .map_err(|_| format!("{what} is not a number: {value:?}"))
}

/// Sends the request and returns the response line.
fn call(invocation: &Invocation) -> io::Result<String> {
    let dir = ControlDir::open_existing()?;
    exchange(
        &dir,
        invocation.pid,
        &invocation.method,
        invocation.params.clone(),
    )
}

/// Sends one request to the instance `pid`, or the only one, and returns the
/// response line.
pub fn exchange(
    dir: &ControlDir,
    pid: Option<u32>,
    method: &str,
    params: Option<Value>,
) -> io::Result<String> {
    exchange_within(dir, pid, method, params, RESPONSE_TIMEOUT)
}

/// [`exchange`], giving up on a response after `patience`.
pub fn exchange_within(
    dir: &ControlDir,
    pid: Option<u32>,
    method: &str,
    params: Option<Value>,
    patience: Duration,
) -> io::Result<String> {
    let (_, stream) = dir.connect(pid)?;
    stream.set_read_timeout(Some(patience))?;
    stream.set_write_timeout(Some(patience))?;
    let mut writer = stream.try_clone()?;
    let request = protocol::request(1, method, params);
    writeln!(writer, "{request}")?;

    let limit = u64::try_from(MAX_LINE + 1).expect("the limit fits in u64");
    let mut response = Vec::new();
    BufReader::new(stream)
        .take(limit)
        .read_until(b'\n', &mut response)?;
    if response.last() != Some(&b'\n') {
        let reason = if response.is_empty() {
            "the instance closed the connection without a response"
        } else {
            "the response is incomplete or too long"
        };
        return Err(io::Error::new(io::ErrorKind::InvalidData, reason));
    }
    String::from_utf8(response)
        .map(|line| line.trim_end().to_owned())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the response is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{Invocation, parse_args, protocol};
    use serde_json::json;
    use std::ffi::OsString;

    fn parse(args: &[&str]) -> Result<Option<Invocation>, String> {
        parse_args(args.iter().map(OsString::from).collect())
    }

    #[test]
    fn parses_methods_and_arguments() {
        assert_eq!(
            parse(&["version"]),
            Ok(Some(Invocation {
                pid: None,
                method: "version".into(),
                params: None,
            }))
        );
        assert_eq!(
            parse(&["--pid", "42", "send-keys", "<right> <right>"]),
            Ok(Some(Invocation {
                pid: Some(42),
                method: "send-keys".into(),
                params: Some(json!({ "keys": "<right> <right>" })),
            }))
        );
        assert_eq!(
            parse(&["open-file", "/tmp/a"]).unwrap().unwrap().params,
            Some(json!({ "path": "/tmp/a" }))
        );
        assert_eq!(
            parse(&["screenshot", "--pid=7"]).unwrap().unwrap().pid,
            Some(7)
        );
    }

    #[test]
    fn parses_run_command_and_its_negative_argument() {
        let params = |args: &[&str]| parse(args).unwrap().unwrap().params;
        assert_eq!(
            params(&["run-command", "forward-char"]),
            Some(json!({ "name": "forward-char", "arg": null }))
        );
        assert_eq!(
            params(&["run-command", "next-line", "-3"]),
            Some(json!({ "name": "next-line", "arg": -3 }))
        );
        assert_eq!(
            parse(&["run-command"]),
            Err("run-command needs NAME".into())
        );
        assert!(
            parse(&["run-command", "next-line", "x"])
                .unwrap_err()
                .contains("not a number")
        );
    }

    #[test]
    fn parses_get_text_and_show_buffer() {
        let params = |args: &[&str]| parse(args).unwrap().unwrap().params;
        assert_eq!(
            params(&["get-text", "5", "2"]),
            Some(json!({ "start_row": 5, "rows": 2 }))
        );
        assert_eq!(params(&["show-buffer", "1"]), Some(json!({ "buffer": 1 })));
        assert_eq!(parse(&["get-text", "5"]), Err("get-text needs ROWS".into()));
        assert!(parse(&["show-buffer", "x"]).unwrap_err().contains("\"x\""));
    }

    /// The smallest state the server accepts.
    const STATE: &str = r#"{
        "version": 1,
        "buffers": [{ "id": 0 }],
        "windows": [{
            "buffer": 0,
            "primary": 0,
            "selections": [{ "cursor": 0 }],
            "scroll": { "top": 0, "left": 0 }
        }],
        "selected": 0,
        "pending_keys": ""
    }"#;

    #[test]
    fn the_server_accepts_what_the_client_sends() {
        for args in [
            &["version"][..],
            &["get-state"],
            &["screenshot"],
            &["get-text", "0", "5"],
            &["show-buffer", "0"],
            &["send-keys", "C-f"],
            &["run-command", "forward-char"],
            &["run-command", "forward-char", "-2"],
            &["check-handover"],
            &["export-state"],
            &["import-state", STATE],
            &["exit"],
            &["exit", "18446744073709551615"],
        ] {
            let invocation = parse(args).unwrap().unwrap();
            let line = protocol::request(1, &invocation.method, invocation.params);
            assert!(protocol::parse(&line).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn import_state_takes_json() {
        let parsed = parse(&["import-state", r#"{"version":1}"#])
            .unwrap()
            .unwrap();
        assert_eq!(parsed.params, Some(json!({ "state": { "version": 1 } })));
        assert_eq!(
            parse(&["import-state"]),
            Err("import-state needs JSON".into())
        );
        assert!(
            parse(&["import-state", "{nope"])
                .unwrap_err()
                .starts_with("import-state:")
        );
    }

    #[test]
    fn ime_takes_a_report_as_json() {
        let parsed = parse(&["ime", r#"{"commit":"日本"}"#]).unwrap().unwrap();
        assert_eq!(parsed.params, Some(json!({ "commit": "日本" })));
        assert_eq!(parse(&["ime"]), Err("ime needs JSON".into()));
        assert!(parse(&["ime", "{nope"]).unwrap_err().starts_with("ime:"));
    }

    #[test]
    fn exit_takes_a_generation_or_nothing() {
        let params = |args: &[&str]| parse(args).unwrap().unwrap().params;
        assert_eq!(params(&["exit"]), None);
        assert_eq!(
            params(&["exit", "7"]),
            Some(json!({ "generation": 7 })),
            "guarded"
        );
        // Not a silent unconditional exit.
        assert!(parse(&["exit", "x"]).unwrap_err().contains("\"x\""));
        assert!(parse(&["exit", "-1"]).unwrap_err().contains("\"-1\""));
    }

    #[test]
    fn unknown_methods_are_left_to_the_server() {
        let invocation = parse(&["no-such-method"]).unwrap().unwrap();
        assert_eq!(invocation.method, "no-such-method");
        assert_eq!(invocation.params, None);
    }

    #[test]
    fn help_is_not_an_error() {
        assert_eq!(parse(&["--help"]), Ok(None));
        assert_eq!(parse(&["version", "-h"]), Ok(None));
    }

    #[test]
    fn rejects_bad_command_lines() {
        assert_eq!(parse(&[]), Err("no method given".into()));
        assert_eq!(parse(&["send-keys"]), Err("send-keys needs KEYS".into()));
        assert_eq!(parse(&["open-file"]), Err("open-file needs PATH".into()));
        assert_eq!(parse(&["--pid"]), Err("--pid needs a process ID".into()));
        assert!(
            parse(&["--pid", "x", "version"])
                .unwrap_err()
                .contains("\"x\"")
        );
        assert!(
            parse(&["--verbose", "version"])
                .unwrap_err()
                .contains("unknown option")
        );
        assert!(
            parse(&["version", "extra"])
                .unwrap_err()
                .contains("unexpected")
        );
        assert!(
            parse(&["send-keys", "a", "b"])
                .unwrap_err()
                .contains("unexpected")
        );
    }
}
