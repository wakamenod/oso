//! What a terminal tells the host besides its grid: the events its sequences raise, already
//! judged by the policy, and how it ended (ADR 0120).
//!
//! Must not depend on: the engine or the thread.

use std::io;
use std::process::ExitStatus;

/// One thing a terminal reports, in the order it happened. Text in it has been cleaned of
/// control, bidi and invisible characters and cut to 256 bytes, except a clipboard write.
#[derive(Debug)]
pub enum Event {
    /// OSC 0, 1 or 2.
    Title(String),
    Bell,
    /// The working directory as the program sent it: OSC 7's `file:` URI, or a plain path
    /// from OSC 9;9 or OSC 1337 `CurrentDir`. At most 4 KiB, UTF-8, no control characters;
    /// whether it names a local directory is the host's to check.
    Directory(String),
    /// OSC 9 or 777.
    Notification {
        title: String,
        body: String,
    },
    /// OSC 9;4.
    Progress(Progress),
    /// OSC 133.
    Prompt(Prompt),
    /// The text of an OSC 52, OSC 5522 or OSC 1337 `Copy` write, at most 1 MiB, for the
    /// kill ring.
    ClipboardWrite(String),
    /// The terminal ended; nothing follows.
    Ended(End),
}

/// OSC 9;4's states, with the percent where one was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Remove,
    Set(u8),
    Error(Option<u8>),
    Indeterminate,
    Paused(Option<u8>),
}

/// OSC 133's marks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// `A`: a prompt starts.
    Start,
    /// `B`: the prompt's input starts.
    Input,
    /// `C`: a command's output starts.
    Output { command: Option<String> },
    /// `D`: the command ended.
    End { exit: Option<i32> },
}

/// How a terminal ended.
#[derive(Debug)]
pub enum End {
    /// The child exited, with this status.
    Exited(ExitStatus),
    /// The terminal's end came without a status: the child outlived its terminal by more
    /// than 2 s, or its handle was dropped.
    Closed,
    /// The terminal could not start, or its thread failed.
    Failed(io::Error),
}
