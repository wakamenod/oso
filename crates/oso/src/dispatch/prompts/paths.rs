//! The prompts that read a path, for a command's argument: `C-x C-f`'s and `C-x C-w`'s by its
//! segments (ADR 0087), and a project's files, listed as they are walked (ADR 0114). The mark
//! list shares their way in, as it too lists before it reads.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{ArgSpec, Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptError;

use super::super::{Dispatcher, Step};
use super::Reading;

impl Dispatcher {
    /// Opens the prompt for the command `id` that lists what `spec` reads: a mark, a path by its
    /// segments, or a project's file.
    pub(super) fn read_listed(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        spec: ArgSpec,
    ) -> (Effects, Result<(), ExecuteError>) {
        match spec {
            ArgSpec::Mark { prompt } => self.read_mark(editor, id, prefix, prompt),
            ArgSpec::File { prompt, history } => {
                self.read_file(editor, id, prefix, prompt, history)
            }
            ArgSpec::ProjectFile { history } => self.read_project_file(editor, id, prefix, history),
            _ => self.execute(
                editor,
                id,
                Args::with_prefix(prefix),
                Invocation::default(),
                Step::MayJoin,
            ),
        }
    }

    /// Opens the prompt that reads a path for the command `id`, starting at the shown buffer's
    /// directory, or the one oso started in, unless a prompt is selected (ADR 0087).
    fn read_file(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        prompt: &'static str,
        history: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        let start = editor.start_directory().to_path_buf();
        let directory = editor
            .current()
            .1
            .path()
            .and_then(std::path::Path::parent)
            .map_or(start, std::path::Path::to_path_buf);
        let initial = oso_core::complete::typed_form(&directory, true, editor.home());
        // A relative path typed in place of it starts there too.
        match editor.open_file_prompt(prompt, history, &initial, directory) {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: None,
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }

    /// Opens the prompt that lists the files of the current buffer's project for the command
    /// `id`, or of its directory outside one, unless a prompt is selected (ADR 0114). The host
    /// walks the project, and the list grows as it does. The history is the spec's, one for each
    /// project.
    fn read_project_file(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        history: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        let (root, name) = match editor.project_files_root() {
            Ok(found) => found,
            Err(not) => {
                editor.message(not.to_string());
                return (effects, Ok(()));
            }
        };
        let utf8 = root.to_str().is_some();
        match editor.open_project_files(root, &name, history) {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: None,
            }),
            Err(PromptError::Invalid) if !utf8 => {
                editor.message("Cannot list a project whose path is not UTF-8");
            }
            Err(PromptError::Invalid) => {
                editor.message("A project's files are listed already");
            }
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }
}
