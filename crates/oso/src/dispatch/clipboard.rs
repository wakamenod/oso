//! A command that waits for the OS clipboard, and the input held until it has run
//! (ADR 0074).
//!
//! Must not depend on: `app` or the clipboard itself; the host reads it and brings the
//! answer.
//!
//! A command whose spec reads the clipboard, `yank`, does not run when its keys
//! resolve: `run_or_read` keeps it here, as it keeps one that reads a prompt's answer
//! (ADR 0072), with its arguments, the host reads the clipboard off
//! the UI thread, and the command runs through the same `execute` once the answer
//! comes, so undo and the messages see one command. Keys, text and commits that arrive
//! meanwhile are held in order, and handled once it has run, so typing after `C-y` lands
//! after the yanked text.

use std::collections::VecDeque;

use oso_core::command::{Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::kill::FromClipboard;

use super::{Dispatcher, Step};

/// The most input held while a command waits. The wait is short, and the host answers
/// within a bound, so this is reached only by a flood of keys from the socket.
const MAX_HELD: usize = 4096;

/// Input that came while a command waited.
enum Held {
    Key(KeyPress),
    Text(String),
    Commit(String),
    Composition,
}

/// A command waiting for the clipboard.
pub(super) struct Waiting {
    id: CommandId,
    args: Args,
    key_char: Option<char>,
    shifted: bool,
    /// The host has been asked to read.
    asked: bool,
    held: VecDeque<Held>,
}

impl Dispatcher {
    /// Keeps the command `id` waiting for the clipboard, with its arguments and what it
    /// knows of its key. `run_or_read` decides that it waits, as it decides what a
    /// command reads in a prompt (ADR 0072).
    pub(super) fn wait_for_clipboard(
        &mut self,
        id: CommandId,
        args: Args,
        invocation: &Invocation<'_>,
    ) {
        self.waiting = Some(Waiting {
            id,
            args,
            key_char: invocation.key_char,
            shifted: invocation.shifted,
            asked: false,
            held: VecDeque::new(),
        });
    }

    /// Whether a command waits for the clipboard.
    #[cfg(any(test, feature = "dev"))]
    pub fn waits_for_clipboard(&self) -> bool {
        self.waiting.is_some()
    }

    /// Whether the host should start reading the clipboard: true once for each command
    /// that waits.
    pub fn take_clipboard_request(&mut self) -> bool {
        self.waiting
            .as_mut()
            .is_some_and(|waiting| !std::mem::replace(&mut waiting.asked, true))
    }

    /// Runs the command that waited, with what the clipboard held, then the input held
    /// meanwhile, until a command waits again, which keeps the rest. Returns what the host
    /// should do.
    pub fn clipboard_answered(&mut self, editor: &mut Editor, answer: &FromClipboard) -> Effects {
        let Some(waiting) = self.waiting.take() else {
            return Effects::default();
        };
        let mut effects = self.as_input(editor, |this, editor| {
            let invocation = Invocation {
                key_char: waiting.key_char,
                shifted: waiting.shifted,
                clipboard: Some(answer),
                ..Invocation::default()
            };
            this.execute(editor, waiting.id, waiting.args, invocation, Step::MayJoin)
                .0
        });
        let mut held = waiting.held;
        while let Some(input) = held.pop_front() {
            if effects.exit {
                break;
            }
            if let Some(again) = &mut self.waiting {
                again.held.push_back(input);
                again.held.append(&mut held);
                break;
            }
            let more = match input {
                Held::Key(key) => self.handle_key(editor, key),
                Held::Text(text) => self.insert_text(editor, &text),
                Held::Commit(text) => self.commit_text(editor, &text),
                Held::Composition => {
                    self.begin_composition(editor);
                    Effects::default()
                }
            };
            effects = effects.union(more);
        }
        effects
    }

    /// Holds a key while a command waits. Returns whether it was held.
    pub(super) fn hold_key(&mut self, key: KeyPress) -> bool {
        self.hold(|| Held::Key(key))
    }

    /// Holds text while a command waits. Returns whether it was held.
    pub(super) fn hold_text(&mut self, text: &str, commit: bool) -> bool {
        self.hold(|| {
            if commit {
                Held::Commit(text.to_owned())
            } else {
                Held::Text(text.to_owned())
            }
        })
    }

    /// Holds the start of a composition while a command waits. Returns whether it was
    /// held.
    pub(super) fn hold_composition(&mut self) -> bool {
        self.hold(|| Held::Composition)
    }

    fn hold(&mut self, input: impl FnOnce() -> Held) -> bool {
        let Some(waiting) = &mut self.waiting else {
            return false;
        };
        if waiting.held.len() < MAX_HELD {
            waiting.held.push_back(input());
        } else {
            // What it was stays out of the log, as typed text does (ADR 0028).
            log::warn!("input dropped while waiting for the clipboard");
        }
        true
    }
}
