//! The open menu as the host draws it (ADR 0117): the menu on top, its describe function's
//! notes, its heads' checks and the prefix argument for its title row, and the rules the
//! dispatcher and the draw share: what a head is now, and the head `RET` runs.
//!
//! Must not depend on: `app` or winit.

use oso_core::editor::Editor;
use oso_core::menu::{Definition, HeadAction, Lookup, Note, Notes, Open};

use super::super::shown::prefix_text;
use super::{Level, RET, Stack};
use crate::dispatch::Dispatcher;

/// The most bytes of a note or of a describe function's line that are kept, as the layout
/// measures them: a describe function is built in, but what it reads may be long.
const NOTE_BYTES: usize = 512;

/// The open menu, as the host draws it and the channel names it.
pub struct OpenMenu<'a> {
    pub(super) dispatcher: &'a Dispatcher,
    pub(super) stack: &'a Stack,
}

impl OpenMenu<'_> {
    /// The menu on top, the one shown.
    pub fn open(&self) -> &Open {
        &self.top().open
    }

    /// A number that changes whenever another menu is shown, so that what is worked out for
    /// one is never taken for another's.
    pub fn generation(&self) -> u64 {
        self.dispatcher.menus.changes
    }

    /// Puts what its describe function says of it now in `out`, or nothing for a menu with
    /// none.
    pub fn notes_into(&self, editor: &Editor, out: &mut Described) {
        let open = self.open();
        out.set(notes_of(editor, open), open.definition().heads().len());
    }

    /// Each head's check, by head: `Some(on)` for a switch, or for a command that says whether
    /// its state is on (`Command::state`), and `None` for any other.
    pub fn checks_into(&self, editor: &Editor, checks: &mut Vec<Option<bool>>) {
        let open = self.open();
        // The switches on come in the order of their heads, and a switch has one head.
        let mut on = open.switches_on().peekable();
        checks.clear();
        checks.extend(
            open.definition()
                .heads()
                .iter()
                .map(|head| match &head.action {
                    HeadAction::Switch(name) => Some(on.next_if_eq(&name.as_str()).is_some()),
                    HeadAction::Command(id) => {
                        let state = self.dispatcher.registry.get(*id)?.state?;
                        Some(state(editor))
                    }
                    HeadAction::Menu(_) | HeadAction::Quit => None,
                }),
        );
    }

    /// Writes the prefix argument being typed as the title row shows it, `C-u 5-`, into
    /// `out`, which it empties first; nothing for none.
    pub fn prefix_into(&self, out: &mut String) {
        out.clear();
        prefix_text(self.dispatcher.prefix, out);
        if !out.is_empty() {
            out.push('-');
        }
    }

    fn top(&self) -> &Level {
        // A stack is never left empty: closing its last level drops it.
        let last = self.stack.levels.len() - 1;
        &self.stack.levels[last]
    }
}

/// What a describe function says of its menu now, each string cut to 512 bytes, with each
/// head's note found once.
#[derive(Debug, Default)]
pub struct Described {
    notes: Notes,
    /// Each head's note, as an index into `notes.heads`, by head.
    by_head: Vec<Option<usize>>,
}

/// What a head of the open menu is now, by its describe function's notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadNow<'a> {
    /// As defined.
    Shown,
    /// Drawn dim, and its key says why it cannot run.
    Unavailable(&'a str),
    /// Left out of the float: its key is a foreign key.
    Hidden,
}

impl Described {
    /// The notes, cut.
    pub fn notes(&self) -> &Notes {
        &self.notes
    }

    /// The note on `head`, if there is one: the first given for it.
    pub fn get(&self, head: usize) -> Option<&Note> {
        let index = (*self.by_head.get(head)?)?;
        self.notes.heads.get(index).map(|(_, note)| note)
    }

    /// What `head` is now.
    pub fn now(&self, head: usize) -> HeadNow<'_> {
        match self.get(head) {
            Some(note) if note.hidden => HeadNow::Hidden,
            Some(Note {
                unavailable: Some(why),
                ..
            }) => HeadNow::Unavailable(why),
            _ => HeadNow::Shown,
        }
    }

    /// The head `RET` runs in `definition` now: the head on `RET`, else the default head,
    /// the one its describe function names or its own. A describe function names one only where
    /// no head is on `RET` (ADR 0117 step 2).
    pub fn ret_head(&self, definition: &Definition) -> Option<usize> {
        let default = self.default_head(definition);
        match definition.lookup(&[RET]) {
            Lookup::Head(head) => Some(head),
            Lookup::Unbound => default,
            Lookup::Pending | Lookup::Quit => None,
        }
    }

    /// The default head, marked in the float where `RET` runs it, whether or not its own keys
    /// are `RET` (ADR 0117 step 2).
    pub fn marked_head(&self, definition: &Definition) -> Option<usize> {
        let default = self.default_head(definition)?;
        (self.ret_head(definition) == Some(default)).then_some(default)
    }

    fn default_head(&self, definition: &Definition) -> Option<usize> {
        let named = self
            .notes
            .ret_head
            .filter(|&head| head < definition.heads().len());
        debug_assert!(
            named.is_none() || definition.lookup(&[RET]) == Lookup::Unbound,
            "a describe function named a default head in a menu with a head on RET"
        );
        named.or(definition.ret_head())
    }

    /// Takes `notes` on a menu of `heads` heads in place of those before, cut, keeping the
    /// index's room. A note on a head past them is never found.
    fn set(&mut self, mut notes: Notes, heads: usize) {
        if let Some(line) = &mut notes.line {
            cut(line);
        }
        self.by_head.clear();
        self.by_head.resize(heads, None);
        for (index, (head, note)) in notes.heads.iter_mut().enumerate() {
            cut(&mut note.text);
            if let Some(why) = &mut note.unavailable {
                cut(why);
            }
            if let Some(slot) = self.by_head.get_mut(*head) {
                slot.get_or_insert(index);
            }
        }
        self.notes = notes;
    }
}

/// Cuts `text` to [`NOTE_BYTES`] at a character's boundary.
fn cut(text: &mut String) {
    if text.len() > NOTE_BYTES {
        let mut end = NOTE_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// What `open`'s describe function says of it, or nothing for a menu with none.
pub(super) fn notes_of(editor: &Editor, open: &Open) -> Notes {
    open.definition()
        .describe()
        .map_or_else(Notes::default, |describe| describe(editor, open))
}

/// What `open`'s describe function says of it now, cut and indexed.
pub(super) fn described(editor: &Editor, open: &Open) -> Described {
    let mut out = Described::default();
    out.set(notes_of(editor, open), open.definition().heads().len());
    out
}
