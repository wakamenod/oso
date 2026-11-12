//! From key presses to commands: the keymaps, the key resolver and the
//! command registry, wired together without a window so that key sequences
//! can be tested as plain functions of the editor state. The prefix argument a
//! command leaves waits here for the next one, next to the pending prefix keys
//! (ADR 0034, ADR 0058). A command's arguments are read in a prompt before it runs,
//! in [`prompts`] (ADR 0072).
//!
//! Must not depend on: `app` or winit; key sequences stay testable without a
//! window.

use oso_core::command::{
    Args, CommandId, Ctx, Effects, ExecuteError, Invocation, Prefix, Registry,
};
use oso_core::editor::{Editor, MacroEvent};
use oso_core::key::Key;
use oso_core::key::KeyPress;
#[cfg(any(test, feature = "dev"))]
use oso_core::keymap::Layers;
use oso_core::keymap::{CANCEL, KeyResolver, Keymap, Resolution, is_cancel};

use crate::emergency::Publisher;
use crate::{bindings, commands};

mod agent;
mod args;
mod buffers;
pub use agent::{Unshowable, unshowable};
pub use args::Given;
pub(crate) use buffers::size;
mod clipboard;
mod extended;
mod held;
mod hold_all;
mod jump;
mod keymaps;
mod macros;
mod prompts;
#[cfg(any(test, feature = "dev"))]
mod run_command;
mod shown;

use keymaps::Keymaps;
use shown::{masked, reads_passphrase};

/// Whether a command's edits may join the undo step before, if it asks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    MayJoin,
    Apart,
}

/// Runs commands for key presses. Keys from the window and from the dev
/// control socket both come through here.
pub struct Dispatcher {
    registry: Registry,
    /// Every keymap, held apart from the resolver, so that working out which are active
    /// borrows only them.
    keymaps: Keymaps,
    /// The commands that run in a search rather than end it, for `run-command` (ADR 0082).
    #[cfg(any(test, feature = "dev"))]
    isearch_commands: Vec<CommandId>,
    /// The commands that run in a query-replace rather than end it, for `run-command`
    /// (ADR 0113).
    #[cfg(any(test, feature = "dev"))]
    replace_commands: Vec<CommandId>,
    /// The commands that run in a jump rather than end it, for `run-command` (ADR 0116).
    #[cfg(any(test, feature = "dev"))]
    jump_commands: Vec<CommandId>,
    /// Commands waiting for a prompt's answer, the outermost first.
    reading: Vec<prompts::Reading>,
    /// The next command run gets a passphrase.
    passphrase_next: bool,
    resolver: KeyResolver,
    /// The prefix argument for the next command.
    prefix: Option<Prefix>,
    /// The user's keymap given while a prefix key was pending, which goes in once none is
    /// (ADR 0108).
    user_waiting: Option<Keymap>,
    /// The user's keymaps waiting ones replaced, for the caller to drop elsewhere.
    replaced: Vec<Keymap>,
    /// The command that ran last, with no undefined or cancelled key since, and
    /// how many times in a row it had run before that.
    last: Option<(CommandId, usize)>,
    /// Keeps the emergency save up to date, in the running app (ADR 0062).
    publisher: Option<Publisher>,
    /// A command waiting for the OS clipboard, and the input held meanwhile (ADR 0074).
    waiting: Option<clipboard::Waiting>,
    /// Keyboard macros: what a recording takes, and a replay's state here (ADR 0113).
    macros: macros::MacroInput,
    /// Input held while query-replace's `!` runs (ADR 0113).
    held_during_all: hold_all::HeldDuringAll,
    /// The input being handled came through the dev socket, which never reads the system
    /// clipboard. It travels with what waits or is held.
    from_socket: bool,
    /// `RET` or `TAB`, held while a worker matches the input (ADR 0096).
    completion_held: Option<held::HeldTake>,
    /// Whether the last input made the cursor flash, until the host takes it (ADR 0097).
    view_jumped: Option<bool>,
    /// A command ran for the agent channel since the host last asked: what it queued is the
    /// agent's, so the saves it queued are checked as the channel's (ADR 0108).
    ran_for_agent: bool,
    /// The only commands `M-x` lists, for snapshots that a new command must not change.
    #[cfg(test)]
    m_x_only: Option<&'static [&'static str]>,
}

impl Dispatcher {
    /// A dispatcher with the default commands and global keymap.
    pub fn new() -> Self {
        let registry = commands::registry();
        let global = bindings::global_keymap(&registry);
        Self::with(registry, global)
    }

