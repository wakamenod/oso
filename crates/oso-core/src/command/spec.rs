//! What a command reads before it runs: its argument spec, which documents it and tells the
//! dispatcher and `run-command` what to read or accept (ADR 0034, ADR 0072, ADR 0076, ADR 0088).
//!
//! Must not depend on: `keymap`.

use super::Prefix;

/// Which arguments a command takes. It documents the command
/// (`describe-function`) and tells `run-command` what to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    /// No arguments. A prefix argument, if given, is ignored, as in Emacs.
    None,
    /// A numeric prefix argument, read with [`Args::count`](super::Args::count): a repeat count
    /// that defaults to 1 and may be negative to reverse the direction.
    Count,
    /// A whole number, read in a prompt that says `prompt` before the command runs, and
    /// kept in the history named `history` (ADR 0072). A numeric prefix argument stands
    /// for it, with no prompt. The command reads it with [`Ctx::answer`](super::Ctx::answer).
    Number {
        prompt: &'static str,
        history: &'static str,
    },
    /// A passphrase, read in a prompt that says `prompt` and shows one `*` per character.
    /// It enters no history, and its buffer may hold secrets. The command reads it with
    /// [`Ctx::answer`](super::Ctx::answer), and must not keep it.
    Passphrase { prompt: &'static str },
    /// A command's name, read in a prompt that says `prompt` and completes over the
    /// registry, kept in the history named `history` (ADR 0076). The dispatcher runs that
    /// command in its place, with its prefix argument, as `M-x` does.
    Command {
        prompt: &'static str,
        history: &'static str,
    },
    /// A buffer's name, read in a prompt that says `prompt` and completes over the buffers,
    /// most recent first, kept in the history named `history` (ADR 0088). The command reads
    /// the name with [`Ctx::answer`](super::Ctx::answer).
    Buffer {
        prompt: &'static str,
        history: &'static str,
        purpose: BufferPurpose,
    },
    /// A mark of the selected window's, read in a prompt that says `prompt` and completes over
    /// the primary cursor's mark and its ring, with the buffer around the selected one beside
    /// it, and no history (ADR 0088). The command reads the row with
    /// [`Ctx::answer`](super::Ctx::answer).
    Mark { prompt: &'static str },
    /// The key that ran it, read with [`Ctx::input`](super::Ctx::input) or [`Ctx::key_char`](super::Ctx::key_char), and a numeric
    /// prefix argument as with `Count`. `M-x` does not list it, as no key runs it there
    /// (ADR 0076).
    Key,
    /// A numeric prefix argument, and with none or only `C-u`, what the OS clipboard
    /// holds, read with [`Ctx::clipboard`](super::Ctx::clipboard). The dispatcher reads the clipboard first, off
    /// the UI thread, and runs the command once it has the answer, as `yank` needs
    /// (ADR 0074).
    CountAndClipboard,
}

/// What a buffer is read for, which decides the order and what the prompt takes (ADR 0088).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferPurpose {
    /// To show: the current buffer last, and a name that matches none makes a buffer.
    Switch,
    /// To kill: the current buffer first, and only a listed one.
    Kill,
}

impl ArgSpec {
    /// Whether a command with this spec, given `prefix`, waits for the clipboard.
    pub const fn reads_clipboard(self, prefix: Option<Prefix>) -> bool {
        matches!(self, Self::CountAndClipboard)
            && matches!(prefix, None | Some(Prefix::Universal(_)))
    }
}
