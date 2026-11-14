//! The command registry (ADR 0034).
//!
//! Must not depend on: `keymap`, key presses or the concrete commands, which
//! the tip crate registers.
//!
//! A [`Command`] is a name, a docstring, an argument spec, a [`Reach`] and a
//! plain function over host-owned state: `fn(&mut Ctx, Args) -> Result<(),
//! CommandError>`. The host owns a [`Registry`] and the [`Editor`], and
//! lends the editor to each command through a [`Ctx`]. Commands keep no
//! state of their own, which keeps them testable and restartable.
//!
//! A command runs once per invocation and operates on the whole selection
//! set (ADR 0007), not once per cursor.
//!
//! [`Registry::execute`] runs a command under `catch_unwind`, so a panicking
//! command costs one error, not the session (ADR 0014).

use std::borrow::Cow;
use std::fmt;

use crate::buffer::EditError;
use crate::editor::Editor;
use crate::kill::FromClipboard;

mod registry;
mod spec;

pub use registry::{CommandId, ExecuteError, RegisterError, Registry};
pub use spec::{ArgSpec, BufferPurpose};

/// The function behind a command.
pub type CommandFn = fn(&mut Ctx<'_>, Args) -> Result<(), CommandError>;

/// How a command that toggles reads whether its state is on, for the `[x]` a menu head naming it
/// shows (ADR 0117). It reads only the editor, so it does no I/O.
pub type StateFn = fn(&Editor) -> bool;

/// A command: what `M-x`, keymaps and the control socket's `run-command`
/// invoke.
#[derive(Clone, Copy)]
pub struct Command {
    /// The name, in lower-case kebab case: `forward-char`.
    pub name: &'static str,
    /// What the command does. The first line is a one-sentence summary.
    pub doc: &'static str,
    /// Which arguments the command takes.
    pub args: ArgSpec,
    /// How far the agent channel may reach it (ADR 0085). It has no default, so that each
    /// command decides.
    pub reach: Reach,
    /// The function that runs it.
    pub run: CommandFn,
    /// How to read whether it is on, for a command that toggles; `None` for any other
    /// (ADR 0117).
    pub state: Option<StateFn>,
}

impl Command {
    /// The first line of its docstring: a one-sentence summary.
    pub fn summary(&self) -> &'static str {
        self.doc.lines().next().unwrap_or_default()
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Command")
            .field("name", &self.name)
            .field("args", &self.args)
            .field("reach", &self.reach)
            .finish_non_exhaustive()
    }
}

/// How far the agent channel may reach a command (ADR 0085). The agent channel enforces it.
/// Keys, `M-x` and the dev control socket's `run-command`, which trusts the same user
/// (ADR 0044), reach every command whatever it says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Moves, marks, shows or opens, and changes no buffer text and no file. Copying to the
    /// kill ring is one: from the channel it never reaches the OS clipboard.
    Look,
    /// Edits buffer text or saves.
    Change,
    /// Needs a person at the keyboard, and the channel refuses it. Either it must stay the
    /// person's (exiting, answering a prompt, reading the clipboard, a passphrase, anything
    /// that spawns a process, trusts a project, or shows what a buffer that may hold secrets
    /// withholds), or it reads a key, which the channel replaces with a count or text.
    Person,
}

/// The largest count the dev socket or the agent channel passes on, in either direction: a
/// command runs on the UI thread and may repeat its work this many times (ADR 0044).
pub const MAX_REMOTE_COUNT: isize = 10_000;

/// A prefix argument as `C-u`, `M--` and digits build it (ADR 0058). The dispatcher
/// holds it between keys, and these rules are Emacs's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    /// `C-u` alone: 4, and 4 times more with each one after it. A command may treat
    /// it apart from a number, as `C-u C-l` does.
    Universal(isize),
    /// `M--` or `C-u -` alone: -1, and digits make it a negative number.
    Negative,
    /// A number, which more digits go on to build.
    Number(isize),
    /// A number that `C-u` ended: digits after it are text, so that `C-u 5 C-u 3`
    /// types five 3s.
    Closed(isize),
}