    /// A dispatcher over `registry`, with `global` as the global keymap.
    pub fn with(registry: Registry, global: Keymap) -> Self {
        let keymaps = Keymaps::new(&registry, global);
        let macros = macros::MacroInput::new(&registry);
        #[cfg(any(test, feature = "dev"))]
        let isearch_commands = bindings::isearch_commands(&registry);
        #[cfg(any(test, feature = "dev"))]
        let replace_commands = bindings::replace_commands(&registry);
        #[cfg(any(test, feature = "dev"))]
        let jump_commands = bindings::jump_commands(&registry);
        Self {
            keymaps,
            #[cfg(any(test, feature = "dev"))]
            isearch_commands,
            #[cfg(any(test, feature = "dev"))]
            replace_commands,
            #[cfg(any(test, feature = "dev"))]
            jump_commands,
            reading: Vec::new(),
            passphrase_next: false,
            registry,
            resolver: KeyResolver::new(),
            prefix: None,
            user_waiting: None,
            replaced: Vec::new(),
            last: None,
            publisher: None,
            waiting: None,
            macros,
            held_during_all: hold_all::HeldDuringAll::default(),
            from_socket: false,
            completion_held: None,
            view_jumped: None,
            ran_for_agent: false,
            #[cfg(test)]
            m_x_only: None,
        }
    }

    /// Whether a command ran for the agent channel since this was last asked, so that the
    /// saves it queued are the agent's (ADR 0108).
    pub fn take_ran_for_agent(&mut self) -> bool {
        std::mem::take(&mut self.ran_for_agent)
    }

    /// Keeps the emergency save up to date through `publisher`.
    pub fn set_publisher(&mut self, publisher: Publisher) {
        self.publisher = Some(publisher);
    }

    /// Tells the emergency save what has changed. A command's end does this itself; the
    /// host calls this for anything else that changes a buffer: a save reporting, a load.
    /// That takes in nothing a panicking command left.
    pub fn publish_for_emergency(&self, editor: &Editor) {
        if let Some(publisher) = &self.publisher {
            publisher.publish(editor);
        }
    }

    /// The keys of the pending prefix, empty if none is pending.
    #[cfg(any(test, feature = "dev"))]
    pub fn pending(&self) -> &[KeyPress] {
        self.resolver.pending()
    }

    /// Whether keys are in progress at all: a prefix key, or a prefix argument, even one that
    /// takes no more digits, which the next command would take.
    pub fn keys_in_progress(&self) -> bool {
        !self.resolver.pending().is_empty() || self.prefix.is_some()
    }

    /// Whether the next key is read as a key rather than typed: a prefix key is
    /// pending, or a prefix argument reads digits or a minus. The input method stays
    /// off then, as a letter or a digit would start a composition (ADR 0069).
    pub fn reads_keys(&self) -> bool {
        !self.resolver.pending().is_empty()
            || self
                .prefix
                .is_some_and(|prefix| prefix.reads_digits() || prefix.reads_minus())
    }

