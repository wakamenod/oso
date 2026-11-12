//! The prompts that read a path, for a command's argument: `C-x C-f`'s and `C-x C-w`'s by its
//! segments (ADR 0087), and a project's files, listed as they are walked, or a line of them,
//! searched for (ADR 0114). The mark list shares their way in, as it too lists before it reads.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{ArgSpec, Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptError;

use super::super::{Dispatcher, Step};
use super::{ProjectPrompt, Reading, Then};

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
            ArgSpec::ProjectFile { history } => {
                self.read_project(editor, id, prefix, ProjectPrompt::Files { history })
            }
            ArgSpec::ProjectSearch { history } => {
                self.read_project(editor, id, prefix, ProjectPrompt::Search { history })
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
        if let Some(refused) = refused_in_a_prompt(editor) {
            return refused;
        }
        if let Some(prompt) = open_path_prompt(editor, prompt, history, false) {
            self.reading.push(Reading::run(id, prefix, prompt));
        }
        (redraw(), Ok(()))
    }

    /// Opens the prompt of `spec` for the command `id` over the current buffer's project, or its
    /// directory outside one, unless a prompt is selected (ADR 0114): its files, listed as the
    /// host walks it, or a search of them. With `C-u`, a directory is read first, with
    /// `C-x C-f`'s completion, and stands for the project.
    fn read_project(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        project: ProjectPrompt,
    ) -> (Effects, Result<(), ExecuteError>) {
        if let Some(refused) = refused_in_a_prompt(editor) {
            return refused;
        }
        if matches!(prefix, Some(Prefix::Universal(_))) {
            let label = match project {
                ProjectPrompt::Search { .. } => "Search in directory:",
                ProjectPrompt::Files { .. } => "Find file in directory:",
            };
            if let Some(prompt) = open_path_prompt(editor, label, "project-directory", true) {
                self.reading.push(Reading {
                    then: Then::Directory(project),
                    ..Reading::run(id, None, prompt)
                });
            }
            return (redraw(), Ok(()));
        }
        match editor.project_files_root() {
            Ok((root, name)) => self.open_project(editor, id, project, root, &name),
            Err(not) => editor.message(not.to_string()),
        }
        (redraw(), Ok(()))
    }

    /// Takes the directory `C-u` read for `project`, and opens that prompt on it, unless it is
    /// the home directory, one that holds it, or the root of the file system.
    pub(super) fn directory_answered(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        project: ProjectPrompt,
        text: &str,
    ) -> Effects {
        let directory = std::path::PathBuf::from(text);
        match editor.listable_root(&directory) {
            Ok((root, name)) => self.open_project(editor, reading.id, project, root, &name),
            Err(_) => {
                let verb = project.verb();
                editor.message(format!(
                    "Cannot {verb} the home directory, one that holds it, or /"
                ));
            }
        }
        redraw()
    }

    /// Opens `project`'s prompt, its files or its search, on `root`, named `name`, for the
    /// command `id`. The history is the spec's: one for each project for its files, one for
    /// every search.
    fn open_project(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        project: ProjectPrompt,
        root: std::path::PathBuf,
        name: &str,
    ) {
        let utf8 = root.to_str().is_some();
        let (opened, open_already) = match project {
            ProjectPrompt::Search { history } => (
                editor.open_project_search(root, name, history),
                "A project is searched already",
            ),
            ProjectPrompt::Files { history } => (
                editor.open_project_files(root, name, history),
                "A project's files are listed already",
            ),
        };
        match opened {
            Ok(prompt) => self.reading.push(Reading::run(id, None, prompt)),
            Err(PromptError::Invalid) if !utf8 => {
                let verb = project.verb();
                editor.message(format!("Cannot {verb} a project whose path is not UTF-8"));
            }
            Err(PromptError::Invalid) => editor.message(open_already),
            Err(error) => editor.prompt_says(error.to_string()),
        }
    }
}

impl ProjectPrompt {
    /// What it does, as a message says it.
    fn verb(self) -> &'static str {
        match self {
            Self::Search { .. } => "search",
            Self::Files { .. } => "list",
        }
    }
}

/// What a command does when it would open a prompt from inside one: it says so.
fn refused_in_a_prompt(editor: &mut Editor) -> Option<(Effects, Result<(), ExecuteError>)> {
    editor.prompt().is_some().then(|| {
        editor.prompt_says("Not in the minibuffer");
        (redraw(), Ok(()))
    })
}

/// The effects of opening a prompt: a frame.
fn redraw() -> Effects {
    Effects {
        redraw: true,
        ..Effects::default()
    }
}

/// Opens a prompt that says `label` and reads a path with `C-x C-f`'s completion, kept in the
/// history `history`, starting at the shown buffer's directory, or the one oso started in, for a
/// directory if `directory`. A relative path typed in place of it starts there too. `None` if it
/// could not open, which the prompt beneath says.
fn open_path_prompt(
    editor: &mut Editor,
    label: &'static str,
    history: &'static str,
    directory: bool,
) -> Option<oso_core::minibuffer::PromptId> {
    let start = editor.start_directory().to_path_buf();
    let start_in = editor
        .current()
        .1
        .path()
        .and_then(std::path::Path::parent)
        .map_or(start, std::path::Path::to_path_buf);
    let initial = oso_core::complete::typed_form(&start_in, true, editor.home());
    let opened = if directory {
        editor.open_directory_prompt(label, history, &initial, start_in)
    } else {
        editor.open_file_prompt(label, history, &initial, start_in)
    };
    match opened {
        Ok(prompt) => Some(prompt),
        Err(error) => {
            editor.prompt_says(error.to_string());
            None
        }
    }
}
