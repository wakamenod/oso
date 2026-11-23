//! When point moves to a native session's permission card, and since when the card, or the
//! session's empty composer, is where an answer may come from (ADR 0165, step 2e-2; ADR 0180).
//!
//! Must not depend on: key presses or how a frame is drawn; the loop says when an event ended
//! and what is due.
//!
//! - As a card appears, point moves to it at once, while the person is in its session's
//!   transcript, or its composer with nothing in it, and in the middle of nothing: no key
//!   sequence, prompt, search, menu, composition, macro or drag. The card takes no answer until
//!   [`QUIET`] passed since, with no key, so a key typed as it appears never answers it.
//! - A card that found the person elsewhere, or writing, waits: point moves once that is no
//!   more and [`QUIET`] passed with no key, so that clearing a draft is not the move's moment.
//! - A card cut by its window takes no point, as it takes no answer. The wait starts again as a
//!   window comes to show all of a card, shows it at another size, or is in front again: it is
//!   measured from when the person could see what they answer.

use std::time::{Duration, Instant};

use oso_core::buffer::{BufferId, Mode};
use oso_core::window::DockSlot;

use super::Current;
use crate::app::App;
use crate::app::agent::QUIET;
use crate::sessions::worker::SessionKey;

/// How much longer a card waits once a prompt an agent opened closed.
const AFTER_AGENT: Duration = Duration::from_millis(1500);

/// When what came in front of the person at `now` waits from, a prompt an agent opened having
/// closed at `closed`, if one has.
fn waits_from(now: Instant, closed: Option<Instant>) -> Instant {
    closed.map_or(now, |closed| now.max(closed + AFTER_AGENT))
}

impl App {
    /// After each event: where an answer to a card may come from now, if anywhere, and since
    /// when: the place coming to be one, a prompt over it closing, its card's window coming to
    /// show all of it, or an agent's prompt closing, longer.
    pub(in crate::app::sessions::hosting::native) fn note_current_card(&mut self) {
        self.note_agents_prompt();
        let place = (self.answering_place()).filter(|_| self.editor.prompt().is_none());
        let Some((key, place)) = place else {
            self.hosting.natives.permissions.current = None;
            return;
        };
        let whole = self.card_shows_all(key);
        let permissions = &mut self.hosting.natives.permissions;
        let since = waits_from(Instant::now(), permissions.agents_prompt_closed);
        match &mut permissions.current {
            Some(now) if now.key == key && now.place == place => {
                if whole && !now.whole {
                    now.since = now.since.max(since);
                }
                now.whole = whole;
            }
            current => {
                *current = Some(Current {
                    key,
                    place,
                    since,
                    whole,
                });
            }
        }
    }

    /// After each event, with a card or with none: when a prompt an agent opened last closed. A key
    /// the person meant for it may come just after, so a card waits longer then.
    pub(in crate::app::sessions::hosting::native) fn note_agents_prompt(&mut self) {
        let open = (self.editor.prompts().iter()).any(|prompt| prompt.from_agent());
        let permissions = &mut self.hosting.natives.permissions;
        if permissions.agents_prompt && !open {
            permissions.agents_prompt_closed = Some(Instant::now());
        }
        permissions.agents_prompt = open;
    }

    /// What session `key`'s card shows is new to the person, or they come back to oso's window: if
    /// it is the one in front, its wait starts again.
    pub(super) fn wait_on_card_again(&mut self, key: SessionKey) {
        if let Some(current) = &mut self.hosting.natives.permissions.current
            && current.key == key
        {
            current.since = current.since.max(Instant::now());
        }
    }

    /// oso's window came in front, or in sight, again: the card in front waits again, as the
    /// person may see it only now.
    pub(in crate::app) fn card_seen_again(&mut self) {
        if let Some(current) = self.hosting.natives.permissions.current {
            self.wait_on_card_again(current.key);
        }
    }

    /// The selected tiled buffer, or the one beneath the prompts, if a card may be answered from
    /// it, and its session: a card, or the empty composer of a session whose card shows.
    fn answering_place(&self) -> Option<(SessionKey, BufferId)> {
        let (window, buffer) = self.editor.tiled();
        let selected = window.buffer();
        match buffer.mode() {
            Mode::Card => Some((self.card_session(selected)?, selected)),
            Mode::Composer => {
                let key = self.composer_session(selected)?;
                let shown = self.hosting.natives.permissions.asks.shown(key).is_some();
                let empty = self.composer_empty(key) && window.selections.len() == 1;
                (shown && empty && !self.composing()).then_some((key, selected))
            }
            _ => None,
        }
    }