    /// Whether text would go in as typed now: no prefix key or prefix argument waits, no
    /// question is open, and what is active takes text. A commit the window layer makes
    /// unasked waits otherwise, so that it neither takes a prefix argument nor is lost
    /// (ADR 0077).
    pub fn types_text(&self, editor: &Editor) -> bool {
        let question = editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question());
        if question || self.prefix.is_some() || !self.resolver.pending().is_empty() {
            return false;
        }
        let reading = self.keymaps.reading();
        let active = self.keymaps.active(&reading, self.prefix, editor);
        active.text_command().is_some()
    }

    /// Runs `work` as one input, as Emacs does a key: the echo area empties first,
    /// but for what waits behind keys in progress, and then shows the keys it left in
    /// progress (ADR 0063). Every input comes through here, so here alone it is asked
    /// whether the view jumped, for the cursor flash (ADR 0097, ADR 0105).
    fn as_input<R>(
        &mut self,
        editor: &mut Editor,
        work: impl FnOnce(&mut Self, &mut Editor) -> R,
    ) -> R {
        editor.clear_echo();
        // A prefix may have been dropped outside an input, as a window's close does.
        self.take_waiting_user();
        let before = editor.view_sample();
        let out = work(self, editor);
        // Before the view is sampled: another digit of line numbers narrows the text area.
        editor.fit_gutter();
        self.view_jumped = Some(editor.flashes(&before, &editor.view_sample()));
        self.take_waiting_user();
        self.show_keys_in_progress(editor);
        out
    }

    /// Puts in the user's keymap that waited for a prefix key, once none is pending. The one
    /// it replaces waits in turn, for [`Dispatcher::take_replaced_keymap`].
    fn take_waiting_user(&mut self) {
        if self.resolver.pending().is_empty()
            && let Some(user) = self.user_waiting.take()
        {
            let old = std::mem::replace(&mut self.keymaps.user, user);
            self.replaced.push(old);
        }
    }

    /// Puts the user's keys from the configuration just above the global keymap, in place of
    /// those before; an empty keymap takes them away (ADR 0108). While a prefix key is
    /// pending, they wait for it to resolve, so that its next key reads the keymaps it
    /// started in.
    ///
    /// Returns the keymap this replaces now, the one in force or one that was waiting, for the
    /// caller to drop elsewhere: a large one takes milliseconds to drop. One that a waiting
    /// keymap replaces later is kept for [`Dispatcher::take_replaced_keymap`].
    pub fn set_user_keymap(&mut self, user: Keymap) -> Option<Keymap> {
        if self.resolver.pending().is_empty() {
            if let Some(stale) = self.user_waiting.take() {
                self.replaced.push(stale);
            }
            Some(std::mem::replace(&mut self.keymaps.user, user))
        } else {
            self.user_waiting.replace(user)
        }
    }

    /// A user's keymap that a waiting one replaced, once its prefix key resolved, for the
    /// caller to drop elsewhere. Call it until it gives `None`.
    pub fn take_replaced_keymap(&mut self) -> Option<Keymap> {
        self.replaced.pop()
    }

    /// Whether the last input since this was last asked made the view jump, so the cursor
    /// flashes, or none came. An input that did not jump ends a flash; an agent's never
    /// does, so after one only a jump is reported.
    pub fn take_view_jumped(&mut self) -> Option<bool> {
        self.view_jumped.take()
    }

    /// Picks up a prefix in progress, for a state handoff, and the echo area shows it.
    /// Commands never run here: keys that are no longer a prefix in this build are
    /// dropped.
    #[cfg(any(test, feature = "dev"))]
    pub fn restore_pending(&mut self, editor: &mut Editor, keys: &[KeyPress]) {
        let layers = Layers {
            user: self.keymaps.user(),
            ..Layers::global(&self.keymaps.global)
        };
        for key in keys {
            if !matches!(self.resolver.feed(&layers, *key), Resolution::Pending) {
                self.resolver.reset();
                break;
            }
        }
        self.show_keys_in_progress(editor);
    }

    /// Handles key presses in order, as from the control socket's `send-keys`,
    /// calling `each` after each one. Keys after one that exits are dropped.
    #[cfg(any(test, feature = "dev"))]
    pub fn handle_keys_with(
        &mut self,
        editor: &mut Editor,
        keys: impl IntoIterator<Item = KeyPress>,
        mut each: impl FnMut(&Editor, Effects),
    ) -> Effects {
        let mut effects = Effects::default();
        for key in keys {
            let key_effects = self.handle_key(editor, key);
            each(editor, key_effects);
            effects = effects.union(key_effects);
            if effects.exit {
                break;
            }
        }
        effects
    }

    /// Inserts `text`, which may hold several characters, as a key that types
    /// them would: through the text command of the active keymaps, as one command
    /// and one undo step. A pending prefix makes it an undefined key sequence.
    /// A key that types several characters comes through here.
    pub fn insert_text(&mut self, editor: &mut Editor, text: &str) -> Effects {
        self.take_text(editor, text, false)
    }

    /// Starts an input method's composition, which counts as a key: keys never reach
    /// oso during one (ADR 0069).
    pub fn begin_composition(&mut self, editor: &mut Editor) {
        if self.hold_composition() {
            return;
        }
        self.as_input(editor, |_, _| ());
    }

    /// Inserts an input method's commit, as [`Dispatcher::insert_text`] does, but
    /// always as an undo step of its own, even of one character (ADR 0069).
    pub fn commit_text(&mut self, editor: &mut Editor, text: &str) -> Effects {
        self.take_text(editor, text, true)
    }

    fn run_text(&mut self, editor: &mut Editor, text: &str, step: Step) -> Effects {
        // Text changes the input, so a key held for its older result is dropped (ADR 0096).
        self.drop_held_completion();
        // Text answers no question, and says so, as another key does.
        if editor
            .prompt()
            .is_some_and(|prompt| prompt.kind().is_question())
        {
            editor.prompt_says("Answer y or n");
            return Effects {
                redraw: true,
                ..Effects::default()
            };
        }
        if !self.resolver.pending().is_empty() {
            let pending = self.resolver.pending();
            log::debug!("{} and text are undefined", masked(pending, true));
            // The text stays out of it, as it does from a log.
            let keys = masked(pending, reads_passphrase(editor));
            editor.echo(format!("{keys} is undefined"));
            self.resolver.reset();
            self.forget_keys(editor);
            return Effects::default();
        }
        let reading = self.keymaps.reading();
        let text_command = self
            .keymaps
            .active(&reading, self.prefix, editor)
            .text_command();
        match text_command {
            Some(id) if !text.is_empty() => {
                let args = Args::with_prefix(self.prefix.take());
                let invocation = Invocation {
                    input: text,
                    ..Invocation::default()
                };
                let effects = self.execute(editor, id, args, invocation, step).0;
                effects.union(self.after_prompts(editor))
            }
            _ => Effects::default(),
        }
    }

    /// An undefined or cancelled key sequence drops the prefix argument and ends a
    /// run of one command.
    fn forget_keys(&mut self, editor: &mut Editor) {
        self.prefix = None;
        self.last = None;
        editor.set_indent_rigidly_layer(false);
        editor.forget_kills();
    }

    /// Runs one command, invoked as `invocation` says but for its repeats, which are
    /// counted here, in an undo group of its own, so that it undoes as one
    /// step across all cursors. Then keeps the prefix argument it left, and scrolls
    /// the selected window to keep its cursor in view, as Emacs's redisplay does. A
    /// failure is not fatal (ADR 0014), and leaves a message, which stays out of the log
    /// around a secret buffer, as what it says can depend on the text. A panic names only the
    /// command, and is a bug to find, so it comes after the command is marked as ended.
    /// What a panicking command left is held back from the emergency save until a command
    /// ends normally, as the person has then seen it and gone on from it.
    fn execute(
        &mut self,
        editor: &mut Editor,
        id: CommandId,
        args: Args,
        invocation: Invocation<'_>,
        step: Step,
    ) -> (Effects, Result<(), ExecuteError>) {
        let repeats = match self.last {
            Some((last, repeats)) if last == id => repeats.saturating_add(1),
            _ => 0,
        };
        self.last = Some((id, repeats));
        // Any command but the person's of the layer of `C-x TAB` closes it first, joining its
        // keys' edits.
        if step == Step::Apart || !self.keymaps.rigid_commands.contains(&id) {
            editor.set_indent_rigidly_layer(false);
        }
        editor.begin_command();
        if std::mem::take(&mut self.passphrase_next) {
            editor.command_reads_passphrase();
        }
        editor.begin_undo_group();
        if step == Step::Apart {
            editor.keep_undo_step_apart();
        }
        let invocation = Invocation {
            repeats,
            ..invocation
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        let boundary = self.publisher.as_ref().map(Publisher::enter_command);
        let result = self.registry.execute(id, &mut ctx, args);
        drop(boundary);
        let mut effects = ctx.effects();
        editor.note_command_ended(effects.prefix.is_some());
        self.prefix = effects.prefix.take();
        match &result {
            Ok(()) => editor.end_undo_group(),
            Err(ExecuteError::Failed { error, .. }) => {
                editor.end_undo_group();
                editor.message(error.message());
            }
            Err(ExecuteError::Panicked { .. }) => editor.end_undo_group_after_panic(),
            Err(error @ ExecuteError::UnknownId(_)) => {
                editor.end_undo_group();
                editor.message(error.to_string());
            }
        }
        editor.end_command();
        self.macro_ran(id, result.is_err(), editor);
        // What the command typed in a prompt that completes is matched once, now (ADR 0076).
        editor.match_completion();
        if let Err(error @ ExecuteError::Panicked { .. }) = &result {
            // This names the command and nothing else (ADR 0028).
            editor.message(error.to_string());
            // What it changed may be visible.
            effects.redraw = true;
        }
        effects.redraw |= editor.follow_cursor();
        if let Some(publisher) = &self.publisher {
            if matches!(result, Err(ExecuteError::Panicked { .. })) {
                publisher.command_panicked(editor);
            } else {
                publisher.command_ended(editor);
            }
        }
        (effects, result)
    }

    /// Handles one key press: waits on a prefix, or runs the command the
    /// keys resolve to. Returns what the host should do: redraw only when a
    /// command changed something visible, and exit when one asked to. The echo
    /// area empties first, as in Emacs, but for what waits behind keys in progress, and
    /// then says what the key left.
    pub fn handle_key(&mut self, editor: &mut Editor, key: KeyPress) -> Effects {
        // A replay takes a key from the window first: `C-g` stops it before anything else.
        if let Some(done) = self.macro_before(editor, (Some(key), || MacroEvent::Key(key))) {
            return done;
        }
        if let Some(done) = self.hold_during_all(editor, hold_all::Held::Key(key)) {
            return done;
        }
        if self.hold_key(key) {
            return Effects::default();
        }
        self.macro_key(editor, key);
        let effects = self.as_input(editor, |this, editor| this.resolve_key(editor, key));
        self.macro_after(editor);
        effects
    }

    fn resolve_key(&mut self, editor: &mut Editor, key: KeyPress) -> Effects {
        // Any key drops a `RET` or `TAB` held for a result, a prefix's too (ADR 0096).
        let dropped = self.drop_held_completion();
        if let Some(effects) = self.jump_digit(editor, key) {
            return effects;
        }
        let reading = self.keymaps.reading();
        let mut layers = self.keymaps.active(&reading, self.prefix, editor);
        // `C-g` is never the user's, Shift or not: under a prefix it cancels whatever is bound,
        // and on its own it runs what the defaults bind (ADR 0108, ADR 0110).
        let cancels = is_cancel(key);
        let key = if cancels && !self.resolver.pending().is_empty() {
            CANCEL
        } else {
            key
        };
        if cancels {
            layers.user = None;
        }
        let (id, shifted) = match self.resolver.feed(&layers, key) {
            Resolution::Command(id) => (id, false),
            // A shifted key that only its unshifted form binds (ADR 0071).
            Resolution::Shifted(id) => (id, true),
            Resolution::Pending => {
                log::debug!("{}-", masked(self.resolver.pending(), true));
                return Effects::default();
            }
            Resolution::Unbound(_) if editor.jump().is_some() => {
                return self.jump_unbound(editor);
            }
            Resolution::Unbound(keys)
                if editor.isearch().is_some() || editor.replace().is_some() =>
            {
                // It ends the search, and the key, or a sequence under isearch's `C-c`, does
                // what it would have (ADR 0082, ADR 0091). So it ends query-replace
                // (ADR 0113).
                let keys = keys.to_vec();
                editor.isearch_exit();
                editor.replace_exit();
                let mut ran = Effects {
                    redraw: true,
                    ..Effects::default()
                };
                for key in keys {
                    ran = ran.union(self.resolve_key(editor, key));
                }
                return ran;
            }
            Resolution::Unbound(keys) => {
                // A key that types a character may be text meant for the buffer,
                // which stays out of the log (ADR 0028).
                log::debug!("{} is undefined", masked(keys, true));
                if editor
                    .prompt()
                    .is_some_and(|prompt| prompt.kind().is_question())
                {
                    editor.prompt_says("Answer y or n");
                } else {
                    let keys = masked(keys, reads_passphrase(editor));
                    editor.echo(format!("{keys} is undefined"));
                }
                self.forget_keys(editor);
                return Effects::default();
            }
            Resolution::Cancelled => {
                log::debug!("Quit");
                editor.drop_cursor_search();
                editor.echo("Quit");
                self.forget_keys(editor);
                return Effects::default();
            }
        };
        // The text comes from the key, so keys from the window and from the socket
        // type the same. `KeyEvent.text` would type `q` for an unbound `C-q` (ADR 0051).
        let mut utf8 = [0; 4];
        let input = key.text().map_or("", |c| c.encode_utf8(&mut utf8));
        let key_char = match key.key {
            Key::Char(c) => Some(c),
            Key::Named(_) => None,
        };
        let args = Args::with_prefix(self.prefix.take());
        if self.hold_for_completion(editor, id, args, dropped) {
            return Effects::default();
        }
        let invocation = Invocation {
            input,
            key_char,
            shifted,
            ..Invocation::default()
        };
        let effects = self.run_or_read(editor, id, args, invocation).0;
        effects.union(self.after_prompts(editor))
    }
}

