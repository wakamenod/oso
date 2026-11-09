//! The registry of commands (ADR 0034): each by name and by id, registering one only with a
//! well-formed name and a docstring, and running one under `catch_unwind`, so that a command
//! that panics costs one error, not the session (ADR 0014).
//!
//! Must not depend on: `keymap`, key presses or the concrete commands, which the tip crate
//! registers.

use std::collections::HashMap;
use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use super::{Args, Command, CommandError, Ctx};

/// Identifies a command in the [`Registry`] that returned it. Keymaps store
/// these, so dispatching a key needs no name lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub(super) u32);

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
