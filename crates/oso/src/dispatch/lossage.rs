//! The last keys the dispatcher received, each with what it did, which `view-lossage` shows in
//! `*Lossage*` (ADR 0150). A key typed where secrets may be is kept as `<hidden>` and never as
//! the key, and so is a character a terminal's program got; a run of them is one line, so it
//! tells no length. A key held to run later is kept as `<held>`, and again as it runs, judged
//! where it runs. The keys live only in memory.
//!
//! Must not depend on: `app` or winit.

use std::collections::VecDeque;
use std::fmt::Write;

use oso_core::buffer::BufferId;
use oso_core::command::{CommandId, Registry};
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::is_cancel;
use oso_core::minibuffer::{Kind, cells_of};
use oso_core::selection::SelectionSet;

use super::Dispatcher;
use crate::commands::help::{VIEW_LOSSAGE, show_internal};

/// How many keys `*Lossage*` shows, as Emacs's `view-lossage` does.
const KEPT_KEYS: usize = 300;
/// The internal buffer of the keys received.
const LOSSAGE: &str = "*Lossage*";
/// What a key kept as `<hidden>` shows.
const HIDDEN: &str = "<hidden>";
/// The column `;;` starts at, in cells.
const COMMENT_COLUMN: usize = 20;

/// How a key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeptAs {
    Key(KeyPress),
    /// A character sent to a terminal's program, which may be a password typed before the
    /// terminal said it reads one.
    SentCharacter,
    /// A key typed where secrets may be: what it did is not kept either.
    Secret,
    /// A key the window layer gave that oso has no name for.
    Unnamed,
    /// A key held to run later, which is kept again when it runs.
    Held,
}

/// What a key did, as far as `*Lossage*` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Did {
    /// No command ran: a prefix, or a key a mode, a menu or a terminal kept.
    Nothing,
    Ran(CommandId),
    SentToTerminal,
    Undefined,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    kept: KeptAs,
    did: Did,
}

/// The keys received, and `*Lossage*`.
pub(super) struct Lossage {
    command: Option<CommandId>,
    /// Oldest first, at most [`KEPT_KEYS`].
    entries: VecDeque<Entry>,
    /// The newest entry is the key being handled, which what it does is noted on.
    handling: bool,
    buffer: Option<BufferId>,
}

impl Lossage {
    pub(super) fn new(registry: &Registry) -> Self {
        Self {
            command: registry.lookup(VIEW_LOSSAGE),
            entries: VecDeque::with_capacity(KEPT_KEYS),
            handling: false,
            buffer: None,
        }
    }

    /// Keeps a key. One where secrets may be, right after another, adds nothing: a run of
    /// them is one line.
    /// Returns whether it added an entry.
    fn push(&mut self, kept: KeptAs) -> bool {
        if kept == KeptAs::Secret && self.newest_hides() {
            return false;
        }
        if self.entries.len() == KEPT_KEYS {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            kept,
            did: Did::Nothing,
        });
        true
    }

    /// Whether the newest entry is a hidden key's, which another one joins.
    fn newest_hides(&self) -> bool {
        self.entries
            .back()
            .is_some_and(|entry| matches!(entry.kept, KeptAs::Secret | KeptAs::SentCharacter))
    }

    /// The newest entry, if it is the key being handled.
    fn handled(&mut self) -> Option<&mut Entry> {
        if self.handling {
            self.entries.back_mut()
        } else {
            None
        }
    }

    /// Notes what the key being handled did, the first time it does something.
    fn note(&mut self, did: Did) {
        if !self.handling {
            return;
        }
        if let Some(entry) = self.entries.back_mut()
            && entry.did == Did::Nothing
            && entry.kept != KeptAs::Secret
        {
            entry.did = did;
        }
    }

    /// The text of `*Lossage*`: a key a line, and what it did after `;;`.
    fn text(&self, registry: &Registry) -> String {
        let mut text = String::new();
        for entry in &self.entries {
            let start = text.len();
            match entry.kept {
                KeptAs::Key(key) => {
                    let _ = write!(text, "{key}");
                }
                KeptAs::SentCharacter | KeptAs::Secret => text.push_str(HIDDEN),
                KeptAs::Unnamed => text.push_str("<no Emacs name>"),
                KeptAs::Held => text.push_str("<held>"),
            }
            let did = match entry.did {
                Did::Nothing => None,
                Did::Ran(id) => registry.get(id).map(|command| command.name),
                Did::SentToTerminal => Some("sent to the terminal"),
                Did::Undefined => Some("undefined"),
            };
            if let Some(did) = did {
                let pad = COMMENT_COLUMN
                    .saturating_sub(cells_of(&text[start..]))
                    .max(1);
                let _ = write!(text, "{:pad$};; {did}", "");
            }
            text.push('\n');
        }
        text
    }
}

