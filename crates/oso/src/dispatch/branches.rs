//! A command that reads a branch of the current project (`ArgSpec::BranchAndBaseOnPrefix`,
//! ADR 0121): it waits while the host lists the project's branches off the UI thread, then
//! reads the branch in a prompt that completes over them, and after a prefix argument a base in
//! a second prompt over the same list.
//!
//! Must not depend on: `app`, git or the sessions worker; the host lists the branches and
//! brings the answer.
//!
//! The wait is the clipboard's (ADR 0074) made simpler: no input is held, so keys typed while
//! the branches are listed run as they come. If one of them opens a prompt or selects another
//! window or buffer before the list comes, the wait is dropped and a message says so, as its
//! prompt would open somewhere else. Each prompt takes any name typed, as `C-x b` does
//! (ADR 0088): a new branch, or a revision for the base.

use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, CommandId, Effects, ExecuteError, Prefix};
use oso_core::complete::Candidate;
use oso_core::editor::{CompletionStyle, Editor};

use super::Dispatcher;
use super::prompts::Reading;

/// The branches in the order the host listed them, and any name typed is an answer.
const STYLE: CompletionStyle = CompletionStyle {
    given_order: true,
    takes_input: true,
    preview: false,
};

/// A command waiting for the host to list the current project's branches.
pub(super) struct BranchWait {
    id: CommandId,
    prefix: Option<Prefix>,
    place: Place,
    /// The host has been asked to list them.
    asked: bool,
}

/// Where the command was when it began waiting, to drop it if that changed.
#[derive(PartialEq, Eq)]
struct Place {
    window: usize,
    buffer: BufferId,
    prompts: usize,
}

impl Place {
    fn of(editor: &Editor) -> Self {
        Self {
            window: editor.selected_window(),
            buffer: editor.window().buffer(),
            prompts: editor.prompts().len(),
        }
    }
}

/// The base prompt that follows the branch's after a prefix argument, over the same branches.
#[derive(Clone)]
pub(super) struct BasePrompt {
    prompt: &'static str,
    history: &'static str,
    branches: Arc<[Candidate]>,
}

/// The effects of showing a prompt or a message: a frame.
fn redraw() -> Effects {
    Effects {
        redraw: true,
        ..Effects::default()
    }
}

impl Dispatcher {
    /// Keeps the command `id`, run with `prefix`, waiting for the host to list the branches,
    /// unless a prompt is selected, whose window always shows its own buffer. A wait already
    /// under way is dropped for this one.
    pub(super) fn wait_for_branches(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
    ) -> (Effects, Result<(), ExecuteError>) {
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (redraw(), Ok(()));
        }
        self.branch_wait = Some(BranchWait {
            id,
            prefix,
            place: Place::of(editor),
            asked: false,
        });
        (Effects::default(), Ok(()))
    }

    /// Whether the host should list the current project's branches for a command that waits:
    /// once per wait.
    pub fn take_branches_request(&mut self) -> bool {
        match self.branch_wait.as_mut().filter(|wait| !wait.asked) {
            Some(wait) => {
                wait.asked = true;
                true
            }
            None => false,
        }
    }

    /// Opens the branch prompt for the command that waited, completing over `branches` and
    /// taking any name typed, unless a prompt opened or the selected window changed meanwhile,
    /// which a message then says it was dropped for. `Err(why)` says why in the echo area and
    /// drops the wait. Returns what the host should do.
    pub fn branches_answered(
        &mut self,
        editor: &mut Editor,
        branches: Result<Vec<String>, String>,
    ) -> Effects {
        let Some(wait) = self.branch_wait.take() else {
            return Effects::default();
        };
        let spec = self.registry.get(wait.id).map(|command| command.args);
        let Some(ArgSpec::BranchAndBaseOnPrefix {
            prompt,
            base_prompt,
            history,
        }) = spec
        else {
            log::error!("a branch wait for a command that reads no branch");
            return Effects::default();
        };
        if wait.place != Place::of(editor) {
            editor.message("Branch prompt dropped: a prompt opened or the window changed");
            return redraw();
        }
        let branches: Arc<[Candidate]> = match branches {
            Ok(branches) => branches
                .into_iter()
                .map(|branch| Candidate::new(branch, "", ""))
                .collect(),
            Err(why) => {
                editor.message(why);
                return redraw();
            }
        };
        match editor.open_completion_as(prompt, history, Arc::clone(&branches), STYLE) {
            Ok(opened) => {
                let base = wait.prefix.map(|_| BasePrompt {
                    prompt: base_prompt,
                    history,
                    branches,
                });
                let command = (wait.id, wait.prefix);
                self.reading.push(Reading::branch(command, opened, base));
            }
            Err(error) => editor.prompt_says(error.to_string()),
        }
        redraw()
    }

    /// Takes the branch read for `reading`, and opens its base prompt, which runs the command
    /// with both.
    pub(super) fn open_base_after(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        base: &BasePrompt,
        branch: String,
    ) -> Effects {
        let branches = Arc::clone(&base.branches);
        match editor.open_completion_as(base.prompt, base.history, branches, STYLE) {
            Ok(prompt) => self.reading.push(reading.base_after(prompt, branch)),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        redraw()
    }
}
