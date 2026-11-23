//! The `M-v` held while a dictation's text is due (ADR 0175). TypeWhisper types a dictation's
//! text itself as well, by putting it on the clipboard and posting Command-V, which oso reads
//! as `M-v`, `scroll-down-command`. oso types the text itself, so that Command-V must not
//! scroll.
//!
//! Must not depend on: `app`, the clipboard or dictation's threads; the host opens the guard,
//! looks at the clipboard and brings the answer, as for a yank (`clipboard`).
//!
//! - While the guard the host set is open, an `M-v` (Command-V, with no other modifier) is held,
//!   and the host asks the clipboard whether it carries TypeWhisper's marker type. Nothing of
//!   the text is read.
//! - With the marker, the key is dropped, and the log says so. Without it, or with no answer,
//!   it runs as usual.
//! - Input that comes meanwhile is held behind it, in order, as behind a yank.
//! - Keys from the dev socket go through the same check, so the flows cover it; a keyboard
//!   macro's replay does not.

use std::collections::VecDeque;
use std::time::Instant;

use oso_core::command::Effects;
use oso_core::editor::Editor;
use oso_core::key::{Key, KeyPress, Modifiers};

use super::Dispatcher;
use super::clipboard::Held;

/// How long the host holds a pasting `M-v`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PasteGuard {
    /// Never: no dictation's text is due.
    #[default]
    Off,
    /// Until this moment: a dictation stopped and its text is due, or came, and TypeWhisper's
    /// own paste may still follow.
    Until(Instant),
}

/// An `M-v` held while the host looks at the clipboard, and what came after it.
struct Check {
    key: KeyPress,
    from_socket: bool,
    /// The host has been asked to look.
    asked: bool,
    held: VecDeque<Held>,
}

/// The dispatcher's part: the guard and the key it holds.
#[derive(Default)]
pub(super) struct PasteHold {
    guard: PasteGuard,
    check: Option<Check>,
    /// The next `M-v` runs unchecked: it is the one the check let through.
    let_through: bool,
}

impl PasteHold {
    /// The input held behind an `M-v` being checked, if one is.
    pub(super) fn held(&mut self) -> Option<&mut VecDeque<Held>> {
        self.check.as_mut().map(|check| &mut check.held)
    }

    /// Whether the guard holds an `M-v` now.
    fn open(&self, now: Instant) -> bool {
        match self.guard {
            PasteGuard::Off => false,
            PasteGuard::Until(until) => now < until,
        }
    }
}

/// Whether `key` is Command-V as TypeWhisper posts it: `M-v` alone.
pub(super) fn pastes(key: KeyPress) -> bool {
    key.modifiers == Modifiers::META && key.key == Key::Char('v')
}

impl Dispatcher {
    /// The host says how long a pasting `M-v` is held (ADR 0175).
    pub fn guard_pastes(&mut self, guard: PasteGuard) {
        self.paste_guard.guard = guard;
    }

    /// Whether the host should look at the clipboard for a held `M-v`, once for each:
    /// `Some`, saying whether the key came through the dev socket.
    pub fn take_paste_check(&mut self) -> Option<bool> {
        let check = (self.paste_guard.check.as_mut()).filter(|check| !check.asked)?;
        check.asked = true;
        Some(check.from_socket)
    }

    /// Whether an `M-v` is held for the clipboard's answer, and what is typed behind it.
    pub fn holds_paste(&self) -> bool {
        self.paste_guard.check.is_some()
    }

    /// Whether an `M-v` is held for the clipboard's answer, for the dev socket's state.
    #[cfg(feature = "dev")]
    pub fn checks_paste(&self) -> bool {
        self.paste_guard.check.is_some()
    }

    /// Drops the held `M-v` if the clipboard carries TypeWhisper's marker, `marked`, and runs it
    /// otherwise, or with no answer; then the input held behind it. Returns what the host
    /// should do.
    pub fn paste_checked(&mut self, editor: &mut Editor, marked: Option<bool>) -> Effects {
        let Some(check) = self.paste_guard.check.take() else {
            return Effects::default();
        };
        let effects = if marked == Some(true) {
            log::info!("dictation: TypeWhisper's Command-V dropped");
            Effects::default()
        } else {
            let before = std::mem::replace(&mut self.from_socket, check.from_socket);
            self.paste_guard.let_through = true;
            let effects = self.handle_key(editor, check.key);
            self.paste_guard.let_through = false;
            self.from_socket = before;
            effects
        };
        self.replay_held(editor, check.held, effects)
    }

    /// Holds `key` for the host's look at the clipboard, if it is a pasting `M-v` while the
    /// guard is open. Returns whether it was held.
    pub(super) fn guard_takes(&mut self, editor: &Editor, key: KeyPress) -> bool {
        let guard = &mut self.paste_guard;
        if std::mem::take(&mut guard.let_through) {
            return false;
        }
        if !pastes(key) || guard.guard == PasteGuard::Off || editor.macro_replaying() {
            return false;
        }
        if !guard.open(Instant::now()) {
            guard.guard = PasteGuard::Off;
            return false;
        }
        guard.check = Some(Check {
            key,
            from_socket: self.from_socket,
            asked: false,
            held: VecDeque::new(),
        });
        true
    }
}