impl Prefix {
    /// The number a command reads.
    pub const fn value(self) -> isize {
        match self {
            Self::Universal(n) | Self::Number(n) | Self::Closed(n) => n,
            Self::Negative => -1,
        }
    }

    /// Whether plain digits still build it.
    pub const fn reads_digits(self) -> bool {
        !matches!(self, Self::Closed(_))
    }

    /// Whether a plain `-` still builds it. After digits it is text, as in Emacs:
    /// `C-u 5 -` types five of them, and `M--` changes the number's sign.
    pub const fn reads_minus(self) -> bool {
        matches!(self, Self::Universal(_) | Self::Negative)
    }

    /// `current` after `C-u`. Counts saturate, since commands limit their work.
    pub const fn universal(current: Option<Self>) -> Self {
        match current {
            None => Self::Universal(4),
            Some(Self::Universal(n)) => Self::Universal(n.saturating_mul(4)),
            Some(Self::Negative) => Self::Universal(-4),
            Some(Self::Number(n)) => Self::Closed(n),
            Some(Self::Closed(_)) => Self::Universal(4),
        }
    }

    /// `current` after `M--` or `-`. A second one in a row cancels the first, and
    /// after a number it changes the sign.
    pub const fn negative(current: Option<Self>) -> Option<Self> {
        match current {
            None | Some(Self::Universal(_)) => Some(Self::Negative),
            Some(Self::Negative) => None,
            Some(Self::Number(n) | Self::Closed(n)) => Some(Self::Number(n.saturating_neg())),
        }
    }

    /// `current` after a digit. `-0` stays negative, so that `-05` is -5.
    pub const fn digit(current: Option<Self>, digit: u32) -> Self {
        let digit = digit as isize;
        match current {
            Some(Self::Number(n) | Self::Closed(n)) if n < 0 => {
                Self::Number(n.saturating_mul(10).saturating_sub(digit))
            }
            Some(Self::Number(n) | Self::Closed(n)) => {
                Self::Number(n.saturating_mul(10).saturating_add(digit))
            }
            Some(Self::Negative) if digit == 0 => Self::Negative,
            Some(Self::Negative) => Self::Number(-digit),
            None | Some(Self::Universal(_)) => Self::Number(digit),
        }
    }
}

/// The arguments one invocation of a command gets. Built by the caller:
/// the key dispatcher, `M-x` or `run-command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Args {
    prefix: Option<Prefix>,
}

impl Args {
    /// No arguments.
    pub const NONE: Self = Self { prefix: None };

    /// A numeric prefix argument of `n`.
    pub const fn numeric(n: isize) -> Self {
        Self {
            prefix: Some(Prefix::Number(n)),
        }
    }

    /// The prefix argument as the keys built it.
    pub const fn with_prefix(prefix: Option<Prefix>) -> Self {
        Self { prefix }
    }

    /// The prefix argument as the keys built it, for the commands that build on it
    /// and those that treat `C-u` apart from a number.
    pub const fn raw(self) -> Option<Prefix> {
        self.prefix
    }

    /// The numeric prefix argument, or `None` if none was given.
    pub const fn prefix(self) -> Option<isize> {
        match self.prefix {
            Some(prefix) => Some(prefix.value()),
            None => None,
        }
    }

    /// The repeat count: the numeric prefix argument, or 1.
    pub const fn count(self) -> isize {
        match self.prefix {
            Some(prefix) => prefix.value(),
            None => 1,
        }
    }
}

/// What a command asks the host to do once it returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Effects {
    /// Something visible changed: draw a new frame.
    pub redraw: bool,
    /// Exit the editor.
    pub exit: bool,
    /// The prefix argument the next command gets. The dispatcher takes this and
    /// holds it next to the pending prefix keys, so the host never sees it.
    pub prefix: Option<Prefix>,
}