/// Whether input goes where secrets may be: a buffer that may hold them, a passphrase prompt,
/// or anything the echo area and the control socket withhold as around such a buffer, such
/// as a prompt, an isearch or a jump over it.
pub(super) fn typing_in_secret(editor: &Editor) -> bool {
    editor.current().1.is_secret()
        || editor.around_secret()
        || editor
            .prompts()
            .iter()
            .any(|prompt| prompt.kind() == Kind::Passphrase)
}

impl Dispatcher {
    /// Keeps `key`, which the window or the socket gave, before it is handled.
    pub(super) fn lossage_key(&mut self, editor: &Editor, key: KeyPress) {
        let kept = if typing_in_secret(editor) {
            KeptAs::Secret
        } else {
            KeptAs::Key(key)
        };
        // A key that joined a run of hidden ones has no entry of its own to note on.
        self.lossage.handling = self.lossage.push(kept);
    }

    /// The key being handled has been handled: nothing later is noted on it.
    pub(super) fn lossage_close(&mut self) {
        self.lossage.handling = false;
    }

    /// The key being handled went to a terminal's program: a character it types is hidden,
    /// and joins a run of hidden keys before it.
    pub(super) fn lossage_sent(&mut self, key: KeyPress) {
        let Some(entry) = self.lossage.handled() else {
            return;
        };
        if key.text().is_some() && entry.kept == KeptAs::Key(key) {
            self.lossage.entries.pop_back();
            if self.lossage.newest_hides() {
                self.lossage.handling = false;
                return;
            }
            let _ = self.lossage.push(KeptAs::SentCharacter);
        }
        self.lossage.note(Did::SentToTerminal);
    }

    /// The key being handled was taken before it ran: if it did nothing and is no `C-g`, which
    /// stops what holds keys, it is held to run later, and kept as `<held>`, with no key, as
    /// where it runs is not known yet; a run of them is one line. It is kept again then.
    pub(super) fn lossage_held(&mut self, key: KeyPress) {
        if let Some(entry) = self.lossage.handled()
            && entry.kept == KeptAs::Key(key)
            && entry.did == Did::Nothing
            && !is_cancel(key)
        {
            entry.kept = KeptAs::Held;
            // A run of held keys is one line, which tells no length.
            let len = self.lossage.entries.len();
            if len >= 2 && self.lossage.entries[len - 2].kept == KeptAs::Held {
                self.lossage.entries.pop_back();
            }
        }
        self.lossage.handling = false;
    }

    /// The key sequence being handled is undefined: the key being handled says so.
    pub(super) fn note_undefined(&mut self) {
        self.lossage.note(Did::Undefined);
    }

    /// The key sequence being handled is undefined: the key being handled says so, then the
    /// keys are forgotten.
    pub(super) fn note_undefined_and_forget(&mut self, editor: &mut Editor) {
        self.note_undefined();
        self.forget_keys(editor);
    }

    pub(super) fn lossage_ran(&mut self, id: CommandId) {
        self.lossage.note(Did::Ran(id));
    }

    /// Keeps a key the window gave that oso has no name for, as `<no Emacs name>`, so that
    /// `*Lossage*` tells a key that came but cannot be bound from one that never came; as
    /// `<hidden>` where secrets may be.
    pub fn note_unnamed_key(&mut self, editor: &Editor) {
        let kept = if typing_in_secret(editor) {
            KeptAs::Secret
        } else {
            KeptAs::Unnamed
        };
        let _ = self.lossage.push(kept);
        self.lossage.handling = false;
    }

    /// Shows `*Lossage*` if `id`, which ran without an error, is `view-lossage`, with the
    /// cursor after the newest key, as in Emacs. Returns whether it was.
    pub(super) fn answer_view_lossage(&mut self, id: CommandId, editor: &mut Editor) -> bool {
        if Some(id) != self.lossage.command {
            return false;
        }
        let text = self.lossage.text(&self.registry);
        if show_internal(editor, &mut self.lossage.buffer, LOSSAGE, &text, true) {
            editor.current_mut().0.selections = SelectionSet::new(text.len());
        }
        true
    }
}
