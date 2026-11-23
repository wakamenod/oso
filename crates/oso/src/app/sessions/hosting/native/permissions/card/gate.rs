//! Whether a native session's permission card takes an answer now (ADR 0165, step 2e-2): the
//! one check of both the card's keys and the dev state, so that end-to-end flows wait for
//! exactly what the card checks.
//!
//! Must not depend on: key presses or how a frame is drawn; the card's keys ask, and this
//! answers.

use std::time::Instant;

use oso_core::window::DockSlot;

use super::plan::answered_in_time;
use crate::app::App;
use crate::sessions::worker::SessionKey;

impl App {
    /// The gate of session `key`'s card's answers: whether it takes one now; else the echo
    /// area says why.
    pub(super) fn card_admits_answer(&mut self, key: SessionKey) -> bool {
        // The key being answered is the last one: the quiet moment is since the one before.
        let (before, now) = (self.agent_key_before(), Instant::now());
        let Some(refusal) = self.card_refusal(key, before, now) else {
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

    /// Why session `key`'s card takes no answer at `now`, if it takes none; `before` is when
    /// the person typed the key before the answer. The one check of both the card's keys and the dev
    /// state, so that end-to-end flows wait for exactly what the card checks.
    fn card_refusal(
        &self,
        key: SessionKey,
        before: Option<Instant>,
        now: Instant,
    ) -> Option<Refusal> {
        if self.editor.macro_replaying() {
            return Some(Refusal::Macro);
        }
        let card = (self.hosting.natives.map.get(&key)).and_then(|native| native.card);
        let current = self.hosting.natives.permissions.current;
        let Some(current) = current.filter(|current| card == Some(current.card)) else {
            return Some(Refusal::NotCurrent);
        };
        if !answered_in_time(current.since, before, now) {
            return Some(Refusal::Wait);
        }
        if !self.card_shows_all(key) {
            return Some(Refusal::Cut);
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
            .find_map(|&(drawn, width)| (drawn == key).then_some(width));
        if drawn.is_none_or(|width| regions.text.columns < width) || buffer.wraps() {
            return false;
        }
        let whole = (self.hosting.natives.permissions.asks.shown(key))
            .and_then(|ask| ask.question.as_ref())
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

    /// Whether the selected card, if one is, takes an answer now: what its keys check, with
    /// the next key the answer. For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn card_takes_answer(&self) -> Option<bool> {
        let key = self.card_session(self.editor.window().buffer())?;
        // A card with no call shown takes no answer, as its keys say before this gate.
        (self.hosting.natives.permissions.asks.shown(key)).and_then(|ask| ask.question.as_ref())?;
        // The next key's key before is the last one now.
        let refusal = self.card_refusal(key, self.agent_person_typed(), Instant::now());
        Some(refusal.is_none())
    }
}

/// Why a card takes no answer yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// A macro replays.
    Macro,
    /// The card is not the selected one point came to.
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
