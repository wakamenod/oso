//! Keyboard macros in the dispatcher (ADR 0113): recording what it takes, a command's input at
//! a time, without the macro commands' own keys; `e` again after `C-x e`; and feeding a
//! replay back through the same entry points, waiting for whatever a command left running,
//! and stopping where Emacs would ding.
//!
//! Must not depend on: `app` or winit. The host calls [`Dispatcher::replay`] at each turn of
//! its loop while a replay is under way, and nothing here logs an event.

use std::collections::VecDeque;
use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::command::{Args, CommandId, Effects, Invocation, Registry};
use oso_core::editor::{Editor, MacroEvent};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::keymap::is_cancel;
use oso_core::minibuffer::Kind;

use super::{Dispatcher, Step};

/// What is said when input that reached a password prompt drops a macro (ADR 0120, Q17).
const DROPPED: &str = "Keyboard macro not recorded: typed in a buffer that may hold secrets";

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
    /// `kmacro-start-macro`'s, which begins a recording.
    start: Option<CommandId>,
    /// The terminals the recording sent keys or text to, in case one turns out to have read
    /// a password then (ADR 0120, Q17).
    terminals: Vec<BufferId>,
    /// Those of the last macro, until each has sent its first snapshot since the recording
    /// ended: a password prompt it says it saw may have been open while it was recorded.
    unheard: Vec<BufferId>,
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
            start: registry.lookup("kmacro-start-macro"),
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
        // Text for a terminal's program goes as a key does, and is recorded as text is
        // (ADR 0120).
        if self.text_to_terminal(editor, text) {
            if editor.macro_recording()
                && !self.macros.replaying
                && editor.macro_takes_text(text.len())
            {
                self.macros.group.push(event());
            }
            self.macro_after(editor);
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
        if Some(id) == self.macros.start {
            self.macros.terminals.clear();
        }
        // A recording that ended became the last macro, with the terminals it typed into.
        if self.macros.ids.contains(&id) && !failed && !editor.macro_recording() {
            let typed = std::mem::take(&mut self.macros.terminals);
            if !typed.is_empty() {
                self.macros.unheard = typed;
            }
        }
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
        // A question or a passphrase prompt takes the key, as a replay never answers one, and
        // a menu a replay left open takes it as its head's (ADR 0117).
        if let (Some(key), Some(again), Some(call)) = (key, repeat, self.macros.call)
            && key == again
            && self.resolver.pending().is_empty()
            && self.prefix.is_none()
            && !asking(editor)
            && !self.menu_open()
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

    /// Notes that the recording sent input to the terminal `buffer`'s program.
    pub(super) fn macro_typed_into(&mut self, editor: &Editor, buffer: BufferId) {
        if editor.macro_recording()
            && !self.macros.replaying
            && !self.macros.terminals.contains(&buffer)
        {
            self.macros.terminals.push(buffer);
        }
    }

    /// The terminal `buffer` sent a snapshot, which says whether its program read a password
    /// since the one before (ADR 0120, Q17). Input that reached a password prompt drops what
    /// holds it, as input that reaches any buffer that may hold secrets does (ADR 0113): the
    /// recording that sent it anything, or the last macro, if this is the terminal's first
    /// snapshot since that recording ended. So keys typed before the host heard of the prompt
    /// are caught too.
    pub fn terminal_reported(&mut self, editor: &mut Editor, buffer: BufferId, echo_off: bool) {
        let first = self.macros.unheard.iter().position(|id| *id == buffer);
        if let Some(at) = first {
            self.macros.unheard.swap_remove(at);
        }
        if !echo_off {
            return;
        }
        if editor.macro_recording() && self.macros.terminals.contains(&buffer) {
            editor.macro_cancel(DROPPED);
        }
        if first.is_some() {
            self.macros.unheard.clear();
            editor.macro_forget_last(DROPPED);
        }
    }

    /// Whether a replay's input is about to reach the terminal `buffer` while its program
    /// reads a password: the replay stops instead, so a macro never types a password.
    pub(super) fn replay_meets_a_password(
        &mut self,
        editor: &mut Editor,
        buffer: BufferId,
    ) -> bool {
        if !self.macros.replaying || !editor.buffer(buffer).is_some_and(|b| b.is_secret()) {
            return false;
        }
        editor.macro_stop(Some(
            "Keyboard macro stopped: the terminal's program reads a password",
        ));
        true
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
            // A search the worker found failing stops it, as one that fails on its key does.
            if editor.isearch().is_some_and(|search| search.failing()) {
                editor.macro_stop(None);
                break;
            }
            let Some(event) = editor.macro_next() else {
                break;
            };
            self.macros.replaying = true;
            self.macros.failed = false;
            let ran = self.feed(editor, &event);
            // What a command of the replay asked of a terminal is bound to it as part of the
            // replay, so a send to a password prompt stops it too (ADR 0120, Q17).
            self.take_key_requests(editor);
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
