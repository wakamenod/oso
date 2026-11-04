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
  get-state         the buffer, cursor, scroll position and surface size
  screenshot        writes a PNG of the current frame and prints its path
  send-keys KEYS    sends keys in Emacs notation, such as \"C-x C-f\" or \"<right>\"
  take-timings      key latencies since the last call, startup and load times
  open-file PATH    visits a file in the selected window; nothing in it runs

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
    let params = match method.as_str() {
        "send-keys" => {
            let keys = positional.next().ok_or("send-keys needs KEYS")?;
            Some(json!({ "keys": keys }))
        }
        "open-file" => {
            let path = positional.next().ok_or("open-file needs PATH")?;
            Some(json!({ "path": path }))
        }
        _ => None,
    };
    if let Some(extra) = positional.next() {
        return Err(format!("unexpected argument {extra:?}"));
    }
    Ok(Some(Invocation {
        pid,
        method,
        params,
    }))
}

/// Sends the request and returns the response line.
fn call(invocation: &Invocation) -> io::Result<String> {
    let dir = ControlDir::open_existing()?;
    let (_, stream) = dir.connect(invocation.pid)?;
    stream.set_read_timeout(Some(RESPONSE_TIMEOUT))?;
    stream.set_write_timeout(Some(RESPONSE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let request = protocol::request(1, &invocation.method, invocation.params.clone());
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
    use super::{Invocation, parse_args};
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
    fn unknown_methods_are_left_to_the_server() {
        let invocation = parse(&["run-command"]).unwrap().unwrap();
        assert_eq!(invocation.method, "run-command");
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
