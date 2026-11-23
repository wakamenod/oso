//! What is sent to a native session, carried out (ADR 0165, part 2f): the delivery path's
//! native route puts it into the session's composer at point, not sent, the person sending it
//! (the author's Q1 (a), as ADR 0121's Q17 keeps a terminal's paste unsubmitted); and a review's
//! question the person confirmed in oso's prompt goes at once, or waits behind the turn under
//! way, the composer's draft untouched (Q4 (a)): with `submit_paste`, the submits ADR 0163
//! allows.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - Both are escaped once, as `Outgoing` says, what goes into a draft also so that it makes
//!   no `@` of the person's after it the start of a word.
//! - Nothing goes into a composer that would then hold more than [`MAX_MESSAGE`], measured
//!   before anything is copied, nor into one a yank turned secret, which sends nothing; a
//!   stopped session takes no question, as it is resumed first.

use oso_core::buffer::{BufferId, Mode, Replacement};
use oso_core::window::DockSlot;

use crate::app::App;
use crate::app::sessions::Outgoing;
use crate::sessions::native::queue::{MAX_MESSAGE, MAX_QUEUED, Refusal, Run, Sent};
use crate::sessions::send;
use crate::sessions::worker::SessionKey;

/// What a composer that would hold too much says.
fn too_much() -> String {
    format!(
        "The composer would hold over {} KiB: nothing was put in",
        MAX_MESSAGE >> 10
    )
}

impl App {
    /// Puts `text` into native session `key`'s composer at its point, escaped as it says;
    /// with `show`, the session is shown first, point in its composer after the text. The
    /// composer's buffer, or why nothing was put in.
    pub(in crate::app) fn compose_into(
        &mut self,
        key: SessionKey,
        text: Outgoing<'_>,
        show: bool,
    ) -> Result<BufferId, String> {
        // `claude -p` runs no `/add-dir`, which the composer's send would carry unescaped.
        if let Outgoing::Command(_) = text {
            return Err("A native session takes no slash command of oso's".to_owned());
        }
        let transcript = (self.hosting.natives.map.get(&key)).map(|native| native.buffer);
        let transcript = transcript.ok_or_else(|| "That session is gone".to_owned())?;
        let id = (self.composer(key)).ok_or_else(|| "The session has no composer".to_owned())?;
        let buffer = (self.editor.buffer(id)).ok_or_else(|| "The composer is gone".to_owned())?;
        if buffer.mode() != Mode::Composer {
            return Err("The composer is no longer one".to_owned());
        }
        if buffer.is_secret() {
            return Err(
                "The composer holds secret text, which is not sent: take it out".to_owned(),
            );
        }
        // Measured before it is copied: escaping never shortens it.
        let len = buffer.text().len();
        if len.saturating_add(text.len()) > MAX_MESSAGE {
            return Err(too_much());
        }
        let window = self.editor.dock_window(transcript, DockSlot::Composer);
        let cursor = (window.and_then(|id| self.editor.window_by_id(id)))
            .map(|window| window.selections.primary().cursor);
        let at = cursor.map_or(len, |cursor| cursor.min(len));
        let next = (buffer.text().chunks_in_range(at..len)).find_map(|chunk| chunk.chars().next());
        let text = text.escaped(next);
        if len.saturating_add(text.len()) > MAX_MESSAGE {
            return Err(too_much());
        }
        if show {
            self.show_session(key);
            self.composer_go(key, true);
        }
        let edit = Replacement {
            range: at..at,
            text: &text,
        };
        (self.editor.apply_edits(id, &[edit])).map_err(|_| "Nothing was put in".to_owned())?;
        Ok(id)
    }

    /// Where native session `key`'s process is, for a question held until it runs
    /// (ADR 0176, part O-2): `None` for a session that is not a native one.
    pub(in crate::app) fn native_run(&self, key: SessionKey) -> Option<Run> {
        self.turn(key).map(|turn| turn.run)
    }

    /// Sends `text`, which the person confirmed in oso's prompt, to native session `key`: at
    /// once, or queued behind the turn under way, escaped, the composer's draft and a paused
    /// queue left as they are. What to say, or why it was not sent: a stopped session is
    /// resumed first.
    pub(in crate::app) fn send_confirmed(
        &mut self,
        key: SessionKey,
        text: &str,
    ) -> Result<String, String> {
        let turn = self
            .turn(key)
            .ok_or_else(|| "That session is gone".to_owned())?;
        if turn.run == Run::Ended {
            let resume = self.dispatcher.where_is(&self.editor, "session-resume");
            return Err(format!("it has stopped: {resume} first"));
        }
        // Measured before it is copied.
        if text.len() > MAX_MESSAGE {
            return Err(format!("it is over {} KiB", MAX_MESSAGE >> 10));
        }
        let text = send::for_claude(text, None).into_owned();
        let native = (self.hosting.natives.map.get_mut(&key))
            .ok_or_else(|| "That session is gone".to_owned())?;
        let paused = native.composing.queue.paused().is_some();
        let said = match native.composing.queue.send_confirmed(text, turn) {
            Sent::Now(text) if self.deliver(key, &text) => Ok("sent".to_owned()),
            Sent::Now(_) => {
                self.not_sent(key, None);
                Err("its input took nothing".to_owned())
            }
            Sent::Queued(count) if paused => {
                let send = self
                    .dispatcher
                    .where_is(&self.editor, "native-composer-send");
                Ok(format!(
                    "queued as {count}, behind a paused queue: {send} in its composer goes on"
                ))
            }
            Sent::Queued(count) => Ok(format!("queued as {count}, to go once the turn ends")),
            Sent::Refused(Refusal::TooLong) => Err(format!("it is over {} KiB", MAX_MESSAGE >> 10)),
            Sent::Refused(Refusal::Full) => Err(format!("its queue holds {MAX_QUEUED} already")),
            _ => Err("it took nothing".to_owned()),
        };
        self.draw_queue(key);
        said
    }
}
