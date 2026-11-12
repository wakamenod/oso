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
use oso_core::minibuffer::{self, Kind, Outcome, PromptError, PromptId};

use super::{Dispatcher, Step, buffers, extended};

mod paths;

/// A command waiting for the answer of the prompt `prompt`.
pub(super) struct Reading {
    id: CommandId,
    prefix: Option<Prefix>,
    prompt: PromptId,
    /// The answer is a passphrase.
    passphrase: bool,
    /// The answer names a command, which runs in its place, as `M-x` does (ADR 0076).
    runs_answer: bool,
    /// query-replace's prompts: the first, or the second with the first's answer (ADR 0113).
    replace: Option<ReplaceStage>,
}

/// Which of query-replace's two prompts a command waits for (ADR 0113).
#[derive(Clone)]
enum ReplaceStage {
    /// The string to replace.
    From { regexp: bool },
    /// Its replacement, after this string.
    To { regexp: bool, from: String },
}

/// The most cells of the string to replace that the second prompt's label shows.
const REPLACE_LABEL_CELLS: usize = 40;

/// The history query-replace's prompts share, one for each kind.
fn replace_history(regexp: bool) -> &'static str {
    if regexp {
        "query-replace-regexp"
    } else {
        "query-replace"
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
            Some(ArgSpec::Passphrase { prompt }) => (prompt, Kind::Passphrase),
            // What the clipboard holds is read off the UI thread, with no prompt (ADR 0074).
            Some(spec) if spec.reads_clipboard(args.raw()) && invocation.clipboard.is_none() => {
                self.wait_unless_refused(editor, (id, spec), args, &invocation);
                return (Effects::default(), Ok(()));
            }
            Some(ArgSpec::Command { prompt, history } | ArgSpec::Theme { prompt, history }) => {
                (prompt, Kind::Complete { history })
            }
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
                spec @ (ArgSpec::Mark { .. } | ArgSpec::File { .. } | ArgSpec::ProjectFile { .. }),
            ) => {
                return self.read_listed(editor, id, args.raw(), spec);
            }
            _ => return self.execute(editor, id, args, invocation, Step::MayJoin),
        };
        let (prompt, kind) = kind;
        // `M-x` completes over the commands, and runs the one named in its place (ADR 0076).
        let runs_answer = matches!(spec, Some(ArgSpec::Command { .. }));
        let opened = match kind {
            Kind::Complete { history } if runs_answer => {
                let items = self.command_candidates(editor);
                editor.open_completion(extended::with_prefix(prompt, args.raw()), history, items)
            }
            // `load-theme` completes over the themes the host draws with (ADR 0089).
            Kind::Complete { history } if matches!(spec, Some(ArgSpec::Theme { .. })) => {
                editor.open_completion(prompt, history, editor.theme_candidates())
            }
            _ => editor.open_prompt(prompt, kind),
        };
        match opened {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix: args.raw(),
                prompt,
                passphrase: kind == Kind::Passphrase,
                runs_answer,
                replace: None,
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
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: None,
            }),
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
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: None,
            }),
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

    /// Opens query-replace's first prompt, offering the last pair as the default, unless the
    /// selected buffer is read-only, which it says before anything is read (ADR 0113).
    fn read_replace(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        regexp: bool,
    ) -> (Effects, Result<(), ExecuteError>) {
        let redraw = Effects {
            redraw: true,
            ..Effects::default()
        };
        if let Err(refused) = editor.replace_may_begin() {
            editor.message(refused.to_string());
            return (redraw, Ok(()));
        }
        let kind = if regexp { " regexp" } else { "" };
        let label = match editor.last_replace(regexp) {
            Some((from, to)) => {
                let (from, from_cut) = minibuffer::cut_to(from, REPLACE_LABEL_CELLS);
                let (to, to_cut) = minibuffer::cut_to(to, REPLACE_LABEL_CELLS);
                format!("Query replace{kind} (default {from}{from_cut} → {to}{to_cut}): ")
            }
            None => format!("Query replace{kind}: "),
        };
        let stage = ReplaceStage::From { regexp };
        self.open_replace_prompt(editor, id, prefix, label, stage);
        (redraw, Ok(()))
    }

    fn open_replace_prompt(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        prefix: Option<Prefix>,
        label: String,
        stage: ReplaceStage,
    ) {
        let regexp = match &stage {
            ReplaceStage::From { regexp } | ReplaceStage::To { regexp, .. } => *regexp,
        };
        let kind = Kind::Text {
            history: Some(replace_history(regexp)),
        };
        match editor.open_prompt(label, kind) {
            Ok(prompt) => self.reading.push(Reading {
                id,
                prefix,
                prompt,
                passphrase: false,
                runs_answer: false,
                replace: Some(stage),
            }),
            Err(error) => editor.prompt_says(error.to_string()),
        }
    }

    /// Takes the answer of one of query-replace's prompts: the string opens the second, and
    /// the replacement, or an empty string, runs the command.
    fn replace_answered(
        &mut self,
        editor: &mut Editor,
        reading: &Reading,
        stage: &ReplaceStage,
        text: &str,
    ) -> Effects {
        let (from, to) = match stage {
            ReplaceStage::From { regexp } if !text.is_empty() => {
                let kind = if *regexp { " regexp" } else { "" };
                let (shown, cut) = minibuffer::cut_to(text, REPLACE_LABEL_CELLS);
                let label = format!("Query replace{kind} {shown}{cut} with: ");
                let stage = ReplaceStage::To {
                    regexp: *regexp,
                    from: text.to_owned(),
                };
                self.open_replace_prompt(editor, reading.id, reading.prefix, label, stage);
                return Effects {
                    redraw: true,
                    ..Effects::default()
                };
            }
            ReplaceStage::From { .. } => ("", ""),
            ReplaceStage::To { from, .. } => (from.as_str(), text),
        };
        let invocation = Invocation {
            answer: Some(from),
            second: Some(to),
            ..Invocation::default()
        };
        let args = Args::with_prefix(reading.prefix);
        self.execute(editor, reading.id, args, invocation, Step::MayJoin)
            .0
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
                        if let Some(stage) = &reading.replace {
                            return self.replace_answered(editor, &reading, stage, &text);
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

/// What `ArgSpec::Buffer` asks for.
struct ReadBuffer {
    prompt: &'static str,
    history: &'static str,
    purpose: BufferPurpose,
}
