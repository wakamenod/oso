//! The editor's commands (ADR 0034). Each is a plain function over the
//! host-owned state in `Ctx`, registered with a docstring and an argument
//! spec. Each feature has a module here with its own table of commands, and
//! [`TABLES`] lists them. Default key bindings are in `bindings`.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

#[cfg(feature = "dev")]
mod dev_tools;
mod edit;
mod mark;
mod messages;
mod minibuffer;
mod motion;
mod prefix;
mod recovery;
mod save;
mod scroll;
mod session;

#[cfg(test)]
pub(crate) use messages::kept_messages;
use oso_core::command::{Command, CommandError, Ctx, Registry};
use oso_core::motion::{TOO_MUCH, WalkBudget};

/// Every feature's commands. A new feature adds a module and one line here, in
/// alphabetical order and never joined with another, so that features added at
/// the same time rarely touch the same line. This is only the order they
/// register in: a list shown to the user decides its own order.
#[rustfmt::skip]
const TABLES: &[&[Command]] = &[
    #[cfg(feature = "dev")]
    dev_tools::COMMANDS,
    edit::COMMANDS,
    mark::COMMANDS,
    messages::COMMANDS,
    minibuffer::COMMANDS,
    motion::COMMANDS,
    prefix::COMMANDS,
    recovery::COMMANDS,
    save::COMMANDS,
    scroll::COMMANDS,
    session::COMMANDS,
];

/// Refuses a command that would change which buffer a window shows, while a prompt is
/// selected: its window always shows its own (ADR 0072).
fn not_in_prompt(ctx: &Ctx<'_>) -> Result<(), CommandError> {
    match ctx.editor.prompt() {
        Some(_) => Err(CommandError::new("Not in the minibuffer")),
        None => Ok(()),
    }
}

/// The signal for a command whose budget ran out, after it did what it could.
fn budget_left(budget: WalkBudget) -> Result<(), CommandError> {
    if budget.is_spent() {
        return Err(CommandError::new(TOO_MUCH));
    }
    Ok(())
}

/// A registry with every command in [`TABLES`].
///
/// # Panics
///
/// If a command is registered twice or lacks a docstring. The tests build
/// this registry, so such a mistake cannot ship.
pub fn registry() -> Registry {
    let mut registry = Registry::new();
    for table in TABLES {
        if let Err(e) = registry.register_all(table) {
            panic!("{e}");
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::{TABLES, registry};
    use oso_core::buffer::Buffer;
    use oso_core::command::{Args, Ctx, Effects, ExecuteError};
    use oso_core::editor::Editor;
    use oso_core::window::ViewSize;

    pub(super) fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    pub(super) fn run(
        editor: &mut Editor,
        name: &str,
        args: Args,
    ) -> (Result<(), String>, Effects) {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::new(editor);
        let result = registry.execute(id, &mut ctx, args).map_err(|e| match e {
            ExecuteError::Failed { error, .. } => error.message().to_owned(),
            other => panic!("{other}"),
        });
        (result, ctx.effects())
    }

    pub(super) fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    #[cfg(not(feature = "dev"))]
    #[test]
    fn a_build_without_the_dev_tools_has_no_dev_commands() {
        assert_eq!(registry().lookup("dev-panic"), None);
    }

    #[test]
    fn every_command_registers() {
        let commands: usize = TABLES.iter().map(|table| table.len()).sum();
        assert_eq!(registry().iter().len(), commands);
    }
}
