//! The prompts that read a path, for a command's argument: `C-x C-f`'s and `C-x C-w`'s by its
//! segments (ADR 0087), and a project's files, listed as they are walked, or a line of them,
//! searched for (ADR 0114). The mark
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
            ArgSpec::ProjectFile { .. } | ArgSpec::ProjectSearch { .. } => {
                self.read_project(editor, id, prefix, spec)
            }
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
                directory_for: None,
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }

    /// Opens the prompt of `spec` for the command `id` over the current buffer's project, or its
    /// directory outside one, unless a prompt is selected (ADR 0114): its files, listed as the
    /// host walks it, or a search of them. With `C-u`, a directory is read first, with `C-x C-f`'s
    /// completion, and stands for the project.
    fn read_project(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        spec: ArgSpec,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        if matches!(prefix, Some(Prefix::Universal(_))) {
            let prompt = if matches!(spec, ArgSpec::ProjectSearch { .. }) {
                "Search in directory:"
            } else {
                "Find file in directory:"
            };
            let start = editor.start_directory().to_path_buf();
            let directory = editor
                .current()
                .1
                .path()
                .and_then(std::path::Path::parent)
                .map_or(start, std::path::Path::to_path_buf);
            let initial = oso_core::complete::typed_form(&directory, true, editor.home());
            match editor.open_file_prompt(prompt, "project-directory", &initial, directory) {
                Ok(prompt) => self.reading.push(Reading {
                    id,
                    prefix: None,
                    prompt,
                    passphrase: false,
                    runs_answer: false,
                    replace: None,
                    directory_for: Some(spec),
                }),
                Err(error) => editor.prompt_says(error.to_string()),
            }
            return (effects, Ok(()));
        }
        match editor.project_files_root() {
            Ok((root, name)) => self.open_project(editor, id, spec, root, &name),
            Err(not) => editor.message(not.to_string()),
        }
        (effects, Ok(()))
    }

    /// Takes the directory `C-u` read for the prompt of `spec`, and opens that prompt on it,
    /// unless it is the home directory, one that holds it, or the root of the file system.
    pub(super) fn directory_answered(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        spec: ArgSpec,
        text: &str,
    ) -> Effects {
        let directory = std::path::PathBuf::from(text);
        match editor.listable_root(&directory) {
            Ok((root, name)) => self.open_project(editor, reading.id, spec, root, &name),
            Err(_) => {
                let verb = if matches!(spec, ArgSpec::ProjectSearch { .. }) {
                    "search"
                } else {
                    "list"
                };
                editor.message(format!(
                    "Cannot {verb} the home directory, one that holds it, or /"
                ));
            }
        }
        Effects {
            redraw: true,
            ..Effects::default()
        }
    }

    /// Opens the prompt of `spec`, the project's files or its search, on `root`, named `name`,
    /// for the command `id`. The history is the spec's: one for each project for its files, one
    /// for every search.
    fn open_project(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        spec: ArgSpec,
        root: std::path::PathBuf,
        name: &str,
    ) {
        let utf8 = root.to_str().is_some();
        let opened = match spec {
            ArgSpec::ProjectSearch { history } => editor.open_project_search(root, name, history),
            ArgSpec::ProjectFile { history } => editor.open_project_files(root, name, history),
            _ => return,
        };
        let (verb, open_already) = if matches!(spec, ArgSpec::ProjectSearch { .. }) {
            ("search", "A project is searched already")
        } else {
            ("list", "A project's files are listed already")
        };
        match opened {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix: None,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: None,
                directory_for: None,
            }),
            Err(PromptError::Invalid) if !utf8 => {
                editor.message(format!("Cannot {verb} a project whose path is not UTF-8"));
            }
            Err(PromptError::Invalid) => editor.message(open_already),
            Err(error) => editor.prompt_says(error.to_string()),
        }
    }
}
