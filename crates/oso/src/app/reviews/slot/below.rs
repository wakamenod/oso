//! The session below the review layout's slot as the layout is left and put back (ADR 0184,
//! amending ADR 0169): kept by its record name, shown below the slot again while it is live
//! and hosted, else named as ended.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here starts, resumes or
//! signals a session: the hosted table is only read.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::window::WindowId;

use super::super::super::App;
use super::laid::SlotKinds;
use super::windows::{ask_window, fit_ask_to, split_below};
use crate::sessions::hosted::{Hosted, Run};
use crate::sessions::spaces::kept::{self, Kept};

/// The record name of the session the layout shows below its slot, else the one `review-ask`
/// showed in the slot itself, while it is a hosted session's: none for a buffer that may hold
/// secrets or is withheld from agents (ADR 0184).
pub(super) fn session_shown(
    editor: &Editor,
    kept: &[Kept],
    slot: WindowId,
    kinds: &SlotKinds<'_>,
) -> Option<String> {
    let laid = kept::review(kept)?;
    let below = ask_window(editor, kept).and_then(|ask| editor.window_by_id(ask));
    let filling = (editor.window_by_id(slot)).filter(|window| Some(window.buffer()) == laid.asked);
    let buffer = below.or(filling)?.buffer();
    let shown = editor.buffer(buffer)?;
    if shown.is_secret() || shown.withheld_from_agents() {
        return None;
    }
    (kinds.session)(buffer)
}

/// What goes below the slot as a layout is put back, for the session it showed there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app::reviews) enum SessionBack {
    /// Its buffer: the session is live and hosted.
    Show(BufferId),
    /// It ended, or is no longer hosted: the person is told, by this name, and nothing is
    /// started (ADR 0184).
    Ended(String),
}

/// What goes below the slot for the session of record name `name` among `sessions`: its
/// buffer while it runs and its buffer may be shown, else that it ended; nothing while it
/// starts, or while its buffer is gone, may hold secrets or is withheld from agents. Starts
/// nothing.
pub(in crate::app::reviews) fn session_back<'a>(
    editor: &Editor,
    sessions: impl IntoIterator<Item = &'a Hosted>,
    name: &str,
) -> Option<SessionBack> {
    // A name the record gave two sessions: the one running, if any.
    let mut found = None;
    for session in sessions.into_iter().filter(|session| session.name == name) {
        let runs = matches!(session.run, Run::Running { .. } | Run::Starting);
        if runs || found.is_none() {
            found = Some(session);
        }
        if runs {
            break;
        }
    }
    let Some(session) = found else {
        return Some(SessionBack::Ended(name.to_owned()));
    };
    match session.run {
        Run::Running { .. } => {
            let buffer = session.buffer?;
            let shown = editor.buffer(buffer)?;
            (!shown.is_secret() && !shown.withheld_from_agents())
                .then_some(SessionBack::Show(buffer))
        }
        Run::Starting => None,
        Run::Stopped | Run::Failed => Some(SessionBack::Ended(name.to_owned())),
    }
}

/// Shows the session's buffer `buffer` below the slot of the review layout in front, as a
/// layout is put back: in the window below the slot while there is one, else in one split off
/// below it, else in the slot, too small to split (ADR 0169); the split below put back to
/// `ask_permille`. The selected window stays. Returns whether it showed it.
pub(in crate::app::reviews) fn session_below(
    editor: &mut Editor,
    kept: &mut [Kept],
    buffer: BufferId,
    ask_permille: Option<u16>,
) -> bool {
    if !kept::review_in_frame(kept) {
        return false;
    }
    let Some(slot) = kept::review(kept)
        .and_then(|laid| laid.slot)
        .filter(|slot| super::windows::in_tree(editor, *slot))
    else {
        return false;
    };
    let below = ask_window(editor, kept).or_else(|| split_below(editor, slot));
    editor.show_in(below.unwrap_or(slot), buffer);
    if let (Some(below), Some(permille)) = (below, ask_permille) {
        fit_ask_to(editor, slot, below, permille);
    }
    if let Some(laid) = kept::review_mut(kept) {
        laid.ask = below;
        laid.asked = Some(buffer);
    }
    true
}
impl App {
    /// The session of record name `name` goes below the slot of the layout in front, the split
    /// below put back to `ask_permille`, while it is live and hosted; else, if it ended, the
    /// person is told how to resume it, and nothing is started (ADR 0184).
    pub(super) fn put_back_session(&mut self, name: &str, ask_permille: Option<u16>) {
        match session_back(&self.editor, self.hosting.table().iter(), name) {
            Some(SessionBack::Show(buffer)) => {
                let kept = self.watch.spaces_mut().kept_mut();
                session_below(&mut self.editor, kept, buffer, ask_permille);
            }
            Some(SessionBack::Ended(name)) => {
                let name = crate::commands::review::one_line(&name, 40);
                self.editor
                    .message(format!("session {name} ended: M-x session-resume"));
            }
            None => {}
        }
    }
}