impl Effects {
    /// Both sets of requests.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            redraw: self.redraw || other.redraw,
            exit: self.exit || other.exit,
            prefix: match other.prefix {
                Some(prefix) => Some(prefix),
                None => self.prefix,
            },
        }
    }
}

/// How a command was invoked, from what the caller knows. Its `Debug` leaves out the text
/// and the answer, which may be a passphrase.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The text that invoked it: the character a key types, or text from an input
    /// method.
    pub input: &'a str,
    /// The character on the key that invoked it, whatever modifiers are held: `5`
    /// for `M-5`.
    pub key_char: Option<char>,
    /// How many times in a row this command has just run before now.
    pub repeats: usize,
    /// What a prompt read for the command, as its [`ArgSpec`] asked (ADR 0072).
    pub answer: Option<&'a str>,
    /// What a second prompt read, for [`ArgSpec::Replace`] (ADR 0113),
    /// [`ArgSpec::DirectoryAndTextOnPrefix`] and, after a prefix argument, the base of
    /// [`ArgSpec::BranchAndBaseOnPrefix`] (ADR 0121).
    pub second: Option<&'a str>,
    /// It runs because the person answered `y` to a question it asked.
    pub confirmed: bool,
    /// Its key had Shift, which its binding has not (Emacs's shift translation).
    pub shifted: bool,
    /// What the OS clipboard held, for a command that waited for it.
    pub clipboard: Option<&'a FromClipboard>,
    /// The names of the switches that are on in the open menu whose head ran it (ADR 0117).
    /// Empty for a command invoked in any other way.
    pub switches: &'a [&'a str],
}

impl fmt::Debug for Invocation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Invocation")
            .field("repeats", &self.repeats)
            .field("confirmed", &self.confirmed)
            .field("shifted", &self.shifted)
            .field("switches", &self.switches)
            .finish_non_exhaustive()
    }
}

/// What a command gets to work with: the host's editor state, how it was
/// invoked, and a place to leave requests for the host.
///
/// New state reaches commands as new fields of [`Editor`] or new methods
/// here, so existing commands keep compiling.
pub struct Ctx<'a> {
    /// The editor state.
    pub editor: &'a mut Editor,
    invocation: Invocation<'a>,
    effects: Effects,
}

impl<'a> Ctx<'a> {
    /// A context over `editor`, with no input and no requests yet.
    pub fn new(editor: &'a mut Editor) -> Self {
        Self::invoked(editor, Invocation::default())
    }

    /// A context over `editor` for a command invoked as `invocation` says.
    pub fn invoked(editor: &'a mut Editor, invocation: Invocation<'a>) -> Self {
        Self {
            editor,
            invocation,
            effects: Effects::default(),
        }
    }

