//! The prompts that read a path, for a command's argument: `C-x C-f`'s and `C-x C-w`'s by its
//! segments (ADR 0087), and a project's files, listed as they are walked, or a line of them,
//! searched for (ADR 0114). The mark list shares their way in, as it too lists before it reads.
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.

use oso_core::command::{ArgSpec, Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::PromptError;

use super::super::{Dispatcher, Step};
use super::{ProjectPrompt, Reading, TextPrompt, Then};

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
                self.read_file(editor, id, prefix, prompt, history, false)
            }
            ArgSpec::Directory { prompt, history } => {
                self.read_file(editor, id, prefix, prompt, history, true)
            }
            ArgSpec::Target { prompt, history } => {
                self.read_target(editor, id, prefix, prompt, history)
            }
            ArgSpec::ProjectFile { history } => {
                self.read_project(editor, id, prefix, ProjectPrompt::Files { history })
            }
            ArgSpec::ProjectSearch { history } => {
                self.read_project(editor, id, prefix, ProjectPrompt::Search { history })
            }
            ArgSpec::ConversationSearch { history } => {
                self.read_conversations(editor, id, prefix, history)
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

    /// Opens the prompt that reads a path for the command `id`, or a directory's if `directory`,
    /// starting at the directory the shown buffer stands in, unless a prompt is selected (ADR
    /// 0087, ADR 0134, ADR 0141).
    fn read_file(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        prompt: &'static str,
        history: &'static str,
        directory: bool,
    ) -> (Effects, Result<(), ExecuteError>) {
        if let Some(refused) = refused_in_a_prompt(editor) {
            return refused;
        }
        if let Some(prompt) = open_path_prompt(editor, prompt, history, directory, None) {
            self.reading.push(Reading::run(id, prefix, prompt));
        }
        (redraw(), Ok(()))
    }

    /// Opens the prompt that reads where a listing's rows go for the command `id`: a path, as
    /// `C-x C-f` reads one, starting at the directory of the next window's listing, or with
    /// `C-u` this one's (ADR 0134). The prefix argument goes on to the command.
    fn read_target(
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
        let this_one = matches!(prefix, Some(Prefix::Universal(_)));
        let start = editor.listing_target(this_one);
        // The marked rows are what goes, so the prompt says how many, whoever marked them.
        let marked = (editor.listing(editor.window().buffer())).map_or(0, |record| record.marked);
        let label = match (marked, prompt.strip_suffix(" to:")) {
            (0, _) | (_, None) => prompt.to_owned(),
            (1, Some(verb)) => format!("{verb} the marked row to:"),
            (count, Some(verb)) => format!("{verb} {count} marked rows to:"),
        };
        if let Some(prompt) = open_path_prompt(editor, &label, history, false, start.as_deref()) {
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
            if let Some(prompt) = open_path_prompt(editor, label, "project-directory", true, None) {
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

    /// Opens the prompt that searches Claude Code's past conversations for the command `id`
    /// (ADR 0145): every conversation's, or with `C-u` the next scope's, the space in front's
    /// else the project's.
    fn read_conversations(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        history: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        if let Some(refused) = refused_in_a_prompt(editor) {
            return refused;
        }
        let scopes = editor.conversation_scopes();
        let at = usize::from(matches!(prefix, Some(Prefix::Universal(_))) && scopes.len() > 1);
        match editor.open_conversation_search(scopes, at, history) {
            Ok(prompt) => self.reading.push(Reading::run(id, None, prompt)),
            Err(PromptError::Invalid) => editor.message("A search is open already"),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (redraw(), Ok(()))
    }

    /// Opens the directory prompt of a `DirectoryAndTextOnPrefix` command `id`, labelled and
    /// kept as `(label, history)`, whose answer opens `then` (ADR 0121).
    pub(super) fn read_directory(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        (label, history): (&'static str, &'static str),
        then: TextPrompt,
    ) -> (Effects, Result<(), ExecuteError>) {
        if let Some(refused) = refused_in_a_prompt(editor) {
            return refused;
        }
        if let Some(prompt) = open_path_prompt(editor, label, history, true, None) {
            self.reading.push(Reading {
                then: Then::DirectoryThenText(then),
                ..Reading::run(id, prefix, prompt)
            });
        }
        (redraw(), Ok(()))
    }

    /// Opens `find-file`'s prompt in `dir`, as `C-x C-f` from a buffer there would: `RET` on the
    /// row of a worktree of `*Spaces*` that no space is in (ADR 0131, step 2b). Nothing opens
    /// while a prompt is selected.
    pub fn find_file_in(&mut self, editor: &mut Editor, dir: &std::path::Path) -> Effects {
        let found =
            self.registry
                .lookup("find-file")
                .and_then(|id| match self.registry.get(id)?.args {
                    ArgSpec::File { prompt, history } => Some((id, prompt, history)),
                    _ => None,
                });
        let Some((id, label, history)) = found else {
            return Effects::default();
        };
        if refused_in_a_prompt(editor).is_some() {
            return redraw();
        }
        if let Some(prompt) = open_path_prompt(editor, label, history, false, Some(dir)) {
            self.reading.push(Reading::run(id, None, prompt));
        }
        redraw()
    }

    /// Takes the directory read for `reading`, and opens its text prompt, which runs the
    /// command with both.
    pub(super) fn open_text_after(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        then: TextPrompt,
        directory: String,
    ) -> Effects {
        let kind = oso_core::minibuffer::Kind::Text {
            history: Some(then.history),
        };
        match editor.open_prompt(then.prompt, kind) {
            Ok(prompt) => self.reading.push(Reading {
                then: Then::TextAfter(directory),
                ..Reading::run(reading.id, reading.prefix, prompt)
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        redraw()
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
        let climbs = directory
            .components()
            .any(|component| component == std::path::Component::ParentDir);
        match editor.listable_root(&directory) {
            Ok((root, name)) => self.open_project(editor, reading.id, project, root, &name),
            Err(_) if climbs => {
                let verb = project.verb();
                editor.message(format!(
                    "Cannot {verb} a directory named with .., name it whole"
                ));
            }
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
/// history `history`, starting at `start_in` if given, else the directory the shown buffer
/// stands in (ADR 0141), for a directory if `directory`. A relative path typed in place of it
/// starts there too. `None` if it could not open, which the prompt beneath says.
fn open_path_prompt(
    editor: &mut Editor,
    label: &str,
    history: &'static str,
    directory: bool,
    start_in: Option<&std::path::Path>,
) -> Option<oso_core::minibuffer::PromptId> {
    let start_in = match start_in {
        Some(dir) => dir.to_path_buf(),
        None => editor.default_directory(editor.window().buffer()),
    };
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
