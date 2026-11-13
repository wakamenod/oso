//! The command that opens a menu by name (ADR 0117).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.
//!
//! The dispatcher reads the menu's name in a prompt that completes over the menus in force, and
//! opens it in the command's place, as the open menus are its own; `open-menu` itself only
//! names what it reads.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands of menus.
pub const COMMANDS: &[Command] = &[Command {
    name: "open-menu",
    doc: "Read a menu's name, with completion, and open it.\n\nThe prefix argument given before goes to the first head run in it. A menu's key opens it too.",
    args: ArgSpec::Menu {
        prompt: "Menu: ",
        history: "menu",
    },
    // A menu reads keys, so only a person opens one (ADR 0117, Q22).
    reach: Reach::Person,
    run: open_menu,
    state: None,
}];

fn open_menu(_: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    Err(CommandError::new("Open a menu through its key or M-x"))
}