    /// The text that invoked the command: the character a key types, or text
    /// from an input method. Empty for a command invoked in any other way.
    pub fn input(&self) -> &'a str {
        self.invocation.input
    }

    /// The character on the key that invoked the command, whatever modifiers are
    /// held. `None` for a command invoked in any other way.
    pub fn key_char(&self) -> Option<char> {
        self.invocation.key_char
    }

    /// How many times in a row the command has just run before now, for a command
    /// that does something else when repeated, as `C-l` does. It keeps no state.
    pub fn repeats(&self) -> usize {
        self.invocation.repeats
    }

    /// What a prompt read for the command, as its [`ArgSpec`] asked, or what `run-command`
    /// gave in its place. `None` for a command that reads nothing.
    pub fn answer(&self) -> Option<&'a str> {
        self.invocation.answer
    }

    /// The string to replace and its replacement, read for a command whose [`ArgSpec`] is
    /// `Replace`: both empty for an empty first answer, which takes the last pair.
    pub fn replace_pair(&self) -> Option<(&'a str, &'a str)> {
        Some((self.answer()?, self.invocation.second.unwrap_or_default()))
    }

    /// The directory and the text two prompts read for a command whose [`ArgSpec`] is
    /// `DirectoryAndTextOnPrefix`, each empty if the person gave none (ADR 0121). `None` when
    /// no prompt read them: with no prefix argument.
    pub fn directory_and_text(&self) -> Option<(&'a str, &'a str)> {
        Some((self.answer()?, self.invocation.second.unwrap_or_default()))
    }

    /// The branch and, after a prefix argument, the base an `ArgSpec::BranchAndBaseOnPrefix`
    /// read (ADR 0121); `None` when nothing was read. An empty base stands for none.
    pub fn branch_and_base(&self) -> Option<(&'a str, Option<&'a str>)> {
        let base = self.invocation.second.filter(|base| !base.is_empty());
        Some((self.answer()?, base))
    }

    /// The whole number a prompt read for a command whose [`ArgSpec`] is `Number`, parsed as
    /// the prompt checked it. `None` for a command that reads none.
    pub fn number(&self) -> Option<i64> {
        self.answer().and_then(crate::minibuffer::parse_number)
    }

    /// Whether the command runs because the person answered `y` to a question it asked
    /// (ADR 0072). It checks again what is true now: the answer agrees only to what the
    /// question said.
    pub fn confirmed(&self) -> bool {
        self.invocation.confirmed
    }

    /// Whether the key that invoked the command had a Shift that its binding has
    /// not, as `S-<right>` runs `forward-char`. A motion then selects (ADR 0071).
    pub fn shifted(&self) -> bool {
        self.invocation.shifted
    }

    /// What the OS clipboard held, for a command whose [`ArgSpec`] reads it. `None` when
    /// it was not read: another spec, a numeric prefix, or a caller with no clipboard.
    pub fn clipboard(&self) -> Option<&'a FromClipboard> {
        self.invocation.clipboard
    }

    /// Whether the switch `name` is on in the open menu whose head ran the command, as
    /// transient's suffixes read its arguments (ADR 0117). Off for a command invoked in any
    /// other way, the agent channel's `run` among them.
    pub fn switch(&self, name: &str) -> bool {
        self.invocation.switches.contains(&name)
    }

    /// Leaves `prefix` for the next command, in place of the one this command got.
    pub fn leave_prefix(&mut self, prefix: Option<Prefix>) {
        self.effects.prefix = prefix;
    }

    /// Asks for a new frame. Call it when the command changed something
    /// visible, and only then.
    pub fn request_redraw(&mut self) {
        self.effects.redraw = true;
    }

    /// Asks the host to exit the editor once the command returns.
    pub fn request_exit(&mut self) {
        self.effects.exit = true;
    }

    /// What the commands run in this context asked for.
    pub fn effects(&self) -> Effects {
        self.effects
    }
}

/// A command that failed. The host says its message with [`Editor::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    message: Cow<'static, str>,
}

impl CommandError {
    /// An error with `message`, such as `"Beginning of buffer"`.
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandError {}

impl From<EditError> for CommandError {
    fn from(error: EditError) -> Self {
        Self::new(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ArgSpec, Args, Command, CommandError, CommandId, Ctx, Effects, ExecuteError, Invocation,
        Prefix, Reach, RegisterError, Registry,
    };
    use crate::editor::Editor;
    use crate::window::Scroll;
    use proptest::prelude::*;

    fn forward(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
        let (window, _) = ctx.editor.current_mut();
        let start = window.scroll().start.saturating_add_signed(args.count());
        window.set_scroll(Scroll {
            start,
            ..window.scroll()
        });
        ctx.request_redraw();
        Ok(())
    }

    fn fail(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        Err(CommandError::new("Beginning of buffer"))
    }

    fn explode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        let (window, _) = ctx.editor.current_mut();
        window.set_scroll(Scroll {
            start: 7,
            ..window.scroll()
        });
        panic!("deliberate panic quoting SECRET-MARKER");
    }

    fn quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        ctx.request_exit();
        Ok(())
    }

