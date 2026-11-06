//! Commands only a dev build has, for the end-to-end flows (ADR 0062).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};

/// The dev commands.
pub const COMMANDS: &[Command] = &[Command {
    name: "dev-panic",
    doc: "Panic under the command boundary, quoting the buffer's text in the message.\n\nFor testing that the panic is caught, that nothing is saved, and that the text reaches no output.",
    args: ArgSpec::None,
    run: dev_panic,
}];

fn dev_panic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    panic!("a deliberate panic: {}", ctx.editor.current().1.text());
}

#[cfg(test)]
mod tests {
    use oso_core::command::{Args, Ctx, ExecuteError};

    use crate::commands::registry;
    use crate::commands::tests::editor;

    #[test]
    fn dev_panic_panics_at_the_command_boundary_and_the_error_quotes_nothing() {
        let mut editor = editor("text");
        let registry = registry();
        let id = registry.lookup("dev-panic").unwrap();
        let mut ctx = Ctx::new(&mut editor);
        let error = registry.execute(id, &mut ctx, Args::NONE).unwrap_err();
        assert_eq!(
            error,
            ExecuteError::Panicked {
                command: "dev-panic"
            }
        );
    }
}