#[cfg(test)]
impl Dispatcher {
    /// Handles key presses in order. Keys after one that exits are dropped.
    pub fn handle_keys(
        &mut self,
        editor: &mut Editor,
        keys: impl IntoIterator<Item = KeyPress>,
    ) -> Effects {
        self.handle_keys_with(editor, keys, |_, _| {})
    }

    /// Has `M-x` list only the commands named in `names`.
    pub(crate) fn list_only_in_m_x(&mut self, names: &'static [&'static str]) {
        self.m_x_only = Some(names);
    }
}

#[cfg(test)]
mod buffer_tests;
#[cfg(test)]
mod complete_tests;
#[cfg(test)]
mod cursor_tests;
#[cfg(test)]
mod edit_tests;
#[cfg(test)]
mod emergency_tests;
#[cfg(test)]
mod file_tests;
#[cfg(test)]
mod grid_tests;
#[cfg(test)]
mod held_tests;
#[cfg(test)]
mod ime_tests;
#[cfg(test)]
mod indent_tests;
#[cfg(test)]
mod isearch_tests;
#[cfg(test)]
mod jump_tests;
#[cfg(test)]
mod kill_tests;
#[cfg(test)]
mod macro_tests;
#[cfg(test)]
mod mark_tests;
#[cfg(test)]
mod motion_tests;
#[cfg(test)]
mod prefix_tests;
#[cfg(test)]
mod project_tests;
#[cfg(test)]
mod prompt_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod replace_tests;
#[cfg(test)]
mod save_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod user_tests;
#[cfg(test)]
mod wrap_tests;
#[cfg(test)]
mod yank_pop_tests;

