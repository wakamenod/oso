//! Commands the host runs on its own: `space-new` with no repository to start from opens
//! `space-add-repo`'s prompt, as its keys would (ADR 0136, step 1b).
//!
//! Must not depend on: `app` or winit; the host names the command.

use oso_core::command::{Args, Effects, Invocation};
use oso_core::editor::Editor;

use super::Dispatcher;

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
}