    const fn command(name: &'static str, run: super::CommandFn) -> Command {
        Command {
            name,
            doc: "A test command.",
            args: ArgSpec::Count,
            reach: Reach::Look,
            run,
            state: None,
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        registry
            .register_all(&[
                command("forward", forward),
                command("fail", fail),
                command("explode", explode),
                command("quit", quit),
            ])
            .unwrap();
        registry
    }

    fn run(
        registry: &Registry,
        editor: &mut Editor,
        name: &str,
        args: Args,
    ) -> (Result<(), ExecuteError>, Effects) {
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        let result = registry.execute(id, &mut ctx, args);
        (result, ctx.effects())
    }

    #[test]
    fn effects_union_keeps_each_request() {
        let redraw = Effects {
            redraw: true,
            ..Effects::default()
        };
        let exit = Effects {
            exit: true,
            ..Effects::default()
        };
        assert_eq!(
            redraw.union(exit),
            Effects {
                redraw: true,
                exit: true,
                prefix: None,
            }
        );
        assert_eq!(redraw.union(Effects::default()), redraw);
        assert_eq!(Effects::default().union(exit), exit);
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            CommandError::new("End of buffer").message(),
            "End of buffer"
        );
        assert_eq!(
            RegisterError::Duplicate("forward").to_string(),
            "command \"forward\" is registered twice"
        );
    }

    #[test]
    fn edit_errors_become_messages_that_quote_no_text() {
        let error = CommandError::from(crate::buffer::EditError::NoUndo);
        assert_eq!(error.message(), "No further undo information");
    }

    #[test]
    fn a_context_carries_the_input_it_was_made_with() {
        let mut editor = Editor::default();
        assert_eq!(Ctx::new(&mut editor).input(), "");
        let invocation = Invocation {
            input: "語",
            ..Invocation::default()
        };
        assert_eq!(Ctx::invoked(&mut editor, invocation).input(), "語");
    }

    #[test]
    fn a_context_reads_the_switches_its_menu_had_on() {
        let mut editor = Editor::default();
        let on = ["files", "other-style"];
        let invocation = Invocation {
            switches: &on,
            ..Invocation::default()
        };
        let ctx = Ctx::invoked(&mut editor, invocation);
        assert!(ctx.switch("files"));
        assert!(ctx.switch("other-style"));
        assert!(!ctx.switch("file"));
        // Any other way of invoking a command has every switch off.
        let mut editor = Editor::default();
        assert!(!Ctx::new(&mut editor).switch("files"));
    }