#[cfg(test)]
mod tests {
    use super::Dispatcher;
    use crate::commands;
    use oso_core::buffer::Buffer;
    use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Effects};
    use oso_core::display_map::DisplayPoint;
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::keymap::Keymap;
    use oso_core::window::{Scroll, ViewSize};

    /// Feeds the keys of `s` and returns the combined effects.
    fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, s: &str) -> Effects {
        let keys: KeySequence = s.parse().unwrap();
        dispatcher.handle_keys(editor, keys.0)
    }

    const REDRAW: Effects = Effects {
        redraw: true,
        exit: false,
        prefix: None,
    };

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("test", text));
        editor.set_view_size(ViewSize::new(10, 20));
        editor
    }

    fn cursor(editor: &Editor) -> usize {
        editor.window().selections.primary().cursor
    }

    /// The cursor as a display point.
    fn point(editor: &Editor) -> DisplayPoint {
        let (window, buffer) = editor.current();
        buffer
            .display()
            .offset_to_display_point(window.selections.primary().cursor)
    }

    #[test]
    fn char_motion_through_the_keymap() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f C-f C-b"), REDRAW);
        assert_eq!(cursor(&editor), 1);
        assert_eq!(
            send(&mut dispatcher, &mut editor, "<right> <right> <left>"),
            REDRAW
        );
        assert_eq!(cursor(&editor), 2);
    }

    #[test]
    fn one_key_passes_one_character_on_screen() {
        let mut dispatcher = Dispatcher::new();
        // Full width, a combining mark, a ZWJ family, a flag, a tab, CRLF.
        let text = "日か\u{3099}👨‍👩‍👧🇯🇵\tx\r\n";
        let mut editor = editor(text);
        let mut stops = vec![0];
        for _ in 0..7 {
            send(&mut dispatcher, &mut editor, "C-f");
            stops.push(cursor(&editor));
        }
        let pieces: Vec<&str> = stops.windows(2).map(|w| &text[w[0]..w[1]]).collect();
        assert_eq!(pieces, ["日", "か\u{3099}", "👨‍👩‍👧", "🇯🇵", "\t", "x", "\r\n"]);
        assert_eq!(point(&editor), DisplayPoint::new(1, 0));
        send(&mut dispatcher, &mut editor, "C-b C-b");
        assert_eq!(point(&editor), DisplayPoint::new(0, 16), "x after the tab");
    }

    #[test]
    fn line_motion_keeps_the_goal_column_across_widths() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdefgh\n日本語\n\nab\tc\r\nabcdefgh");
        send(&mut dispatcher, &mut editor, "C-f C-f C-f C-f C-f");
        assert_eq!(point(&editor), DisplayPoint::new(0, 5));
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(
            point(&editor),
            DisplayPoint::new(1, 4),
            "inside 語: its start"
        );
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(point(&editor), DisplayPoint::new(2, 0), "an empty line");
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(
            point(&editor),
            DisplayPoint::new(3, 2),
            "inside the tab: its start"
        );
        send(&mut dispatcher, &mut editor, "C-n");
        assert_eq!(point(&editor), DisplayPoint::new(4, 5), "back at the goal");
        send(&mut dispatcher, &mut editor, "C-p C-p C-p C-p");
        assert_eq!(point(&editor), DisplayPoint::new(0, 5));
        // Another motion resets the goal.
        send(&mut dispatcher, &mut editor, "C-b C-n");
        assert_eq!(point(&editor), DisplayPoint::new(1, 4));
    }

    #[test]
    fn line_motion_from_a_line_end_stays_at_line_ends() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc\r\n日本語です\nx\nabcdefgh");
        send(&mut dispatcher, &mut editor, "C-e C-n");
        assert_eq!(point(&editor), DisplayPoint::new(1, 10));
        send(&mut dispatcher, &mut editor, "C-n C-n");
        assert_eq!(point(&editor), DisplayPoint::new(3, 8));
        send(&mut dispatcher, &mut editor, "C-a C-p");
        assert_eq!(point(&editor), DisplayPoint::new(2, 0));
        send(&mut dispatcher, &mut editor, "M-> M-<");
        assert_eq!(cursor(&editor), 0);
        send(&mut dispatcher, &mut editor, "M->");
        assert_eq!(point(&editor), DisplayPoint::new(3, 8));
    }

    #[test]
    fn the_window_follows_the_cursor_by_the_fewest_rows_and_columns() {
        let mut dispatcher = Dispatcher::new();
        let long = "x".repeat(30);
        let text: String = (0..30).map(|_| format!("{long}\n")).collect();
        let mut editor = editor(&text);
        for _ in 0..10 {
            send(&mut dispatcher, &mut editor, "C-n");
        }
        assert_eq!((editor.top_row(), editor.window().scroll().left), (1, 0));
        assert_eq!(send(&mut dispatcher, &mut editor, "C-e"), REDRAW);
        assert_eq!((editor.top_row(), editor.window().scroll().left), (1, 11));
        send(&mut dispatcher, &mut editor, "C-b");
        assert_eq!(editor.window().scroll().left, 11, "still in view");
        send(&mut dispatcher, &mut editor, "C-a M->");
        assert_eq!((editor.top_row(), editor.window().scroll().left), (21, 0));
        send(&mut dispatcher, &mut editor, "M-<");
        assert_eq!(editor.window().scroll(), Scroll::default());
    }

    #[test]
    fn page_keys_scroll_by_a_screen() {
        let mut dispatcher = Dispatcher::new();
        let text: String = (0..30).map(|i| format!("{i:02}\n")).collect();
        let mut editor = editor(&text);
        send(&mut dispatcher, &mut editor, "C-v <next>");
        assert_eq!(editor.top_row(), 16);
        assert_eq!(point(&editor), DisplayPoint::new(16, 0));
        send(&mut dispatcher, &mut editor, "M-v <prior>");
        assert_eq!(editor.top_row(), 0);
        assert_eq!(point(&editor), DisplayPoint::new(9, 0));
        // At the start M-v signals and changes nothing.
        assert_eq!(
            send(&mut dispatcher, &mut editor, "M-v"),
            Effects::default()
        );
    }

    #[test]
    fn a_prefix_then_an_unbound_key_changes_nothing() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        send(&mut dispatcher, &mut editor, "C-f C-f C-f");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x"),
            Effects::default()
        );
        assert_eq!(
            dispatcher.pending(),
            &"C-x".parse::<KeySequence>().unwrap().0[..]
        );
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-t"),
            Effects::default()
        );
        assert_eq!(cursor(&editor), 3);
        assert!(dispatcher.pending().is_empty());
        // The next key starts afresh.
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(cursor(&editor), 4);
    }

    #[test]
    fn c_g_cancels_a_prefix_and_quits_without_one() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x C-g"),
            Effects::default()
        );
        assert!(dispatcher.pending().is_empty());
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-g"),
            Effects::default()
        );
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-c"),
            Effects::default()
        );
        assert_eq!(cursor(&editor), 0);
    }

    #[test]
    fn c_x_c_c_exits() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("");
        assert_eq!(
            send(&mut dispatcher, &mut editor, "C-x C-c"),
            Effects {
                redraw: false,
                exit: true,
                prefix: None,
            }
        );
    }

    #[test]
    fn keys_after_an_exit_are_dropped() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let effects = send(&mut dispatcher, &mut editor, "C-f C-x C-c <right> C-x");
        assert!(effects.exit);
        assert_eq!(cursor(&editor), 1);
        assert!(dispatcher.pending().is_empty());
    }

    #[test]
    fn each_key_of_a_batch_is_reported_on_its_own_up_to_an_exit() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let keys: KeySequence = "C-f <f35> C-f C-x C-c C-f".parse().unwrap();
        let mut seen = Vec::new();
        let effects = dispatcher.handle_keys_with(&mut editor, keys.0, |editor, effects| {
            seen.push((cursor(editor), effects.redraw, effects.exit));
        });
        assert!(effects.exit);
        assert_eq!(
            seen,
            [
                (1, true, false),
                (1, false, false),
                (2, true, false),
                (2, false, false),
                (2, false, true),
            ],
        );
        assert_eq!(cursor(&editor), 2, "the key after the exit did not run");
    }

    /// Shows the first buffer for which `secret` is what `is_secret` says.
    #[cfg(feature = "dev")]
    fn show_where(ctx: &mut Ctx<'_>, secret: bool) -> Result<(), CommandError> {
        let found = ctx.editor.buffers().find(|(_, b)| b.is_secret() == secret);
        let id = found.map(|(id, _)| id).unwrap();
        ctx.editor.show(id);
        ctx.request_redraw();
        Ok(())
    }

    #[cfg(feature = "dev")]
    #[test]
    fn a_key_around_a_secret_buffer_leaves_no_timing_one_by_one_in_a_batch() {
        use crate::dev::{HandledKey, SecretWatch, Timings};
        use std::time::Instant;
        let mut registry = commands::registry();
        let secret_command = Command {
            name: "show-secret",
            doc: "Show a secret buffer, as `C-x b` will.",
            args: ArgSpec::None,
            reach: oso_core::command::Reach::Look,
            run: |ctx, _| show_where(ctx, true),
        };
        let plain_command = Command {
            name: "show-plain",
            doc: "Show an ordinary buffer, as `C-x b` will.",
            args: ArgSpec::None,
            reach: oso_core::command::Reach::Look,
            run: |ctx, _| show_where(ctx, false),
        };
        registry
            .register_all(&[secret_command, plain_command])
            .unwrap();
        let table = [
            ("C-f", "forward-char"),
            ("<f1>", "show-secret"),
            ("<f2>", "show-plain"),
        ];
        let global = Keymap::from_table(&registry, &table).unwrap();
        let mut dispatcher = Dispatcher::with(registry, global);
        let mut editor = editor("abcdef");
        // A path counts as secret until it is judged.
        editor.add_buffer(Buffer::visiting("/p/.env".into()));

        let mut timings = Timings::new(Instant::now());
        let mut watch = SecretWatch::new(&editor);
        // Only the first and the last key here have nothing secret around them.
        let keys: KeySequence = "C-f <f1> C-f <f2> C-f".parse().unwrap();
        let mut seen = Vec::new();
        dispatcher.handle_keys_with(&mut editor, keys.0, |editor, effects| {
            let withheld = watch.after_key(editor);
            seen.push(withheld);
            let key = HandledKey {
                redraw: effects.redraw,
                withheld,
            };
            timings.key_handled(Instant::now(), key);
        });
        assert_eq!(seen, [false, true, true, true, false]);
        timings.frame_presented(true, false);
        let taken = timings.take();
        let samples = taken["key_latency_us"].as_array().map(Vec::len);
        assert_eq!(
            samples,
            Some(1),
            "the key before the secret one was dropped"
        );
    }

    fn explode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
        ctx.editor.current_mut().0.selections = oso_core::selection::SelectionSet::new(2);
        panic!("deliberate panic in a test command");
    }

    #[test]
    fn a_panicking_command_does_not_stop_the_editor() {
        let mut registry = commands::registry();
        registry
            .register(Command {
                name: "explode",
                doc: "Panic, to test the command boundary.",
                args: ArgSpec::None,
                reach: oso_core::command::Reach::Look,
                run: explode,
            })
            .unwrap();
        let global =
            Keymap::from_table(&registry, &[("C-c p", "explode"), ("C-f", "forward-char")])
                .unwrap();
        let mut dispatcher = Dispatcher::with(registry, global);
        let mut editor = editor("abcdef");
        assert_eq!(send(&mut dispatcher, &mut editor, "C-c p"), REDRAW);
        assert_eq!(cursor(&editor), 2);
        assert!(dispatcher.pending().is_empty());
        assert_eq!(send(&mut dispatcher, &mut editor, "C-f"), REDRAW);
        assert_eq!(cursor(&editor), 3);
    }

    #[test]
    fn run_command_runs_by_name_with_its_argument() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abcdef");
        let (effects, result) = dispatcher.run_command(&mut editor, "forward-char", None);
        assert_eq!((effects, result), (REDRAW, Ok(())));
        assert_eq!(cursor(&editor), 1);
        let (_, result) = dispatcher.run_command(&mut editor, "forward-char", Some(3));
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 4);
        let (_, result) = dispatcher.run_command(&mut editor, "backward-char", Some(-1));
        assert_eq!(result, Ok(()));
        assert_eq!(cursor(&editor), 5);
    }

    #[test]
    fn run_command_scrolls_to_the_cursor_and_asks_to_exit() {
        let mut dispatcher = Dispatcher::new();
        let text: String = (0..100).map(|i| format!("{i}\n")).collect();
        let mut editor = editor(&text);
        let (_, result) = dispatcher.run_command(&mut editor, "next-line", Some(50));
        assert_eq!(result, Ok(()));
        assert_eq!(editor.top_row(), 41);
        let (effects, result) =
            dispatcher.run_command(&mut editor, "save-buffers-kill-terminal", None);
        assert!(effects.exit);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn run_command_reports_errors_and_keeps_what_they_changed() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let (effects, result) = dispatcher.run_command(&mut editor, "no-such-command", None);
        assert_eq!(effects, Effects::default());
        assert_eq!(result, Err("no command named \"no-such-command\"".into()));
        let (effects, result) = dispatcher.run_command(&mut editor, "forward-char", Some(10));
        assert_eq!(result, Err("forward-char: End of buffer".into()));
        assert_eq!(effects, REDRAW, "it moved as far as it could");
        assert_eq!(cursor(&editor), 3);
    }

    #[test]
    fn run_command_reports_a_panic_and_an_unwanted_argument() {
        let mut registry = oso_core::command::Registry::new();
        registry
            .register(Command {
                name: "explode",
                doc: "Panics.",
                args: ArgSpec::None,
                reach: oso_core::command::Reach::Look,
                run: explode,
            })
            .unwrap();
        let mut dispatcher = Dispatcher::with(registry, Keymap::new());
        let mut editor = editor("abc");
        let (effects, result) = dispatcher.run_command(&mut editor, "explode", None);
        assert_eq!(result, Err("explode panicked".into()));
        assert!(effects.redraw, "what it changed may be visible");
        let (effects, result) = dispatcher.run_command(&mut editor, "explode", Some(1));
        assert_eq!(result, Err("explode takes no argument".into()));
        assert_eq!(effects, Effects::default(), "it did not run");
    }

    #[test]
    fn a_restored_prefix_waits_for_its_next_key_and_runs_nothing() {
        let mut dispatcher = Dispatcher::new();
        let mut editor = editor("abc");
        let keys = |s: &str| s.parse::<KeySequence>().unwrap().0;
        dispatcher.restore_pending(&mut editor, &keys("C-x"));
        assert_eq!(dispatcher.pending(), keys("C-x"));
        assert!(send(&mut dispatcher, &mut editor, "C-c").exit);

        // Not a prefix any more: a command, or nothing at all.
        dispatcher.restore_pending(&mut editor, &keys("C-f"));
        assert!(dispatcher.pending().is_empty());
        assert_eq!(cursor(&editor), 0, "the command did not run");
        dispatcher.restore_pending(&mut editor, &keys("C-x C-z C-z"));
        assert!(dispatcher.pending().is_empty());
    }
}
