//! A native session's slash commands in the app (ADR 0165, step 2d): the list its stream gives
//! kept, `native-composer-command`'s list made from the selected composer's session, and a
//! first message that would clear the conversation held in the composer. The rules are
//! `sessions::native::slash`'s; the composer's send shows a command's row and refuses
//! `/clear`.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - The rows completion offers are made once per change of a session's list, and given again
//!   only when the selected composer's session, or its list, changed: not per event.
//! - A row says each command's argument hint and description in its annotation, and marks a
//!   command that is not the CLI's own: a project's skill may take a built-in's name.

use std::sync::Arc;

use oso_core::buffer::Mode;
use oso_core::complete::Candidate;

use super::Native;
use crate::app::App;
use crate::commands::agent_sessions::SLASH_LIST;
use crate::sessions::native::rows::RowChange;
use crate::sessions::native::wire::Event;
use crate::sessions::native::wire::slash::SlashCommand;
use crate::sessions::worker::SessionKey;

/// Notes what `event` of a native session's `system/init` lists of its slash commands, and
/// whether its conversation moved to another session id, as `system/init` and `result` say it;
/// the control layer gives `initialize`'s commands. The row that says it moved, once.
pub(super) fn commands_event(native: &mut Native, event: &mut Event) -> Option<RowChange> {
    let id = match event {
        Event::Init(init) => {
            let names = std::mem::take(&mut init.slash_commands);
            let terminal = std::mem::take(&mut init.terminal_commands);
            native.commands.system_init(names, terminal);
            &init.session_id
        }
        // A `/clear`'s own turn ends in the new id, though no turn may follow it.
        Event::Result(result) => &result.session_id,
        _ => return None,
    };
    // A command that got by, as a `/clear` would, gave it a new id: oso follows the one it
    // started (a resume or a compaction keeps it, measured), so nothing resumes it.
    match &native.cli_id {
        None if !id.is_empty() => native.cli_id = Some(id.clone()),
        Some(first) if first != id && !id.is_empty() && !native.moved => {
            native.moved = true;
            return Some(native.rows.note(MOVED));
        }
        _ => {}
    }
    None
}

/// What a session whose conversation moved to another id says.
const MOVED: &str = "Claude Code moved this conversation to a new session id, which oso does not \
follow: a resume would reach the conversation before, so it does not resume.";

/// The completion row of `command`.
fn row(command: &SlashCommand, marked: bool) -> Candidate {
    let mut said = String::new();
    if marked && !command.builtin {
        said.push_str("not built in · ");
    }
    if !command.hint.is_empty() {
        said.push_str(&command.hint);
        said.push_str("  ");
    }
    said.push_str(&command.description);
    Candidate::new(command.name.as_str(), "", said)
}

impl App {
    /// After each event: the slash commands `native-composer-command` completes over are the
    /// selected composer's session's, given again when that session or its list changed.
    pub(super) fn sync_slash_commands(&mut self) {
        let selected = self.editor.window().buffer();
        let composer = self.editor.buffer(selected).map(|b| b.mode()) == Some(Mode::Composer);
        let Some(key) = composer.then(|| self.composer_session(selected)).flatten() else {
            return;
        };
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let version = native.commands.version();
        if self.hosting.natives.slash_listed == Some((key, version)) {
            return;
        }
        let items = match &native.slash_rows {
            Some((made, items)) if *made == version => Arc::clone(items),
            _ => {
                // Names alone say nothing of whose a command is.
                let marked = native.commands.says_whose();
                let items: Arc<[Candidate]> = (native.commands.offered())
                    .map(|c| row(c, marked))
                    .collect();
                native.slash_rows = Some((version, Arc::clone(&items)));
                items
            }
        };
        self.editor.set_listed(SLASH_LIST, items);
        self.hosting.natives.slash_listed = Some((key, version));
    }

    /// Puts `text`, a first message that would clear the conversation, in native session
    /// `key`'s composer instead, saying why.
    pub(super) fn hold_in_composer(&mut self, key: SessionKey, text: &str) {
        self.set_composer_text(key, text);
        self.editor.message(format!(
            "{}; the first message waits in the composer",
            crate::sessions::native::slash::CLEAR_REFUSED
        ));
    }
}
