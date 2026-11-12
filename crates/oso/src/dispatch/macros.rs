//! Keyboard macros in the dispatcher (ADR 0113): recording what it takes, a command's input at
//! a time, without the macro commands' own keys; `e` again after `C-x e`; and feeding a
//! replay back through the same entry points, waiting for whatever a command left running,
//! and stopping where Emacs would ding.
//!
//! Must not depend on: `app` or winit. The host calls [`Dispatcher::replay`] at each turn of
//! its loop while a replay is under way, and nothing here logs an event.

use std::collections::VecDeque;
use std::time::Instant;

use oso_core::command::{Args, CommandId, Effects, Invocation, Registry};
use oso_core::editor::{Editor, MacroEvent};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::is_cancel;
use oso_core::minibuffer::Kind;

use super::{Dispatcher, Step};

/// The macro commands, whose own keys are never recorded.
const MACRO_COMMANDS: [&str; 3] = [
    "kmacro-start-macro",
    "kmacro-end-macro",
    "kmacro-end-and-call-macro",
];

/// What the dispatcher keeps for keyboard macros.
#[derive(Default)]
pub(super) struct MacroInput {
    /// The macro commands' ids.
    ids: Vec<CommandId>,
    /// `kmacro-end-and-call-macro`'s, which `e` runs again right after it.
    call: Option<CommandId>,
    /// What the current command's input took so far: its keys, a prefix argument's among them.
    group: Vec<MacroEvent>,
    /// A macro command ran for this input, so it is not recorded.
    command_ran: bool,
    /// A command failed since this was last cleared, which stops a replay.
    pub(super) failed: bool,
    /// An event of a replay is being fed.
    replaying: bool,
    /// Input from the window while a replay runs, for after it.
    held: VecDeque<MacroEvent>,
    /// The key that runs the last macro again, right after `C-x e`.
    repeat: Option<KeyPress>,
}

impl MacroInput {
    pub(super) fn new(registry: &Registry) -> Self {
        Self {
            ids: MACRO_COMMANDS
                .iter()
                .filter_map(|name| registry.lookup(name))
                .collect(),
            call: registry.lookup("kmacro-end-and-call-macro"),
            ..Self::default()
        }
    }
}

/// Whether a question or a passphrase prompt is open, which a replay never answers and `e`
/// never runs one over.
fn asking(editor: &Editor) -> bool {
    editor
        .prompts()
        .iter()
        .any(|prompt| prompt.kind() == Kind::Passphrase || prompt.kind().is_question())
}

/// The key that runs the last macro again right after it ran: `e`, as Emacs's
/// `kmacro-call-repeat-key`.
fn repeat_key() -> Option<KeyPress> {
    "e".parse::<KeySequence>().ok()?.0.first().copied()
}

impl Dispatcher {
    /// Text from a key or an input method, recorded in a keyboard macro as it comes (ADR 0113),
    /// and run as one command: an undo step of its own if an input method `commit`ted it.
    pub(super) fn take_text(&mut self, editor: &mut Editor, text: &str, commit: bool) -> Effects {
        let event = || MacroEvent::Text {
            text: text.to_owned(),
            commit,
        };
        if let Some(done) = self.macro_before(editor, (None, event)) {
            return done;
        }
        // Held while `!` runs, or while a command waits, it is recorded when it runs.
        let held = super::hold_all::Held::Text {
            text: text.to_owned(),
            commit,
            from_socket: self.from_socket,
        };
        if let Some(done) = self.hold_during_all(editor, held) {
            return done;
        }
        if self.hold_text(text, commit) {
            return Effects::default();
        }
        if editor.macro_recording() && !self.macros.replaying && editor.macro_takes_text(text.len())
        {
            self.macros.group.push(event());
        }
        let step = if commit {
            super::Step::Apart
        } else {
            super::Step::MayJoin
        };
        let effects = self.as_input(editor, |this, editor| this.run_text(editor, text, step));
        self.macro_after(editor);
        effects
    }

    /// Notes that the command `id` ran, and how it ended, for a recording and a replay.
    pub(super) fn macro_ran(&mut self, id: CommandId, failed: bool, editor: &Editor) {
        self.macros.failed |= failed;
        if self.macros.ids.contains(&id) {
            self.macros.command_ran = true;
            if !failed && Some(id) == self.macros.call && !editor.macro_recording() {
                self.macros.repeat = repeat_key();
            }
        }
    }

