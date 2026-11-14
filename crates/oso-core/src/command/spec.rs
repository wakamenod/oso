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
    /// Text, read in a prompt that says `prompt` and kept in the history named `history`
    /// (ADR 0085). The agent channel gives it with the command, with no prompt. The command
    /// reads it with [`Ctx::answer`](super::Ctx::answer).
    Text {
        prompt: &'static str,
        history: &'static str,
    },
    /// Nothing with no prefix argument: the command runs at once with no answer. With one,
    /// such as `C-u`, a directory is read first, as [`ArgSpec::File`] reads a path, in a prompt
    /// that says `directory` and kept in the history `directory_history`, then text as
    /// [`ArgSpec::Text`] reads it, in a prompt that says `prompt` and kept in the history
    /// `history` (ADR 0121). An empty answer to either stands for the command's own default.
    /// Only keys give them: the agent channel runs such a command with neither. The command
    /// reads both with [`Ctx::directory_and_text`](super::Ctx::directory_and_text).
    DirectoryAndTextOnPrefix {
        directory: &'static str,
        directory_history: &'static str,
        prompt: &'static str,
        history: &'static str,
    },
    /// A branch of the current project, read in a prompt that says `prompt` and completes over
    /// the project's local branches, which the host lists off the UI thread before the prompt
    /// opens; a name that is none of them is taken as typed, a new branch. With a prefix
    /// argument, a base is read after it, in a prompt that says `base_prompt`, completing over
    /// the same branches and taking any revision. Both are kept in the history `history`. Only
    /// keys give them: the agent channel runs such a command with neither. The command reads
    /// them with [`Ctx::branch_and_base`](super::Ctx::branch_and_base).
    BranchAndBaseOnPrefix {
        prompt: &'static str,
        base_prompt: &'static str,
        history: &'static str,
    },
    /// Text as [`ArgSpec::Text`] reads it, but only with a prefix argument, such as `C-u`; with
    /// none, the command runs at once with no answer. The agent channel gives the text with the
    /// command, or nothing. The command reads it with [`Ctx::answer`](super::Ctx::answer), and
    /// the prefix with its arguments.
    TextOnPrefix {
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
    /// A theme's name, read in a prompt that says `prompt` and completes over the themes the
    /// host draws with, kept in the history named `history` (ADR 0089). The command reads it
    /// with [`Ctx::answer`](super::Ctx::answer).
    Theme {
        prompt: &'static str,
        history: &'static str,
    },
    /// One of a list the host keeps, read in a prompt that says `prompt` and completes over
    /// the candidates the host last gave for `list` with
    /// [`Editor::set_listed`](crate::editor::Editor::set_listed), kept in the history named
    /// `history` (ADR 0121): agent sessions, and their spaces. Their annotations are searched
    /// as completion searches any, so a word of one filters by it. The command reads the
    /// candidate's text with [`Ctx::answer`](super::Ctx::answer).
    Listed {
        list: &'static str,
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
    /// A path, read in a prompt that says `prompt` and completes by segments over listed
    /// directories, starting at the buffer's directory, kept in the history named `history`
    /// (ADR 0087). The command reads the whole path with [`Ctx::answer`](super::Ctx::answer).
    File {
        prompt: &'static str,
        history: &'static str,
    },
    /// A file of the current buffer's project, read in a prompt that completes over the
    /// project's files as paths from its root, listed as they are walked, and kept in the
    /// history named `history`, one for each project (ADR 0114). Its label names the project,
    /// so it has no prompt of its own. With `C-u`, a directory is read first, as for `File`, and
    /// stands for the project. The command reads the whole path with
    /// [`Ctx::answer`](super::Ctx::answer).
    ProjectFile { history: &'static str },
    /// A line of a file of the current buffer's project, read in a prompt whose input is
    /// searched for in the project's files, line by line, the rows coming as they are found,
    /// and kept in the history named `history` (ADR 0114). Its label names the project. With
    /// `C-u`, a directory is read first, as for `ProjectFile`. The command reads the file's
    /// whole path with [`Ctx::answer`](super::Ctx::answer) and the line with
    /// [`Editor::take_search_hit`](crate::editor::Editor::take_search_hit).
    ProjectSearch { history: &'static str },
    /// A menu's name, read in a prompt that says `prompt` and completes over every menu, kept in
    /// the history named `history` (ADR 0117). The dispatcher opens the menu named in place of
    /// running the command, as it runs the command `Command` names, since the open menus are
    /// its own.
    Menu {
        prompt: &'static str,
        history: &'static str,
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
    /// An entry of the kill ring, read in a prompt that says `prompt` and completes over the
    /// ring, newest first, with the selected entry's text beside it, after the dispatcher has
    /// read the clipboard, as for `CountAndClipboard`, so that another app's newest copy is
    /// in the ring (ADR 0091). The command yanks it with
    /// [`Editor::yank_picked`](crate::editor::Editor::yank_picked).
    KillEntry { prompt: &'static str },
    /// A string to replace and its replacement, read in two prompts kept in one history, a
    /// regexp's if `regexp`, before query-replace begins (ADR 0113). An empty first answer
    /// takes the last pair, with no second prompt. The command reads both with
    /// [`Ctx::replace_pair`](super::Ctx::replace_pair).
    Replace { regexp: bool },
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
        match self {
            Self::CountAndClipboard => matches!(prefix, None | Some(Prefix::Universal(_))),
            Self::KillEntry { .. } => true,
            _ => false,
        }
    }
}
