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

use oso_core::command::{ArgSpec, Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::{Kind, Outcome, PromptId};

use super::{Dispatcher, Step, extended};

/// A command waiting for the answer of the prompt `prompt`.
pub(super) struct Reading {
    id: CommandId,
    prefix: Option<Prefix>,
    prompt: PromptId,
    /// The answer is a passphrase.
    passphrase: bool,
    /// The answer names a command, which runs in its place, as `M-x` does (ADR 0076).
    runs_answer: bool,
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
    pub(super) fn run_or_read(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        args: Args,
        invocation: Invocation<'_>,
    ) -> (Effects, Result<(), ExecuteError>) {
        let spec = self.registry.get(id).map(|command| command.args);
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
            Some(ArgSpec::Passphrase { prompt }) => (prompt, Kind::Passphrase),
            // What the clipboard holds is read off the UI thread, with no prompt (ADR 0074).
            Some(spec) if spec.reads_clipboard(args.raw()) && invocation.clipboard.is_none() => {
                self.wait_for_clipboard(editor, id, args, &invocation);
                return (Effects::default(), Ok(()));
            }
            Some(ArgSpec::Command { prompt, history }) => (prompt, Kind::Complete { history }),
            _ => return self.execute(editor, id, args, invocation, Step::MayJoin),
        };
        let (prompt, kind) = kind;
        let opened = match kind {
            Kind::Complete { history } => {
                let items = self.command_candidates(editor);
                editor.open_completion(extended::with_prefix(prompt, args.raw()), history, items)
            }
            _ => editor.open_prompt(prompt, kind),
        };
        match opened {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix: args.raw(),
                prompt,
                passphrase: kind == Kind::Passphrase,
                runs_answer: matches!(kind, Kind::Complete { .. }),
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
                    waiting.map(|reading| {
                        let args = Args::with_prefix(reading.prefix);
                        if reading.runs_answer {
                            return self.run_answer(editor, &text, args);
                        }
                        let invocation = Invocation {
                            answer: Some(&text),
                            ..Invocation::default()
                        };
                        self.execute(editor, reading.id, args, invocation, Step::MayJoin)
                            .0
                    })
                }
                Some(Outcome::Run(name)) => {
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
                    Some(
                        self.execute(editor, id, Args::NONE, invocation, Step::MayJoin)
                            .0,
                    )
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
        effects.union(self.after_prompts(editor))
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
