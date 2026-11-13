//! The terminal engine behind a trait (ADR 0120): it parses a program's output into a grid,
//! fills snapshots from the rows that changed, and reports what the output asks of oso as
//! effects, which the policy judges. libghostty-vt is the one engine; the trait keeps another
//! possible, such as one in a helper process.
//!
//! Must not depend on: the policy or the thread. All of the crate's `unsafe` is in `ghostty`'s
//! `ffi` and `raw`.

use oso_platform::process::pty::Size;

use crate::event::Progress;
use crate::snapshot::{Palette, Row, Snapshot};

#[cfg(unix)]
pub(crate) mod ghostty;

/// A terminal engine. It runs on one thread, its terminal's, and never leaves it.
pub(crate) trait Engine {
    /// Parses program output. Effects it raises wait for `take_effects`.
    fn feed(&mut self, bytes: &[u8]);

    /// Resizes the grid; the size is clamped to the limits already.
    fn resize(&mut self, size: Size);

    /// The theme's colors, for drawing defaults and for answering color queries.
    fn set_palette(&mut self, palette: &Palette);

    /// Puts the rows changed since the last call into `out.rows`, reusing rows from `spare`,
    /// and sets its size, cursor, modes and history. Returns whether any row changed.
    fn snapshot(&mut self, out: &mut Snapshot, spare: &mut Vec<Row>) -> bool;

    /// Moves the effects raised since the last call into `out`.
    fn take_effects(&mut self, out: &mut Vec<Effect>);
}

/// Something the output asked for, before the policy judges it. Text is the bytes the
/// program sent, not yet cleaned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Bytes the engine would write back to the program: device and mode reports.
    Reply(Vec<u8>),
    Bell,
    Title(Vec<u8>),
    Directory(Vec<u8>),
    Notification {
        title: Vec<u8>,
        body: Vec<u8>,
    },
    Progress(Progress),
    /// An OSC 133 mark.
    Prompt(PromptMark),
    ClipboardWrite(Vec<u8>),
    /// An OSC or APC the engine does not know, at most 4 KiB of it.
    Unknown(Vec<u8>),
}

/// OSC 133's marks as the engine reads them: a command line is still raw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PromptMark {
    Start,
    Input,
    Output(Vec<u8>),
    End(Option<i32>),
}
