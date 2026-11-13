//! A command's arguments read in a prompt before it runs, and the command a question's
//! answer runs (ADR 0072).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! A command never waits for an answer. When its [`ArgSpec`] reads one and no prefix
//! argument stands for it, the dispatcher opens a prompt and keeps the command, with its
//! prefix argument and the prompt's id. When that prompt is answered, the command runs
//! with the answer, through the same `execute` as a key. A prompt that closes otherwise
//! drops what waited for it. A question's `y` runs the command it names, confirmed, which
//! checks again what is true by then.

use oso_core::buffer::EditError;
use oso_core::command::{
    ArgSpec, Args, BufferPurpose, CommandId, Effects, ExecuteError, Invocation, Prefix,
};
use oso_core::editor::{CompletionStyle, Editor};
use oso_core::kill::FromClipboard;
use oso_core::minibuffer::{Kind, Outcome, PromptError, PromptId};

use super::branches::BasePrompt;
use super::{Dispatcher, Step, buffers, extended};

mod paths;
mod replace;

use replace::ReplaceStage;

/// A command waiting for the answer of the prompt `prompt`.
pub(super) struct Reading {
    id: CommandId,
    prefix: Option<Prefix>,
    prompt: PromptId,
    /// The answer is a passphrase.
    passphrase: bool,
    /// What the answer is for.
    then: Then,
    /// The switches the menu whose head ran the command had on, for it to read (ADR 0117).
    pub(super) switches: Vec<String>,
}

/// What a prompt's answer is for.
enum Then {
    /// The command, with the answer.
    Run,
    /// The command it names, in the command's place, as `M-x` does (ADR 0076).
    RunAnswer,
    /// One of query-replace's prompts, the first or the second with the first's answer
    /// (ADR 0113).
    Replace(ReplaceStage),
    /// The directory a project's prompt opens on, as `C-u` asks (ADR 0114).
    Directory(ProjectPrompt),
    /// The menu it names, in the command's place, as `open-menu` does (ADR 0117).
    OpenMenu,
    /// The directory a `DirectoryAndTextOnPrefix` command read first; its text prompt follows
    /// (ADR 0121).
    DirectoryThenText(TextPrompt),
    /// The text a `DirectoryAndTextOnPrefix` command reads after this directory.
    TextAfter(String),
    /// The branch a `BranchAndBaseOnPrefix` command read after a prefix argument; its base
    /// prompt follows (ADR 0121).
    BranchThenBase(BasePrompt),
    /// The base a `BranchAndBaseOnPrefix` command reads after this branch.
    BaseAfter(String),
}

/// The text prompt that follows a directory's (ADR 0121).
#[derive(Clone, Copy)]
struct TextPrompt {
    prompt: &'static str,
    history: &'static str,
}