    /// When point is next to move to a card, if one waits for it: [`QUIET`] after it was
    /// shown and after the person's last key, later than the last look.
    pub(in crate::app) fn card_focus_deadline(&self) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        let typed = self.agent_person_typed();
        let looked = permissions.looked;
        (permissions.focus_pending.iter())
            .map(|&(_, shown)| typed.map_or(shown, |typed| typed.max(shown)) + QUIET)
            .filter(|&due| looked.is_none_or(|looked| due > looked))
            .min()
    }

    /// Before the loop waits: point goes to a card that waits for it, once [`QUIET`] passed
    /// with no key, while the person is in its session's transcript, or its composer with no
    /// draft in it.
    pub(in crate::app) fn card_focus_due(&mut self, now: Instant) {
        self.note_current_card();
        let permissions = &mut self.hosting.natives.permissions;
        if permissions.focus_pending.is_empty() {
            return;
        }
        permissions.looked = Some(now);
        let typing = self
            .agent_person_typed()
            .is_some_and(|typed| now < typed + QUIET);
        if typing || self.person_in_the_middle() {
            return;
        }
        let Some(key) = self.session_in_front() else {
            return;
        };
        let due = (self.hosting.natives.permissions.focus_pending.iter())
            .any(|&(pending, shown)| pending == key && now >= shown + QUIET);
        if due {
            self.focus_card(key);
        }
    }

    /// As session `key`'s card appears: point goes to it at once (ADR 0180), unless the person
    /// is elsewhere or in the middle of something, where the card waits for them.
    pub(super) fn focus_card_at_once(&mut self, key: SessionKey) {
        if self.session_in_front() == Some(key) && !self.person_in_the_middle() {
            self.focus_card(key);
        }
    }

    /// The session whose transcript, or whose composer with nothing in it, is the selected
    /// buffer: a draft in the composer keeps point there, and the card waits, and says so.
    fn session_in_front(&self) -> Option<SessionKey> {
        let selected = self.editor.window().buffer();
        (self.hosting.natives.map.iter()).find_map(|(key, native)| {
            let composer = native.composing.buffer == Some(selected);
            let here = selected == native.buffer || (composer && self.composer_empty(*key));
            here.then_some(*key)
        })
    }

    /// Whether the person is in the middle of what a move of point would break: the next key,
    /// or the button's release, is meant for where point is.
    fn person_in_the_middle(&self) -> bool {
        let editor = &self.editor;
        editor.prompt().is_some()
            || self.dispatcher.reads_keys()
            || self.dispatcher.keys_in_progress()
            || self.composing()
            || self.terminals.button_held()
            || editor.isearch().is_some()
            || editor.jump().is_some()
            || editor.replace().is_some()
            || editor.macro_recording()
            || editor.macro_replaying()
            || self.dispatcher.waits_for_clipboard()
            || self.dispatcher.holds_paste()
            || crate::dispatch::holds_input(editor)
    }

    /// Point goes to session `key`'s card, if its window shows all of it.
    fn focus_card(&mut self, key: SessionKey) {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return;
        };
        let Some(window) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
            return;
        };
        if !self.card_shows_all(key) {
            return;
        }
        self.editor.select_window(window);
        self.note_current_card();
        (self.hosting.natives.permissions.focus_pending).retain(|&(pending, _)| pending != key);
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{AFTER_AGENT, waits_from};

    #[test]
    fn a_card_waits_longer_after_a_prompt_an_agent_opened_closed() {
        let now = Instant::now();
        assert_eq!(waits_from(now, None), now);
        assert_eq!(waits_from(now, Some(now)), now + AFTER_AGENT);
        // A card that comes a little later waits for what is left of it.
        let later = now + Duration::from_secs(1);
        assert_eq!(waits_from(later, Some(now)), now + AFTER_AGENT);
        let long = now + 2 * AFTER_AGENT;
        assert_eq!(waits_from(long, Some(now)), long);
    }
}