    #[test]
    fn a_context_carries_how_the_command_was_invoked() {
        let mut editor = Editor::default();
        assert_eq!(Ctx::new(&mut editor).key_char(), None);
        let invocation = Invocation {
            key_char: Some('5'),
            repeats: 2,
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(&mut editor, invocation);
        assert_eq!((ctx.key_char(), ctx.repeats()), (Some('5'), 2));
        ctx.leave_prefix(Some(Prefix::Negative));
        assert_eq!(ctx.effects().prefix, Some(Prefix::Negative));
    }

    #[test]
    fn c_u_is_four_and_each_more_multiplies() {
        let once = Prefix::universal(None);
        assert_eq!(once, Prefix::Universal(4));
        assert_eq!(Prefix::universal(Some(once)), Prefix::Universal(16));
        assert_eq!(
            Prefix::universal(Some(Prefix::Universal(isize::MAX))),
            Prefix::Universal(isize::MAX),
            "it saturates"
        );
        assert_eq!(
            Prefix::universal(Some(Prefix::Negative)),
            Prefix::Universal(-4)
        );
    }

    #[test]
    fn digits_build_a_number_and_c_u_ends_it() {
        let five = Prefix::digit(Some(Prefix::Universal(4)), 5);
        assert_eq!(five, Prefix::Number(5), "a digit replaces what C-u means");
        let fifty_three = Prefix::digit(Some(five), 3);
        assert_eq!(fifty_three, Prefix::Number(53));
        assert_eq!(Prefix::digit(None, 7), Prefix::Number(7));
        assert_eq!(Prefix::digit(Some(Prefix::Number(0)), 7), Prefix::Number(7));
        let closed = Prefix::universal(Some(five));
        assert_eq!(closed, Prefix::Closed(5));
        assert!(!closed.reads_digits() && five.reads_digits());
        assert_eq!(Prefix::universal(Some(closed)), Prefix::Universal(4));
        assert!(!five.reads_minus() && !closed.reads_minus());
        assert!(Prefix::Negative.reads_minus() && Prefix::Universal(4).reads_minus());
        assert_eq!(Prefix::digit(Some(closed), 3), Prefix::Number(53), "M-3");
        assert_eq!(
            Prefix::digit(Some(Prefix::Number(isize::MAX)), 9),
            Prefix::Number(isize::MAX),
            "it saturates"
        );
        assert_eq!(
            Prefix::digit(Some(Prefix::Number(isize::MIN)), 9),
            Prefix::Number(isize::MIN),
        );
    }

    #[test]
    fn minus_makes_a_negative_number() {
        let minus = Prefix::negative(None).unwrap();
        assert_eq!((minus, minus.value()), (Prefix::Negative, -1));
        assert_eq!(
            Prefix::negative(Some(Prefix::Universal(4))),
            Some(Prefix::Negative)
        );
        assert_eq!(Prefix::negative(Some(minus)), None, "two cancel");
        assert_eq!(Prefix::digit(Some(minus), 5), Prefix::Number(-5));
        assert_eq!(Prefix::digit(Some(minus), 0), Prefix::Negative, "-0");
        assert_eq!(
            Prefix::digit(Some(Prefix::Number(-5)), 3),
            Prefix::Number(-53)
        );
        assert_eq!(
            Prefix::negative(Some(Prefix::Number(5))),
            Some(Prefix::Number(-5))
        );
        assert_eq!(
            Prefix::negative(Some(Prefix::Number(isize::MIN))),
            Some(Prefix::Number(isize::MAX))
        );
    }

    #[test]
    fn args_read_a_prefix_as_its_number() {
        let args = Args::with_prefix(Some(Prefix::Universal(16)));
        assert_eq!((args.count(), args.prefix()), (16, Some(16)));
        assert_eq!(args.raw(), Some(Prefix::Universal(16)));
        assert_eq!(Args::with_prefix(Some(Prefix::Negative)).count(), -1);
        assert_eq!(Args::with_prefix(None), Args::NONE);
    }

    #[test]
    fn looks_up_commands_by_name() {
        let registry = registry();
        let id = registry.lookup("forward").unwrap();
        assert_eq!(registry.get(id).unwrap().name, "forward");
        assert_eq!(registry.lookup("backward"), None);
    }

    #[test]
    fn iterates_in_registration_order_with_docs() {
        let registry = registry();
        let names: Vec<_> = registry.iter().map(|(_, c)| c.name).collect();
        assert_eq!(names, ["forward", "fail", "explode", "quit"]);
        for (id, command) in registry.iter() {
            assert_eq!(registry.lookup(command.name), Some(id));
            assert_eq!(command.doc, "A test command.");
        }
    }

    #[test]
    fn rejects_duplicate_names() {
        let mut registry = registry();
        assert_eq!(
            registry.register(command("forward", fail)),
            Err(RegisterError::Duplicate("forward"))
        );
        // The first registration is kept.
        let mut editor = Editor::default();
        assert_eq!(run(&registry, &mut editor, "forward", Args::NONE).0, Ok(()));
        assert_eq!(registry.iter().len(), 4);
    }

    #[test]
    fn rejects_bad_names_and_missing_docs() {
        let mut registry = Registry::new();
        for bad in [
            "",
            "Forward",
            "forward_char",
            "-forward",
            "forward-",
            "a--b",
            "forward char",
        ] {
            assert_eq!(
                registry.register(command(bad, forward)),
                Err(RegisterError::BadName(bad)),
                "{bad:?}"
            );
        }
        let undocumented = Command {
            doc: " \n",
            ..command("undocumented", forward)
        };
        assert_eq!(
            registry.register(undocumented),
            Err(RegisterError::MissingDoc("undocumented"))
        );
        assert!(registry.register(command("forward-char2", forward)).is_ok());
    }

    #[test]
    fn executes_with_arguments_and_reports_effects() {
        let registry = registry();
        let mut editor = Editor::default();
        let (result, effects) = run(&registry, &mut editor, "forward", Args::NONE);
        assert_eq!(result, Ok(()));
        assert_eq!(editor.window().scroll().start, 1);
        assert!(effects.redraw && !effects.exit);
        assert_eq!(
            run(&registry, &mut editor, "forward", Args::numeric(5)).0,
            Ok(())
        );
        assert_eq!(editor.window().scroll().start, 6);
        assert_eq!(
            run(&registry, &mut editor, "forward", Args::numeric(-2)).0,
            Ok(())
        );
        assert_eq!(editor.window().scroll().start, 4);

        let (_, effects) = run(&registry, &mut editor, "quit", Args::NONE);
        assert!(effects.exit && !effects.redraw);
    }

    #[test]
    fn reports_command_errors() {
        let registry = registry();
        let mut editor = Editor::default();
        let (result, _) = run(&registry, &mut editor, "fail", Args::NONE);
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "fail: Beginning of buffer");
    }