/// A project's prompt, which `C-u` reads a directory for first (ADR 0114).
#[derive(Clone, Copy)]
enum ProjectPrompt {
    /// Its files, kept in the history `history`, one for each project.
    Files { history: &'static str },
    /// A search of them, kept in the history `history`.
    Search { history: &'static str },
}

impl Reading {
    /// A command waiting to run with the answer of `prompt`.
    fn run(id: CommandId, prefix: Option<Prefix>, prompt: PromptId) -> Self {
        Self {
            id,
            prefix,
            prompt,
            passphrase: false,
            then: Then::Run,
            switches: Vec::new(),
        }
    }

    /// The command `id` waiting for the branch `prompt` reads: to run with it, or after a prefix
    /// argument to read `base` next (ADR 0121).
    pub(super) fn branch(
        (id, prefix): (CommandId, Option<Prefix>),
        prompt: PromptId,
        base: Option<BasePrompt>,
    ) -> Self {
        Self {
            then: base.map_or(Then::Run, Then::BranchThenBase),
            ..Self::run(id, prefix, prompt)
        }
    }

    /// The same command, with its prefix argument, waiting for the base `prompt` reads after
    /// `branch`.
    pub(super) fn base_after(&self, prompt: PromptId, branch: String) -> Self {
        Self {
            then: Then::BaseAfter(branch),
            ..Self::run(self.id, self.prefix, prompt)
        }
    }
}

/// The most commands one key runs through prompts that end: a question's answer runs
/// one, which may itself end another. Past this, the rest wait for the next key.
const MOST_FOLLOW_UPS: usize = 8;

/// Whether `prefix` stands for a number a command reads: digits or a minus, and not `C-u`
/// alone, which says nothing of a number.
fn stands_for_number(prefix: Option<Prefix>) -> Option<isize> {
    match prefix {
        Some(Prefix::Universal(_)) | None => None,
        Some(prefix) => Some(prefix.value()),
    }
}

impl Dispatcher {
    /// Runs the command `id`, or first opens a prompt for what its spec reads, or waits for
    /// the clipboard. Returns what the host should do.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each argument spec, each a few lines"
    )]
    pub(super) fn run_or_read(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        args: Args,
        invocation: Invocation<'_>,
    ) -> (Effects, Result<(), ExecuteError>) {
        let spec = self.registry.get(id).map(|command| command.args);
        if let Some(ArgSpec::Replace { regexp }) = spec {
            return self.read_replace(editor, id, args.raw(), regexp);
        }
        let kind = match spec {
            Some(ArgSpec::Number { prompt, history }) => {
                if let Some(n) = stands_for_number(args.raw()) {
                    let answer = n.to_string();
                    let invocation = Invocation {
                        answer: Some(&answer),
                        ..invocation
                    };
                    return self.execute(editor, id, Args::NONE, invocation, Step::MayJoin);
                }
                (
                    prompt,
                    Kind::Number {
                        history: Some(history),
                    },
                )
            }
            Some(ArgSpec::Text { prompt, history }) => (
                prompt,
                Kind::Text {
                    history: Some(history),
                },
            ),
            Some(ArgSpec::DirectoryAndTextOnPrefix {
                directory,
                directory_history,
                prompt,
                history,
            }) if args.raw().is_some() => {
                let then = TextPrompt { prompt, history };
                return self.read_directory(
                    editor,
                    id,
                    args.raw(),
                    (directory, directory_history),
                    then,
                );
            }
            // The branches are listed off the UI thread before its prompt opens (ADR 0121).
            Some(ArgSpec::BranchAndBaseOnPrefix { .. }) => {
                return self.wait_for_branches(editor, id, args.raw());
            }
            Some(ArgSpec::TextOnPrefix { prompt, history }) if args.raw().is_some() => (
                prompt,
                Kind::Text {
                    history: Some(history),
                },
            ),
            Some(ArgSpec::Passphrase { prompt }) => (prompt, Kind::Passphrase),
            // What the clipboard holds is read off the UI thread, with no prompt (ADR 0074).
            Some(spec) if spec.reads_clipboard(args.raw()) && invocation.clipboard.is_none() => {
                self.wait_unless_refused(editor, (id, spec), args, &invocation);
                return (Effects::default(), Ok(()));
            }
            Some(ArgSpec::Command { prompt, history } | ArgSpec::Theme { prompt, history }) => {
                (prompt, Kind::Complete { history })
            }
            Some(ArgSpec::Listed {
                prompt, history, ..
            }) => (prompt, Kind::Complete { history }),
            Some(ArgSpec::Menu { prompt, history }) => (prompt, Kind::Complete { history }),
            Some(ArgSpec::Buffer {
                prompt,
                history,
                purpose,
            }) => {
                let read = ReadBuffer {
                    prompt,
                    history,
                    purpose,
                };
                return self.read_buffer(editor, id, args.raw(), read);
            }
            Some(
                spec @ (ArgSpec::Mark { .. }
                | ArgSpec::File { .. }
                | ArgSpec::ProjectFile { .. }
                | ArgSpec::ProjectSearch { .. }),
            ) => {
                return self.read_listed(editor, id, args.raw(), spec);
            }
            _ => return self.execute(editor, id, args, invocation, Step::MayJoin),
        };
        let (prompt, kind) = kind;
        // `M-x` completes over the commands, and runs the one named in its place (ADR 0076).
        let runs_answer = matches!(spec, Some(ArgSpec::Command { .. }));
        // `open-menu` completes over the menus, and opens the one named in its place (ADR 0117).
        let opens_menu = matches!(spec, Some(ArgSpec::Menu { .. }));
        let opened = match kind {
            Kind::Complete { history } if runs_answer => {
                let items = self.command_candidates(editor);
                editor.open_completion(extended::with_prefix(prompt, args.raw()), history, items)
            }
            Kind::Complete { history } if opens_menu => {
                let items = self.menu_candidates(editor);
                editor.open_completion(extended::with_prefix(prompt, args.raw()), history, items)
            }
            // `load-theme` completes over the themes the host draws with (ADR 0089).
            Kind::Complete { history } if matches!(spec, Some(ArgSpec::Theme { .. })) => {
                editor.open_completion(prompt, history, editor.theme_candidates())
            }
            // Sessions and spaces complete over what the host last listed (ADR 0121).
            Kind::Complete { history } if let Some(ArgSpec::Listed { list, .. }) = spec => {
                editor.open_completion(prompt, history, editor.listed_candidates(list))
            }
            _ => editor.open_prompt(prompt, kind),
        };
        match opened {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix: args.raw(),
                prompt,
                passphrase: kind == Kind::Passphrase,
                then: if runs_answer {
                    Then::RunAnswer
                } else if opens_menu {
                    Then::OpenMenu
                } else {
                    Then::Run
                },
                switches: Vec::new(),
            }),
            // The innermost prompt is where the person looks.
            Err(error) => editor.prompt_says(error.to_string()),
        }
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        (effects, Ok(()))
    }

    /// Opens the prompt that reads a mark for the command `id`, `C-.`, unless a prompt is
    /// selected, whose own buffer is no list of marks and may be a passphrase's (ADR 0088).
    fn read_mark(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        prompt: &'static str,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        match editor.open_mark_list(prompt) {
            Ok(prompt) => self.reading.push(Reading::run(id, prefix, prompt)),
            Err(PromptError::Invalid) => editor.message("No mark set in this buffer"),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }

    /// Keeps the command `id` waiting for the clipboard, unless it is `M-y` where it could not
    /// yank, which refuses before it reads anything (ADR 0091).
    fn wait_unless_refused(
        &mut self,
        editor: &mut Editor,
        (id, spec): (CommandId, ArgSpec),
        args: Args,
        invocation: &Invocation<'_>,
    ) {
        if matches!(spec, ArgSpec::KillEntry { .. }) {
            let refused = if editor.prompt().is_some() {
                Some("Not in the minibuffer".to_owned())
            } else {
                let read_only = editor.current().1.read_only();
                read_only.map(|reason| EditError::ReadOnly(reason).to_string())
            };
            if let Some(refused) = refused {
                editor.message(refused);
                return;
            }
        }
        self.wait_for_clipboard(editor, id, args, invocation);
    }

    /// Takes what the clipboard held into the ring, as a yank does, and opens the `M-y` list
    /// for the command `id` (ADR 0091).
    pub(super) fn read_kill(
        &mut self,
        editor: &mut Editor,
        (id, prefix): (CommandId, Option<Prefix>),
        prompt: &'static str,
        answer: &FromClipboard,
    ) -> Effects {
        if let FromClipboard::Text {
            text,
            concealed,
            fingerprint,
        } = answer
        {
            editor.take_from_clipboard(text.clone(), *concealed, *fingerprint);
        }
        match editor.open_kill_list(prompt) {
            Ok(prompt) => self.reading.push(Reading::run(id, prefix, prompt)),
            Err(PromptError::Invalid) => editor.message("Kill ring is empty"),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        Effects {
            redraw: true,
            ..Effects::default()
        }
    }

    /// Opens the prompt that reads a buffer for the command `id`, unless a prompt is selected,
    /// whose window always shows its own buffer, so these refuse there before asking anything
    /// (ADR 0072, ADR 0088).
    fn read_buffer(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        read: ReadBuffer,
    ) -> (Effects, Result<(), ExecuteError>) {
        let effects = Effects {
            redraw: true,
            ..Effects::default()
        };
        if editor.prompt().is_some() {
            editor.prompt_says("Not in the minibuffer");
            return (effects, Ok(()));
        }
        // The buffers, most recent first; showing takes any name.
        let style = CompletionStyle {
            given_order: true,
            takes_input: read.purpose == BufferPurpose::Switch,
            preview: read.purpose == BufferPurpose::Switch,
        };
        let items = buffers::candidates(editor, read.purpose);
        match editor.open_completion_as(read.prompt, read.history, items, style) {
            Ok(prompt) => self.reading.push(Reading::run(id, prefix, prompt)),
            Err(error) => editor.prompt_says(error.to_string()),
        }
        (effects, Ok(()))
    }

    /// Runs what `reading` waited for, now that its prompt answered `text`.
    fn answered(&mut self, editor: &mut Editor, reading: &Reading, text: &str) -> Effects {
        let args = Args::with_prefix(reading.prefix);
        match &reading.then {
            Then::RunAnswer => self.run_answer(editor, text, args),
            Then::OpenMenu => {
                // The prefix argument typed before goes to its first head.
                self.prefix = reading.prefix;
                self.open_menu_named(editor, text)
            }
            Then::Replace(stage) => self.replace_answered(editor, reading, stage, text),
            Then::Directory(project) => self.directory_answered(editor, reading, *project, text),
            Then::DirectoryThenText(then) => {
                self.open_text_after(editor, reading, *then, text.to_owned())
            }
            Then::BranchThenBase(base) => {
                self.open_base_after(editor, reading, base, text.to_owned())
            }
            Then::TextAfter(first) | Then::BaseAfter(first) => {
                let invocation = Invocation {
                    answer: Some(first),
                    second: Some(text),
                    ..Invocation::default()
                };
                self.execute(editor, reading.id, args, invocation, Step::MayJoin)
                    .0
            }
            Then::Run => {
                let switches: Vec<&str> = reading.switches.iter().map(String::as_str).collect();
                let invocation = Invocation {
                    answer: Some(text),
                    switches: &switches,
                    ..Invocation::default()
                };
                self.execute(editor, reading.id, args, invocation, Step::MayJoin)
                    .0
            }
        }
    }

    /// Runs what the prompts that ended call for: a command waiting for an answer, or the
    /// command a question's answer names. Drops what waited for a prompt that closed with
    /// no answer.
    pub(super) fn after_prompts(&mut self, editor: &mut Editor) -> Effects {
        let mut effects = Effects::default();
        for _ in 0..MOST_FOLLOW_UPS {
            let ran = match editor.take_prompt_outcome() {
                Some(Outcome::Answered { prompt, text }) => {
                    let waiting = self
                        .reading
                        .iter()
                        .position(|reading| reading.prompt == prompt)
                        .map(|index| self.reading.remove(index));
                    // What a command does with a passphrase stays out of the log (ADR 0072).
                    self.passphrase_next =
                        waiting.as_ref().is_some_and(|reading| reading.passphrase);
                    self.forget_closed(editor);
                    waiting.map(|reading| self.answered(editor, &reading, &text))
                }
                Some(Outcome::Run(name)) => {
                    // What the person's `y` to the agent's question runs is the agent's, so
                    // that a prompt it opens says so too (ADR 0085).
                    let agents = editor.take_agents_answer();
                    self.ran_for_agent |= agents;
                    self.forget_closed(editor);
                    let Some(id) = self.registry.lookup(name) else {
                        // A bug: a question named a command that does not exist.
                        log::error!("a question's answer names no command: {name}");
                        editor.message(format!("No command named {name}"));
                        break;
                    };
                    let invocation = Invocation {
                        confirmed: true,
                        ..Invocation::default()
                    };
                    let mut run = |editor: &mut Editor| {
                        self.execute(editor, id, Args::NONE, invocation, Step::MayJoin)
                            .0
                    };
                    Some(if agents {
                        editor.for_agent(|editor| run(editor))
                    } else {
                        run(editor)
                    })
                }
                None => {
                    self.forget_closed(editor);
                    None
                }
            };
            match ran {
                Some(ran) => effects = effects.union(ran),
                None => break,
            }
            if effects.exit {
                break;
            }
        }
        // What is left past the bound must not run, confirmed, after some later key.
        if let Some(left) = editor.take_prompt_outcome() {
            log::error!("a prompt's outcome was left past {MOST_FOLLOW_UPS} commands: {left:?}");
        }
        effects
    }

    /// Runs `save-buffers-kill-terminal` for the window's close button, as `C-x C-c` does,
    /// so that both ask the same question (ADR 0059, ADR 0072). Pressed while a question is
    /// open, it does nothing more.
    pub fn close_requested(&mut self, editor: &mut Editor) -> Effects {
        // It waits with the keys while `!` or an undo goes on (ADR 0119).
        if let Some(done) = self.hold_during_all(editor, super::hold_all::Held::Close) {
            return done;
        }
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            return Effects::default();
        }
        let Some(id) = self.registry.lookup("save-buffers-kill-terminal") else {
            return Effects::default();
        };
        // As a key would: a prefix key or argument in progress is dropped, not carried under
        // the question.
        self.resolver.reset();
        self.forget_keys(editor);
        self.show_keys_in_progress(editor);
        let effects = self
            .execute(editor, id, Args::NONE, Invocation::default(), Step::MayJoin)
            .0;
        let effects = effects.union(self.after_prompts(editor));
        // The question takes the keys from an open menu, which closes, as it would from a key
        // (ADR 0117). This runs outside an input, so nothing else closes it.
        if self.close_menus_if_taken(editor) {
            return effects.union(Effects {
                redraw: true,
                ..Effects::default()
            });
        }
        effects
    }

    /// Runs the command named `name`, given to `M-x`, with `M-x`'s prefix argument, reading
    /// first what it reads (ADR 0076).
    fn run_answer(&mut self, editor: &mut Editor, name: &str, args: Args) -> Effects {
        let Some(id) = self.registry.lookup(name) else {
            // The prompt gives only a candidate, and every candidate is a command.
            log::error!("M-x gave no command's name");
            editor.message(format!("No command named {name}"));
            return Effects::default();
        };
        let (effects, _) = self.run_or_read(editor, id, args, Invocation::default());
        effects
    }

    /// Drops the commands that waited for a prompt now closed with no answer.
    fn forget_closed(&mut self, editor: &Editor) {
        self.reading.retain(|reading| {
            editor
                .prompts()
                .iter()
                .any(|prompt| prompt.id() == reading.prompt)
        });
    }
}

/// What `ArgSpec::Buffer` asks for.
struct ReadBuffer {
    prompt: &'static str,
    history: &'static str,
    purpose: BufferPurpose,
}
