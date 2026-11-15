//! Commands the host runs on its own: `space-new` with no repository to start from opens
//! `space-add-repo`'s prompt, as its keys would (ADR 0136, step 1b), and an exit that waited
//! for the terminals runs once they are idle (ADR 0133, step 5).
//!
//! Must not depend on: `app` or winit; the host names the command.

use oso_core::command::{Args, Effects, Invocation};
use oso_core::editor::Editor;

use super::Dispatcher;
use super::hold_all::holds_input;

impl Dispatcher {
    /// Runs the command `name` as its keys would, opening its prompt first when it reads
    /// something. Nothing runs while a prompt is selected.
    pub fn run_from_host(&mut self, editor: &mut Editor, name: &str) -> Effects {
        if editor.prompt().is_some() {
            return Effects::default();
        }
        let Some(id) = self.registry.lookup(name) else {
            log::error!("the host ran no command's name: {name}");
            return Effects::default();
        };
        self.run_or_read(editor, id, Args::NONE, Invocation::default())
            .0
    }

    /// Runs `save-buffers-kill-terminal` confirmed, as `y` to its question would, for an exit
    /// that waited until no terminal was busy, and it exits. If more is at risk than the person
    /// agreed to lose, it asks nothing, as no question opens from a background event
    /// (ADR 0072): the wait stops, and the echo area says why. `None` while a prompt is open
    /// or input is held (`!`, an undo), when it runs nothing and the host tries again later.
    pub fn exit_when_idle(&mut self, editor: &mut Editor) -> Option<Effects> {
        if editor.prompt().is_some() || holds_input(editor) {
            return None;
        }
        if let Err(refused) = editor.confirm_exit(true) {
            // The next `C-x C-c` asks about all of it again.
            editor.reset_exit_guard();
            editor.echo(format!(
                "Not quit, as more is at risk now: {refused}; C-x C-c asks"
            ));
            return Some(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        let Some(id) = self.registry.lookup("save-buffers-kill-terminal") else {
            log::error!("no save-buffers-kill-terminal to exit with");
            return Some(Effects::default());
        };
        let invocation = Invocation {
            confirmed: true,
            ..Invocation::default()
        };
        let effects = self
            .execute(editor, id, Args::NONE, invocation, super::Step::MayJoin)
            .0;
        Some(effects.union(self.after_prompts(editor)))
    }
}
