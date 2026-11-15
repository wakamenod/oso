//! oso's fake language server, for tests only (ADR 0138, step 2): it reads messages on its
//! standard input with `oso-lsp`'s framing and does what the script named by its first
//! argument says, misbehaviour included. An example, so it never ships.
//!
//! Must not depend on: anything of the client's but `framing`; its replies are written by
//! hand, so a test of the client can check the client.
//!
//! Run as `fake-server <script.json>`. A message that is not JSON, or a broken stream, ends
//! it with 2; a script it cannot read, with 64.

mod script;

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use oso_lsp::framing::{Frame, Reader};

use script::{Action, Script};

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        return ExitCode::from(64);
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return ExitCode::from(64);
    };
    let Ok(mut script) = Script::parse(&text) else {
        return ExitCode::from(64);
    };
    if let Some(code) = perform(&script.start()) {
        return code;
    }
    let mut reader = Reader::new(io::stdin().lock());
    loop {
        let message = match reader.next_frame() {
            Ok(Some(Frame::Message(body))) => match serde_json::from_slice(&body) {
                Ok(message) => message,
                Err(_) => return ExitCode::from(2),
            },
            Ok(Some(Frame::Dropped { .. })) => continue,
            Ok(None) => return ExitCode::SUCCESS,
            Err(_) => return ExitCode::from(2),
        };
        if let Some(code) = perform(&script.answer(&message)) {
            return code;
        }
    }
}

/// Carries out `actions`, and gives the code to end with if one ends the server or a write
/// fails.
fn perform(actions: &[Action]) -> Option<ExitCode> {
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());
    for action in actions {
        let written = match action {
            Action::Out(bytes) => out.write_all(bytes).and_then(|()| out.flush()),
            Action::Err(bytes) => err.write_all(bytes).and_then(|()| err.flush()),
            Action::Sleep(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                Ok(())
            }
            Action::Exit(code) => return Some(ExitCode::from(u8::try_from(*code).unwrap_or(1))),
        };
        if written.is_err() {
            return Some(ExitCode::from(2));
        }
    }
    None
}
