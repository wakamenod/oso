//! Whether a native session's permission card takes an answer now (ADR 0165, step 2e-2): the
//! one check of the card's keys, of the composer's that answer it (ADR 0180) and of the dev state,
//! so that end-to-end flows wait for exactly what the card checks. And whether its plan's edit
//! takes an approval (part 3d), as the edit's keys and the dev state check it.
//!
//! Must not depend on: key presses or how a frame is drawn; the card's keys ask, and this
//! answers.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::window::DockSlot;

use super::Current;
use super::plan::answered_in_time;
use crate::app::App;
use crate::sessions::native::card::NARROWEST;
use crate::sessions::worker::SessionKey;

impl App {
    /// The gate of session `key`'s card's answers: whether it takes one from buffer `from`, the
    /// card or the session's composer (ADR 0180), now; else the echo area says why.
    pub(super) fn card_admits_answer(&mut self, key: SessionKey, from: BufferId) -> bool {
        // The key being answered is the last one: the quiet moment is since the one before, up to
        // when the person pressed it, also where it was held back and comes later.
        let before = self.agent_key_before();
        let now = self.agent_person_typed().unwrap_or_else(Instant::now);
        let Some(refusal) = self.card_refusal(key, from, before, now) else {
            return true;
        };
        // Which check refused, and how long since what it measures, so that an end-to-end
        // flow's refused answer can be traced. Only times, never the card's text.
        let since = (self.hosting.natives.permissions.current)
            .filter(|_| refusal != Refusal::NotCurrent)
            .map(|current| current.since);
        let ms = |at: Option<Instant>| {
            at.map_or_else(
                || "none".to_owned(),
                |at| format!("{} ms", now.saturating_duration_since(at).as_millis()),
            )
        };
        log::debug!(
            "native sessions: a card took no answer: {refusal:?}; point came {} ago, the key before {} ago",
            ms(since),
            ms(before),
        );
        // Point came to it with this key: the wait starts now.
        if refusal == Refusal::NotCurrent {
            self.note_current_card();
        }
        self.editor.message(refusal.message());
        false
    }

    /// The gate of session `key`'s plan's edit's approvals: whether the edit, in `buffer`, takes
    /// one now; else the echo area says why.
    pub(super) fn edit_admits_approval(&mut self, key: SessionKey, buffer: BufferId) -> bool {
        let now = self.agent_person_typed().unwrap_or_else(Instant::now);
        let Some(refusal) = self.edit_refusal(key, buffer, now) else {
            return true;
        };
        // Point came to it with this key: the wait starts now.
        if refusal == Refusal::NotCurrent {
            self.note_current_card();
        }
        log::debug!("native sessions: a plan's edit took no approval: {refusal:?}");
        self.editor.message(match refusal {
            Refusal::Macro => "A macro does not approve a plan",
            _ => "Wait a moment, then approve the plan",
        });
        false
    }

    fn edit_refusal(&self, key: SessionKey, buffer: BufferId, now: Instant) -> Option<Refusal> {
        if self.editor.macro_replaying() {
            return Some(Refusal::Macro);
        }
        edit_refusal(self.hosting.natives.permissions.current, key, buffer, now)
    }

