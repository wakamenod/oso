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

/// The key that runs the last macro again right after it ran: `e`, as Emacs's
/// `kmacro-call-repeat-key`.
fn repeat_key() -> Option<KeyPress> {
    "e".parse::<KeySequence>().ok()?.0.first().copied()
}

impl Dispatcher {
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

    /// Before `event` from the window is taken: held while a replay runs, `C-g` stopping it,
    /// or `e` running the last macro again right after `C-x e`. Returns what it did, if it took
    /// the event.
    pub(super) fn macro_before(
        &mut self,
        editor: &mut Editor,
        event: &MacroEvent,
    ) -> Option<Effects> {
        if self.macros.replaying {
            return None;
        }
        let repeat = self.macros.repeat.take();
        if editor.macro_replaying() {
            if matches!(event, MacroEvent::Key(key) if is_cancel(*key)) {
                editor.macro_stop(Some("Keyboard macro stopped"));
                self.macros.held.clear();
                return Some(Effects {
                    redraw: true,
                    ..Effects::default()
                });
            }
            self.macros.held.push_back(event.clone());
            return Some(Effects::default());
        }
        if let (MacroEvent::Key(key), Some(again), Some(call)) = (event, repeat, self.macros.call)
            && *key == again
            && self.resolver.pending().is_empty()
            && self.prefix.is_none()
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
        self.macros.group.push(event.clone());
        None
    }

    /// After input from the window was taken: once its command has run, what it took goes into
    /// the recording, unless it ran a macro command. A passphrase prompt drops the recording,
    /// so nothing typed there is kept (ADR 0072).
    pub(super) fn macro_after(&mut self, editor: &mut Editor) {
        if self.macros.replaying || !self.resolver.pending().is_empty() || self.prefix.is_some() {
            return;
        }
        let group = std::mem::take(&mut self.macros.group);
        if std::mem::take(&mut self.macros.command_ran) || !editor.macro_recording() {
            return;
        }
        if editor
            .prompts()
            .iter()
            .any(|prompt| prompt.kind() == Kind::Passphrase)
        {
            editor.macro_cancel("Keyboard macro not recorded: a passphrase was asked for");
            return;
        }
        editor.macro_record(group);
    }

    /// Whether what an earlier event started still runs, so the next must wait: a clipboard
    /// read, a key held for a completion, a search on a worker, `!`'s slices, or a file loading.
    fn replay_waits(&self, editor: &Editor) -> bool {
        self.waiting.is_some()
            || self.completion_held.is_some()
            || editor.isearch().is_some_and(|search| search.searching())
            || editor
                .replace()
                .is_some_and(|replace| replace.searching() || replace.replacing_all())
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
        let prompts = editor.prompts();
        if prompts
            .iter()
            .any(|prompt| prompt.kind() == Kind::Passphrase)
        {
            return Some(Some("Keyboard macro stopped at a passphrase prompt"));
        }
        if prompts.iter().any(|prompt| prompt.kind().is_question()) {
            return Some(None);
        }
        None
    }

    /// Feeds the replay's events until `until`, or until one must wait or stops it. Once it
    /// has ended, the input held meanwhile runs. Returns what the host should do.
    pub fn replay(&mut self, editor: &mut Editor, until: Instant) -> Effects {
        let mut effects = Effects::default();
        while editor.macro_replaying() && Instant::now() < until {
            if self.replay_waits(editor) {
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
