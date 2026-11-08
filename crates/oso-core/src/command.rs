//! The command registry (ADR 0034).
//!
//! Must not depend on: `keymap`, key presses or the concrete commands, which
//! the tip crate registers.
//!
//! A [`Command`] is a name, a docstring, an argument spec and a plain
//! function over host-owned state: `fn(&mut Ctx, Args) -> Result<(),
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
use std::collections::HashMap;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use crate::buffer::EditError;
use crate::editor::Editor;
use crate::kill::FromClipboard;

/// The function behind a command.
pub type CommandFn = fn(&mut Ctx<'_>, Args) -> Result<(), CommandError>;

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
    /// The function that runs it.
    pub run: CommandFn,
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Command")
            .field("name", &self.name)
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

/// Which arguments a command takes. It documents the command
/// (`describe-function`) and tells `run-command` what to accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgSpec {
    /// No arguments. A prefix argument, if given, is ignored, as in Emacs.
    None,
    /// A numeric prefix argument, read with [`Args::count`]: a repeat count
    /// that defaults to 1 and may be negative to reverse the direction.
    Count,
    /// A numeric prefix argument, and with none or only `C-u`, what the OS clipboard
    /// holds, read with [`Ctx::clipboard`]. The dispatcher reads the clipboard first, off
    /// the UI thread, and runs the command once it has the answer, as `yank` needs
    /// (ADR 0074).
    CountAndClipboard,
}

impl ArgSpec {
    /// Whether a command with this spec, given `prefix`, waits for the clipboard.
    pub const fn reads_clipboard(self, prefix: Option<Prefix>) -> bool {
        matches!(self, Self::CountAndClipboard)
            && matches!(prefix, None | Some(Prefix::Universal(_)))
    }
}

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

/// How a command was invoked, from what the caller knows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Invocation<'a> {
    /// The text that invoked it: the character a key types, or text from an input
    /// method.
    pub input: &'a str,
    /// The character on the key that invoked it, whatever modifiers are held: `5`
    /// for `M-5`.
    pub key_char: Option<char>,
    /// How many times in a row this command has just run before now.
    pub repeats: usize,
    /// Its key had Shift, which its binding has not (Emacs's shift translation).
    pub shifted: bool,
    /// What the OS clipboard held, for a command that waited for it.
    pub clipboard: Option<&'a FromClipboard>,
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

/// Identifies a command in the [`Registry`] that returned it. Keymaps store
/// these, so dispatching a key needs no name lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(u32);

/// Why [`Registry::execute`] did not run a command to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteError {
    /// The command returned an error.
    Failed {
        command: &'static str,
        error: CommandError,
    },
    /// The command panicked. State it touched may be partially updated
    /// (ADR 0014). The panic message is dropped: it may quote buffer text,
    /// which errors never carry (ADR 0028).
    Panicked { command: &'static str },
    /// The id does not belong to this registry.
    UnknownId(CommandId),
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed { command, error } => write!(f, "{command}: {error}"),
            Self::Panicked { command } => write!(f, "{command} panicked"),
            Self::UnknownId(id) => write!(f, "no command with id {}", id.0),
        }
    }
}

impl std::error::Error for ExecuteError {}

