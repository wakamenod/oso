//! The open menu as the host draws it (ADR 0117): the menu on top, its describe function's
//! notes, its heads' checks and the prefix argument for its title row.
//!
//! Must not depend on: `app` or winit.

use oso_core::editor::Editor;
use oso_core::menu::{HeadAction, Notes, Open};

use super::super::shown::prefix_text;
use super::{Level, Stack};
use crate::dispatch::Dispatcher;

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

    /// What its describe function says of it now, or nothing for a menu with none.
    pub fn notes(&self, editor: &Editor) -> Notes {
        notes_of(editor, self.open())
    }

    /// Each head's check, by head: `Some(on)` for a switch, or for a command that says whether
    /// its state is on (`Command::state`), and `None` for any other.
    pub fn checks_into(&self, editor: &Editor, checks: &mut Vec<Option<bool>>) {
        let open = self.open();
        checks.clear();
        checks.extend(
            open.definition()
                .heads()
                .iter()
                .map(|head| match &head.action {
                    HeadAction::Switch(name) => Some(open.switch(name)),
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

/// What `open`'s describe function says of it, or nothing for a menu with none.
pub(super) fn notes_of(editor: &Editor, open: &Open) -> Notes {
    open.definition()
        .describe()
        .map_or_else(Notes::default, |describe| describe(editor, open))
}
