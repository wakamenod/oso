//! A command's arguments read in a prompt before it runs, and the command a question's
//! answer runs (ADR 0072).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a window.
//!
//! A command never waits for an answer. When its [`ArgSpec`] reads one and no prefix
//! argument stands for it, the dispatcher opens a prompt and keeps the command, with its
//! prefix argument and the prompt's depth. When that prompt is answered, the command runs
//! with the answer, through the same `execute` as a key. A prompt that closes otherwise
//! drops what waited for it. A question's `y` runs the command it names, confirmed, which
//! checks again what is true by then.

use oso_core::command::{ArgSpec, Args, CommandId, Effects, ExecuteError, Invocation, Prefix};
use oso_core::editor::Editor;
use oso_core::minibuffer::{Kind, Outcome};

use super::{Dispatcher, Step};

/// A command waiting for the answer of the prompt at `depth`.
pub(super) struct Reading {
    id: CommandId,
    prefix: Option<Prefix>,
    depth: usize,
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
    /// Runs the command `id`, or first opens a prompt for what its spec reads. Returns
    /// what the host should do.
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
            _ => return self.execute(editor, id, args, invocation, Step::MayJoin),
        };
        let (prompt, kind) = kind;
        if let Err(error) = editor.open_prompt(prompt, kind) {
            // The innermost prompt is where the person looks.
            editor.prompt_says(error.to_string());
            let effects = Effects {
                redraw: true,
                ..Effects::default()
            };
            return (effects, Ok(()));
        }
        self.reading.push(Reading {
            id,
            prefix: args.raw(),
            depth: editor.prompts().len(),
        });
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
            let outcome = editor.take_prompt_outcome();
            let open = editor.prompts().len();
            let ran = match outcome {
                Some(Outcome::Answered { depth, text }) => {
                    let waiting = self
                        .reading
                        .iter()
                        .rposition(|reading| reading.depth == depth)
                        .map(|index| self.reading.remove(index));
                    self.reading.retain(|reading| reading.depth <= open);
                    waiting.map(|reading| {
                        let invocation = Invocation {
                            answer: Some(&text),
                            ..Invocation::default()
                        };
                        let args = Args::with_prefix(reading.prefix);
                        self.execute(editor, reading.id, args, invocation, Step::MayJoin)
                            .0
                    })
                }
                Some(Outcome::Run(name)) => {
                    self.reading.retain(|reading| reading.depth <= open);
                    self.registry.lookup(name).map(|id| {
                        let invocation = Invocation {
                            confirmed: true,
                            ..Invocation::default()
                        };
                        self.execute(editor, id, Args::NONE, invocation, Step::MayJoin)
                            .0
                    })
                }
                None => {
                    self.reading.retain(|reading| reading.depth <= open);
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
        effects
    }
}