/// Why a command cannot be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterError {
    /// Another command already has this name.
    Duplicate(&'static str),
    /// The name is not lower-case kebab case (`[a-z0-9]+(-[a-z0-9]+)*`).
    BadName(&'static str),
    /// The docstring is empty.
    MissingDoc(&'static str),
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(name) => write!(f, "command {name:?} is registered twice"),
            Self::BadName(name) => write!(f, "command name {name:?} is not lower-case kebab case"),
            Self::MissingDoc(name) => write!(f, "command {name:?} has no docstring"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// All commands, by name and by id. Owned by the host; there is no global
/// registry.
#[derive(Debug, Default)]
pub struct Registry {
    commands: Vec<Command>,
    by_name: HashMap<&'static str, CommandId>,
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a command. Fails if its name is taken or malformed, or it has
    /// no docstring.
    #[expect(
        clippy::expect_used,
        reason = "each command is a static item in the binary, so there are far fewer than 2^32"
    )]
    pub fn register(&mut self, command: Command) -> Result<CommandId, RegisterError> {
        if !is_command_name(command.name) {
            return Err(RegisterError::BadName(command.name));
        }
        if command.doc.trim().is_empty() {
            return Err(RegisterError::MissingDoc(command.name));
        }
        if self.by_name.contains_key(command.name) {
            return Err(RegisterError::Duplicate(command.name));
        }
        let id = CommandId(u32::try_from(self.commands.len()).expect("fewer than 2^32 commands"));
        self.commands.push(command);
        self.by_name.insert(command.name, id);
        Ok(id)
    }

    /// Adds every command in `commands`, stopping at the first error.
    pub fn register_all(&mut self, commands: &[Command]) -> Result<(), RegisterError> {
        commands
            .iter()
            .try_for_each(|command| self.register(*command).map(drop))
    }

    /// The id of the command named `name`.
    pub fn lookup(&self, name: &str) -> Option<CommandId> {
        self.by_name.get(name).copied()
    }

    /// The command with `id`, if it belongs to this registry.
    pub fn get(&self, id: CommandId) -> Option<&Command> {
        self.commands.get(usize::try_from(id.0).ok()?)
    }

    /// All commands with their ids, in the order they were registered.
    #[expect(
        clippy::expect_used,
        reason = "`register` gives out every id, so each index fits in u32"
    )]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (CommandId, &Command)> {
        self.commands.iter().enumerate().map(|(i, command)| {
            (
                CommandId(u32::try_from(i).expect("ids fit in u32")),
                command,
            )
        })
    }

    /// Runs the command with `id` once, catching a panic at this boundary
    /// (ADR 0014).
    pub fn execute(
        &self,
        id: CommandId,
        ctx: &mut Ctx<'_>,
        args: Args,
    ) -> Result<(), ExecuteError> {
        let command = self.get(id).ok_or(ExecuteError::UnknownId(id))?;
        // Unwind safety: state a panicking command touched may be partially
        // updated; ADR 0014 accepts this, and the editor keeps running.
        match panic::catch_unwind(AssertUnwindSafe(|| (command.run)(ctx, args))) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ExecuteError::Failed {
                command: command.name,
                error,
            }),
            // The payload is dropped unread (ADR 0028).
            Err(_) => Err(ExecuteError::Panicked {
                command: command.name,
            }),
        }
    }
}

/// Whether `name` is lower-case kebab case: `[a-z0-9]+(-[a-z0-9]+)*`.
fn is_command_name(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|word| {
            !word.is_empty() && word.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9'))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        ArgSpec, Args, Command, CommandError, CommandId, Ctx, Effects, ExecuteError, Invocation,
        Prefix, RegisterError, Registry,
    };
    use crate::editor::Editor;
    use crate::window::Scroll;
    use proptest::prelude::*;

    fn forward(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
        let (window, _) = ctx.editor.current_mut();
        let top = window.scroll().top.saturating_add_signed(args.count());
        window.set_scroll(Scroll {
            top,
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
            top: 7,
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
            run,
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
        assert_eq!(editor.window().scroll().top, 1);
        assert!(effects.redraw && !effects.exit);
        assert_eq!(
            run(&registry, &mut editor, "forward", Args::numeric(5)).0,
            Ok(())
        );
        assert_eq!(editor.window().scroll().top, 6);
        assert_eq!(
            run(&registry, &mut editor, "forward", Args::numeric(-2)).0,
            Ok(())
        );
        assert_eq!(editor.window().scroll().top, 4);

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
        assert_eq!(editor.window().scroll().top, 7);
        assert_eq!(run(&registry, &mut editor, "forward", Args::NONE).0, Ok(()));
        assert_eq!(editor.window().scroll().top, 8);
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
