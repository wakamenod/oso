//! Commands that build the numeric prefix argument: `C-u`, `M--` and digits
//! (ADR 0034, ADR 0058).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! Each takes the prefix so far as its arguments, and leaves the new one for the next
//! command. The rules are in `oso_core::command::Prefix`, and the dispatcher holds
//! the prefix between keys. `C-g` needs nothing here: it is a command, so the prefix
//! goes to it, which ignores it.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Prefix, Reach};

/// The prefix argument commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "universal-argument",
        doc: "Begin a numeric argument for the next command: 4, and 4 times more with each repeat.\n\nDigits and `-` after it make another number. After digits it ends them, so that `C-u 5 C-u 3` inserts five 3s, and a further one starts over.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: universal_argument,
    },
    Command {
        name: "digit-argument",
        doc: "Add the digit on the key that ran this to the numeric argument for the next command.\n\nOnce it has begun, plain digits add to it too. It has to come from a key.",
        args: ArgSpec::Key,
        reach: Reach::Person,
        run: digit_argument,
    },
    Command {
        name: "negative-argument",
        doc: "Begin a negative numeric argument for the next command, -1 until digits follow.\n\nAfter a number, change its sign. A second one in a row cancels the first.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: negative_argument,
    },
];

fn universal_argument(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ctx.leave_prefix(Some(Prefix::universal(args.raw())));
    Ok(())
}

fn digit_argument(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let Some(digit) = ctx.key_char().and_then(|c| c.to_digit(10)) else {
        return Err(CommandError::new("digit-argument must be on a digit key"));
    };
    ctx.leave_prefix(Some(Prefix::digit(args.raw(), digit)));
    Ok(())
}

fn negative_argument(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ctx.leave_prefix(Prefix::negative(args.raw()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::command::{Args, Prefix};

    #[test]
    fn each_leaves_a_prefix_for_the_next_command_and_draws_nothing() {
        let mut editor = editor("abc");
        let (result, effects) = run(&mut editor, "universal-argument", Args::NONE);
        assert_eq!(result, Ok(()));
        assert_eq!(effects.prefix, Some(Prefix::Universal(4)));
        assert!(!effects.redraw);
        let (_, effects) = run(&mut editor, "negative-argument", Args::NONE);
        assert_eq!(effects.prefix, Some(Prefix::Negative));
        let args = Args::with_prefix(Some(Prefix::Negative));
        let (_, effects) = run(&mut editor, "negative-argument", args);
        assert_eq!(effects.prefix, None);
    }

    #[test]
    fn a_digit_needs_a_digit_key() {
        let mut editor = editor("abc");
        let (result, effects) = run(&mut editor, "digit-argument", Args::NONE);
        assert_eq!(result, Err("digit-argument must be on a digit key".into()));
        assert_eq!(effects.prefix, None);
    }
}