    /// Whether the plan's edit selected, if one is, takes an approval now: what its keys check.
    /// For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn plan_takes_answer(&self) -> Option<bool> {
        let buffer = self.editor.window().buffer();
        let key = self.plan_session(buffer)?;
        Some(self.edit_refusal(key, buffer, Instant::now()).is_none())
    }

    /// Why session `key`'s card takes no answer from buffer `from` at `now`, if it takes none;
    /// `before` is when the person typed the key before the answer. The one check of the card's
    /// keys, of the composer's that answer it and of the dev state, so that end-to-end flows wait
    /// for exactly what the card checks.
    pub(super) fn card_refusal(
        &self,
        key: SessionKey,
        from: BufferId,
        before: Option<Instant>,
        now: Instant,
    ) -> Option<Refusal> {
        if self.editor.macro_replaying() {
            return Some(Refusal::Macro);
        }
        let current = self.hosting.natives.permissions.current;
        let Some(current) = current.filter(|now| now.key == key && now.place == from) else {
            return Some(Refusal::NotCurrent);
        };
        if !answered_in_time(current.since, before, now) {
            return Some(Refusal::Wait);
        }
        if !self.card_shows_all(key) {
            return Some(Refusal::Cut);
        }
        // Whole only since it was last looked at: the person has not seen it.
        if !current.whole {
            return Some(Refusal::Wait);
        }
        None
    }

    /// Whether session `key`'s card's window shows every row of it, from its start, at the
    /// width it was drawn at, with no row wrapped.
    pub(super) fn card_shows_all(&self, key: SessionKey) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let Some(buffer) = native.card.and_then(|card| self.editor.buffer(card)) else {
            return false;
        };
        let Some(id) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
            return false;
        };
        let (Some(window), Some(regions)) = (self.editor.window_by_id(id), self.editor.regions(id))
        else {
            return false;
        };
        let drawn = (self.hosting.natives.permissions.drawn.iter())
            .find_map(|&(drawn, (width, _))| (drawn == key).then_some(width));
        let columns = regions.text.columns;
        if columns < NARROWEST || drawn.is_none_or(|width| columns < width) || buffer.wraps() {
            return false;
        }
        let whole = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question())
            .is_some_and(|question| question.whole);
        // The whole view scrolls, its keys on its title row; the card shows from its start.
        let scroll = window.scroll();
        match whole {
            true => regions.text.rows > 0 && scroll.left == 0,
            false => {
                scroll.start == 0
                    && scroll.left == 0
                    && buffer.text().line_count() <= regions.text.rows
            }
        }
    }

    /// Whether the selected card, or the card of the selected composer's session, if one is,
    /// takes an answer from there now: what its keys check, with the next key the answer. For
    /// end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn card_takes_answer(&self) -> Option<bool> {
        let from = self.editor.window().buffer();
        let key = (self.card_session(from)).or_else(|| self.composer_session(from))?;
        // A card with no call shown takes no answer, as its keys say before this gate.
        (self.hosting.natives.permissions.asks.shown(key)).and_then(|ask| ask.question())?;
        // The next key's key before is the last one now.
        let refusal = self.card_refusal(key, from, self.agent_person_typed(), Instant::now());
        Some(refusal.is_none())
    }
}

/// Why session `key`'s plan's edit in `buffer` takes no approval at `now`, the time of the
/// approval's key, if it takes none: the wait is since the edit became the selected buffer, as a
/// card's, but not since the key before, as the person writes the edit and then approves it.
fn edit_refusal(
    current: Option<Current>,
    key: SessionKey,
    buffer: BufferId,
    now: Instant,
) -> Option<Refusal> {
    let Some(current) = current.filter(|at| at.key == key && at.place == buffer) else {
        return Some(Refusal::NotCurrent);
    };
    (!answered_in_time(current.since, None, now)).then_some(Refusal::Wait)
}

/// Why a card takes no answer yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// A macro replays.
    Macro,
    /// The card, or its session's empty composer, is not the selected buffer point came to.
    NotCurrent,
    /// Its quiet moment has not passed since point came to it or since the last key.
    Wait,
    /// Its window shows less than all of it.
    Cut,
}

impl Refusal {
    /// What the echo area says of it.
    fn message(self) -> &'static str {
        match self {
            Self::Macro => "A macro does not answer a card",
            Self::NotCurrent | Self::Wait => "Wait a moment, then answer the card",
            Self::Cut => "The card is cut: make the window taller to answer it",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use oso_core::buffer::Buffer;

    use super::{Current, Refusal, edit_refusal};
    use crate::app::agent::QUIET;

    #[test]
    fn an_edit_is_approved_only_after_a_quiet_moment_since_it_came_in_front() {
        let mut editor = oso_core::editor::Editor::default();
        let edit = editor.add_buffer(Buffer::from_text("*plan: s*", ""));
        let other = editor.add_buffer(Buffer::from_text("*plan: t*", ""));
        let since = Instant::now();
        let current = |place| {
            Some(Current {
                key: 7,
                place,
                since,
                whole: false,
            })
        };
        let late = since + QUIET;
        assert_eq!(edit_refusal(current(edit), 7, edit, late), None);
        let soon = since + Duration::from_millis(80);
        assert_eq!(
            edit_refusal(current(edit), 7, edit, soon),
            Some(Refusal::Wait)
        );
        // Another buffer, or another session's, came in front.
        for (at, key) in [(None, 7), (current(other), 7), (current(edit), 8)] {
            assert_eq!(edit_refusal(at, key, edit, late), Some(Refusal::NotCurrent));
        }
        // An agent's prompt that closed lately put the moment off.
        let after = Current {
            since: since + Duration::from_millis(1500),
            ..current(edit).unwrap()
        };
        assert_eq!(
            edit_refusal(Some(after), 7, edit, late),
            Some(Refusal::Wait)
        );
    }
}