    #[test]
    fn catches_a_panicking_command_and_keeps_going() {
        let registry = registry();
        let mut editor = Editor::default();
        let (result, _) = run(&registry, &mut editor, "explode", Args::NONE);
        assert_eq!(result, Err(ExecuteError::Panicked { command: "explode" }));
        // The panic message may quote buffer text, so it is not kept.
        let shown = format!("{} {:?}", result.as_ref().unwrap_err(), result);
        assert!(!shown.contains("SECRET-MARKER"), "{shown}");
        // The partial update stays (ADR 0014), and later commands still run.
        assert_eq!(editor.window().scroll().start, 7);
        assert_eq!(run(&registry, &mut editor, "forward", Args::NONE).0, Ok(()));
        assert_eq!(editor.window().scroll().start, 8);
    }

    #[test]
    fn an_id_from_another_registry_is_an_error() {
        let registry = registry();
        let mut editor = Editor::default();
        let mut ctx = Ctx::new(&mut editor);
        let foreign = CommandId(99);
        assert_eq!(
            registry.execute(foreign, &mut ctx, Args::NONE),
            Err(ExecuteError::UnknownId(foreign))
        );
    }

    #[test]
    fn args_default_to_a_count_of_one() {
        assert_eq!(Args::NONE.count(), 1);
        assert_eq!(Args::NONE.prefix(), None);
        assert_eq!(Args::numeric(-3).count(), -3);
        assert_eq!(Args::numeric(0).prefix(), Some(0));
    }

    proptest! {
        /// Every registered name can be looked up and maps back to itself,
        /// and registering a name twice always fails.
        #[test]
        fn prop_names_round_trip(names in prop::collection::hash_set("[a-z]{1,6}(-[a-z0-9]{1,4}){0,2}", 0..20)) {
            let names: Vec<&'static str> = names.into_iter().map(|n| &*Box::leak(n.into_boxed_str())).collect();
            let mut registry = Registry::new();
            for name in &names {
                registry.register(command(name, forward)).unwrap();
            }
            for name in &names {
                let id = registry.lookup(name).unwrap();
                prop_assert_eq!(registry.get(id).unwrap().name, *name);
                prop_assert_eq!(registry.register(command(name, fail)), Err(RegisterError::Duplicate(name)));
            }
            prop_assert_eq!(registry.iter().len(), names.len());
        }
    }
}