    /// Before an event from the window is taken, ahead of anything that holds it: held while
    /// a replay runs, `C-g` stopping it, or `e` running the last macro again right after
    /// `C-x e`. Returns what it did, if it took the event. One that goes on is recorded once
    /// nothing holds it.
    pub(super) fn macro_before(
        &mut self,
        editor: &mut Editor,
        (key, event): (Option<KeyPress>, impl FnOnce() -> MacroEvent),
    ) -> Option<Effects> {
        if self.macros.replaying {
            return None;
        }
        let repeat = self.macros.repeat.take();
        if editor.macro_replaying() {
            if key.is_some_and(is_cancel) {
                editor.macro_stop(Some("Keyboard macro stopped"));
                self.macros.held.clear();
                return Some(Effects {
                    redraw: true,
                    ..Effects::default()
                });
            }
            self.macros.held.push_back(event());
            return Some(Effects::default());
        }
        // A question or a passphrase prompt takes the key, as a replay never answers one.
        if let (Some(key), Some(again), Some(call)) = (key, repeat, self.macros.call)
            && key == again
            && self.resolver.pending().is_empty()
            && self.prefix.is_none()
            && !asking(editor)
        {
            let (effects, _) = self.execute(
                editor,
                call,
                Args::NONE,
                Invocation::default(),
                Step::MayJoin,
            );
            return Some(effects);
        }
        None
    }

    /// Notes a key as part of the current command's input, for a recording, once nothing
    /// held it.
    pub(super) fn macro_key(&mut self, editor: &Editor, key: KeyPress) {
        if editor.macro_recording() && !self.macros.replaying {
            self.macros.group.push(MacroEvent::Key(key));
        }
    }

    /// After input from the window was taken: once its command has run, what it took goes into
    /// the recording, unless it ran a macro command. A passphrase prompt dropped the recording
    /// as it opened, and input that went to a buffer that may hold secrets drops it here.
    pub(super) fn macro_after(&mut self, editor: &mut Editor) {
        if self.macros.replaying || !self.resolver.pending().is_empty() || self.prefix.is_some() {
            return;
        }
        let group = std::mem::take(&mut self.macros.group);
        if std::mem::take(&mut self.macros.command_ran) || !editor.macro_recording() {
            return;
        }
        editor.macro_record(group);
    }

    /// Whether a replay runs and can feed its next event now, rather than waiting for what an
    /// earlier one started: the host turns again at once only then (ADR 0119).
    pub fn replay_goes_on(&self, editor: &Editor) -> bool {
        editor.macro_replaying() && !self.replay_waits(editor)
    }

    /// Whether what an earlier event started still runs, so the next must wait: a clipboard
    /// read, a key held for a completion, a search on a worker, `!`'s slices, an undo of a
    /// large step (ADR 0119), or a file loading.
    fn replay_waits(&self, editor: &Editor) -> bool {
        self.waiting.is_some()
            || self.completion_held.is_some()
            || editor.isearch().is_some_and(|search| search.searching())
            || editor.replace().is_some_and(|replace| replace.searching())
            || editor.replace_works()
            || editor.crossing().is_some()
            || editor.cursor_search_waits()
            || editor.current().1.loading().is_some()
    }

    /// Why the replay stops after an event, if it does: a command failed or a key was
    /// undefined, isearch fails, a question or a passphrase prompt is open. `Some(None)`
    /// stops it with what is already said.
    fn replay_stops(&mut self, editor: &Editor) -> Option<Option<&'static str>> {
        if std::mem::take(&mut self.macros.failed) {
            return Some(None);
        }
        if editor.isearch().is_some_and(|search| search.failing()) {
            return Some(None);
        }
        asking(editor).then_some(None)
    }

    /// Feeds the replay's events until `until`, or until one must wait or stops it. Once it
    /// has ended, the input held meanwhile runs. Returns what the host should do.
    pub fn replay(&mut self, editor: &mut Editor, until: Instant) -> Effects {
        let mut effects = Effects::default();
        while editor.macro_replaying() && Instant::now() < until {
            if self.replay_waits(editor) {
                break;
            }
            // Whatever opened a question or a passphrase prompt, the replay answers neither.
            if asking(editor) {
                editor.macro_stop(None);
                break;
            }
            let Some(event) = editor.macro_next() else {
                break;
            };
            self.macros.replaying = true;
            self.macros.failed = false;
            let ran = self.feed(editor, &event);
            self.macros.replaying = false;
            effects = effects.union(ran);
            if effects.exit {
                editor.macro_stop(None);
                break;
            }
            if let Some(why) = self.replay_stops(editor) {
                editor.macro_stop(why);
            }
        }
        if !editor.macro_replaying() {
            for event in std::mem::take(&mut self.macros.held) {
                effects = effects.union(self.feed(editor, &event));
            }
        }
        effects
    }

    fn feed(&mut self, editor: &mut Editor, event: &MacroEvent) -> Effects {
        match event {
            MacroEvent::Key(key) => self.handle_key(editor, *key),
            MacroEvent::Text { text, commit: true } => self.commit_text(editor, text),
            MacroEvent::Text {
                text,
                commit: false,
            } => self.insert_text(editor, text),
        }
    }
}
