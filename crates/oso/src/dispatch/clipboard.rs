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
//!
//! Where input came from travels with it: the yank and each input held behind it keep
//! whether they came through the dev socket, and the host reads the system clipboard only
//! for a yank that did not (ADR 0074). If the selected window, its buffer or the open
//! prompts change before the answer comes, the yank is dropped, as it would land somewhere
//! else.

use std::collections::VecDeque;

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, Args, CommandId, Effects, Invocation};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::kill::FromClipboard;

use super::{Dispatcher, Step};

/// The most input held while a command waits. The wait is short, and the host answers
/// within a bound, so this is reached only by a flood of keys from the socket.
const MAX_HELD: usize = 4096;

/// Input that came while a command waited, and whether it came through the dev socket.
enum Held {
    Key(KeyPress, bool),
    Text(String, bool),
    Commit(String, bool),
    Composition(bool),
}

/// Where a yank was when it began waiting, to drop it if that changed.
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

/// A command waiting for the clipboard.
pub(super) struct Waiting {
    id: CommandId,
    args: Args,
    key_char: Option<char>,
    shifted: bool,
    /// It came through the dev socket.
    from_socket: bool,
    place: Place,
    /// The switches the menu whose head ran it had on, for it to read (ADR 0117).
    switches: Vec<String>,
    /// The host has been asked to read.
    asked: bool,
    held: VecDeque<Held>,
}

impl Dispatcher {
    /// Keeps the command `id` waiting for the clipboard, with its arguments, what it
    /// knows of its key and where it came from. `run_or_read` decides that it waits, as it
    /// decides what a command reads in a prompt (ADR 0072).
    pub(super) fn wait_for_clipboard(
        &mut self,
        editor: &Editor,
        id: CommandId,
        args: Args,
        invocation: &Invocation<'_>,
    ) {
        self.waiting = Some(Waiting {
            id,
            args,
            key_char: invocation.key_char,
            shifted: invocation.shifted,
            from_socket: self.from_socket,
            place: Place::of(editor),
            switches: invocation
                .switches
                .iter()
                .map(|&on| on.to_owned())
                .collect(),
            asked: false,
            held: VecDeque::new(),
        });
    }

    /// Says that the input handled until the next call came through the dev socket, or
    /// not.
    #[cfg(feature = "dev")]
    pub fn set_input_from_socket(&mut self, from_socket: bool) {
        self.from_socket = from_socket;
    }

    /// Whether a command waits for the clipboard.
    pub fn waits_for_clipboard(&self) -> bool {
        self.waiting.is_some()
    }

    /// Whether the host should start reading the clipboard, once for each command that
    /// waits: `Some`, saying whether the command came through the dev socket.
    pub fn take_clipboard_request(&mut self) -> Option<bool> {
        let waiting = self.waiting.as_mut().filter(|waiting| !waiting.asked)?;
        waiting.asked = true;
        Some(waiting.from_socket)
    }

    /// Runs the command that waited, with what the clipboard held, unless the place it
    /// would land has changed, then the input held meanwhile, each as from where it came,
    /// until a command waits again, which keeps the rest. Returns what the host should do.
    pub fn clipboard_answered(&mut self, editor: &mut Editor, answer: &FromClipboard) -> Effects {
        let Some(waiting) = self.waiting.take() else {
            return Effects::default();
        };
        let spec = self.registry.get(waiting.id).map(|command| command.args);
        let mut effects = if waiting.place != Place::of(editor) {
            editor.message("Yank dropped: the window changed while the clipboard was read");
            Effects::default()
        } else if let Some(ArgSpec::KillEntry { prompt }) = spec {
            // `M-y` lists the ring, with what the clipboard held in it (ADR 0091).
            let command = (waiting.id, waiting.args.raw());
            self.as_input(editor, |this, editor| {
                this.read_kill(editor, command, prompt, answer)
            })
        } else {
            self.as_input(editor, |this, editor| {
                let switches: Vec<&str> = waiting.switches.iter().map(String::as_str).collect();
                let invocation = Invocation {
                    key_char: waiting.key_char,
                    shifted: waiting.shifted,
                    clipboard: Some(answer),
                    switches: &switches,
                    ..Invocation::default()
                };
                this.execute(editor, waiting.id, waiting.args, invocation, Step::MayJoin)
                    .0
            })
        };
        let before = self.from_socket;
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
                Held::Key(key, from_socket) => {
                    self.from_socket = from_socket;
                    self.handle_key(editor, key)
                }
                Held::Text(text, from_socket) => {
                    self.from_socket = from_socket;
                    self.insert_text(editor, &text)
                }
                Held::Commit(text, from_socket) => {
                    self.from_socket = from_socket;
                    self.commit_text(editor, &text)
                }
                Held::Composition(from_socket) => {
                    self.from_socket = from_socket;
                    self.begin_composition(editor);
                    Effects::default()
                }
            };
            effects = effects.union(more);
        }
        self.from_socket = before;
        effects
    }

    /// Holds a key while a command waits. Returns whether it was held.
    pub(super) fn hold_key(&mut self, key: KeyPress) -> bool {
        let from_socket = self.from_socket;
        self.hold(|| Held::Key(key, from_socket))
    }

    /// Holds text while a command waits. Returns whether it was held.
    pub(super) fn hold_text(&mut self, text: &str, commit: bool) -> bool {
        let from_socket = self.from_socket;
        self.hold(|| {
            if commit {
                Held::Commit(text.to_owned(), from_socket)
            } else {
                Held::Text(text.to_owned(), from_socket)
            }
        })
    }

    /// Holds the start of a composition while a command waits. Returns whether it was
    /// held.
    pub(super) fn hold_composition(&mut self) -> bool {
        let from_socket = self.from_socket;
        self.hold(|| Held::Composition(from_socket))
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
