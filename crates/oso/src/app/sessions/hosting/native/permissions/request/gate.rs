//! Whether a native session's call takes an answer now from its line (ADR 0182, "Point" 5): the one
//! check of the line's keys, of what the line shows of them and of the dev state, so that a key does
//! what the line says and end-to-end flows wait for what the keys check.
//!
//! Must not depend on: how a frame is drawn; the line's keys ask, and this answers. What was read is
//! asked apart.

use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::window::DockSlot;

use super::super::card::Current;
use crate::app::App;
use crate::app::agent::{QUIET, Typed};
use crate::app::sessions::hosting::native::carrier::carrier;
use crate::sessions::native::request::line::{NARROWEST, Says};
use crate::sessions::worker::SessionKey;

/// Why a line takes no answer now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// A macro replays.
    Macro,
    /// The line is not the selected buffer point came to, or oso's window is not in front.
    NotCurrent,
    /// Its quiet moment has not passed since point came to it, since its call came, or since the
    /// key before.
    Wait,
    /// Its window shows less than all of it, or is too narrow.
    Cut,
}

impl Refusal {
    /// What the echo area says of it.
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::Macro => "A macro does not answer a request",
            Self::NotCurrent | Self::Wait => "Wait a moment, then answer the request",
            Self::Cut => "The window is too small to answer the request in",
        }
    }
}

/// When a key is pressed, for an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::sessions::hosting::native) struct Pressed {
    /// The key, or the first of its sequence.
    pub(in crate::app::sessions::hosting::native) at: Instant,
    /// The one before that.
    pub(in crate::app::sessions::hosting::native) before: Option<Instant>,
}

impl Pressed {
    /// The key the person typed last, as `typed` says.
    pub(in crate::app::sessions::hosting::native) fn last(typed: Typed, now: Instant) -> Self {
        Self {
            at: typed.first.unwrap_or(now).min(now),
            before: typed.before_first,
        }
    }

    /// The next key, at `now`, after what `typed` says.
    pub(in crate::app::sessions::hosting::native) fn next(typed: Typed, now: Instant) -> Self {
        Self {
            at: now,
            before: typed.last,
        }
    }
}

/// Whether the quiet wait passed at `pressed` of what came in front at `since`: [`QUIET`] since then
/// and since the key before.
pub(in crate::app::sessions::hosting::native) fn waited(since: Instant, pressed: Pressed) -> bool {
    let quiet = |from: Instant| pressed.at >= from + QUIET;
    quiet(since) && pressed.before.is_none_or(quiet)
}

/// Since when a call may be answered: the latest of point coming to its line, of the call being shown,
/// and of oso's window coming in front.
fn since(point: Instant, shown: Instant, front: Instant) -> Instant {
    point.max(shown).max(front)
}

/// When a line dim now for the quiet wait alone is lit: [`QUIET`] after what came in front at
/// `since`, and after the last key.
fn lit_at(since: Instant, last: Option<Instant>) -> Instant {
    last.map_or(since, |last| last.max(since)) + QUIET
}

impl App {
    /// Why session `key`'s line, `line`, takes no answer from a key `pressed` so, if it takes none.
    pub(super) fn request_refusal(
        &self,
        key: SessionKey,
        line: BufferId,
        pressed: Pressed,
    ) -> Option<Refusal> {
        if self.editor.macro_replaying() {
            return Some(Refusal::Macro);
        }
        let permissions = &self.hosting.natives.permissions;
        let current = (permissions.current).filter(|now| now.key == key && now.place == line);
        let Some(current) = current else {
            return Some(Refusal::NotCurrent);
        };
        let Some(since) = self.line_since(key) else {
            return Some(Refusal::NotCurrent);
        };
        if !waited(since, pressed) {
            return Some(Refusal::Wait);
        }
        // Whole only since it was last looked at: the person has not seen it.
        (!current.whole || !self.line_shows_all(key)).then_some(Refusal::Cut)
    }

    /// Since when session `key`'s line, with point on it, waits to take a key: what its keys, text typed
    /// as point came and a press of the mouse are judged by. None while it takes none whatever the time:
    /// no frame showed point that went there by itself, or oso's window is not in front.
    pub(super) fn line_since(&self, key: SessionKey) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        let current = (permissions.current).filter(|now| now.key == key)?;
        let seen = !permissions.requests.point.unseen(key);
        self.request_since(key, current).filter(|_| seen)
    }

    /// Since when the call session `key` shows could be answered where point is, `current`: the latest
    /// of point coming there, of the call being shown, of oso's window coming in front, and, longer, of
    /// a prompt an agent opened closing. Worked out here, whatever was noted when. None with no call,
    /// or with oso's window not in front.
    fn request_since(&self, key: SessionKey, current: Current) -> Option<Instant> {
        let ask = self.hosting.natives.permissions.asks.shown(key)?;
        let shown = ask.shown.filter(|_| ask.request().is_some())?;
        let since = since(current.since, shown, self.window_seen_since()?);
        Some(self.waits_after_agents(since))
    }

    /// The key pressed last, if session `key`'s line takes an answer of it; else the echo area says
    /// why.
    pub(super) fn request_admits(&mut self, key: SessionKey, line: BufferId) -> Option<Pressed> {
        let pressed = Pressed::last(self.person_typed_at(), Instant::now());
        let Some(refusal) = self.request_refusal(key, line, pressed) else {
            return Some(pressed);
        };
        log::debug!("native sessions: a request took no answer: {refusal:?}");
        // Point came to it with this key: the wait starts now.
        if refusal == Refusal::NotCurrent {
            self.note_current_card();
        }
        self.editor.message(refusal.message());
        None
    }

    /// Whether session `key`'s line's window shows every row of it, at the width it is written at.
    pub(in crate::app::sessions::hosting::native::permissions) fn line_shows_all(
        &self,
        key: SessionKey,
    ) -> bool {
        let Some(native) = self.hosting.natives.map.get(&key) else {
            return false;
        };
        let drawn = &self.hosting.natives.permissions.requests.drawn;
        let drawn = drawn.iter().find(|drawn| drawn.key == key);
        let Some((_, width, line)) = drawn.and_then(|drawn| drawn.line.as_ref()) else {
            return false;
        };
        let Some(id) = self.editor.dock_window(native.buffer, DockSlot::Card) else {
            return false;
        };
        let (Some(window), Some(regions)) = (self.editor.window_by_id(id), self.editor.regions(id))
        else {
            return false;
        };
        let wraps = (self.editor.buffer(window.buffer())).is_some_and(|buffer| buffer.wraps());
        let scroll = window.scroll();
        let columns = regions.text.columns;
        Some(window.buffer()) == native.line
            && !wraps
            && (columns >= NARROWEST && columns >= *width && regions.text.rows >= line.rows)
            && (scroll.start, scroll.left) == (0, 0)
    }

    /// The width session `key`'s line is written at, if a window carries it.
    pub(super) fn line_width(&self, key: SessionKey) -> Option<usize> {
        let native = self.hosting.natives.map.get(&key)?;
        let (_, regions) = carrier(&self.editor, native.buffer, DockSlot::Card)?;
        Some(regions.gutter.columns + regions.text.columns)
    }

    /// What session `key`'s line, `line`, says now.
    pub(super) fn line_says(&self, key: SessionKey, line: BufferId) -> Says {
        let pressed = Pressed::next(self.person_typed_at(), Instant::now());
        Says {
            here: self.editor.tiled().0.buffer() == line,
            answers: self.request_refusal(key, line, pressed).is_none(),
        }
    }

    /// When the line in front is lit, its wait alone keeping it dim.
    pub(in crate::app::sessions::hosting::native) fn line_lit_deadline(&self) -> Option<Instant> {
        let permissions = &self.hosting.natives.permissions;
        let current = permissions.current?;
        let drawn = permissions
            .requests
            .drawn
            .iter()
            .find(|drawn| drawn.key == current.key)?;
        let (says, ..) = drawn.line.as_ref()?;
        let since = self.request_since(current.key, current)?;
        let last = self.agent_person_typed();
        // Past it, what keeps it dim is no wait.
        (says.here && !says.answers)
            .then(|| lit_at(since, last))
            .filter(|at| *at > Instant::now())
    }

    /// Whether the selected line, if one is, takes an answer now: what its keys check, with the next
    /// key the answer, and what was read of its call. For end-to-end flows.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn request_state(&self) -> Option<serde_json::Value> {
        let line = self.editor.tiled().0.buffer();
        let key = (self.line_session(line)).or_else(|| {
            let drawn = &self.hosting.natives.permissions.requests.drawn;
            drawn.first().map(|drawn| drawn.key)
        })?;
        let request = self
            .hosting
            .natives
            .permissions
            .asks
            .shown(key)?
            .request()?;
        let pressed = Pressed::next(self.person_typed_at(), Instant::now());
        let refusal = self.request_refusal(key, line, pressed);
        let (read, of) = request.reading.lines();
        Some(serde_json::json!({
            // Whether oso's window is in front and not covered, as the window system said.
            "seen": self.window_seen_since().is_some(),
            "takes_answer": refusal.is_none(),
            "refusal": refusal.map(|refusal| format!("{refusal:?}")),
            "allows": refusal.is_none() && request.reading.allows(),
            "may_ask": refusal.is_none() && request.reading.may_ask(),
            "read": read,
            "of": of,
            "last_in_view": request.reading.last_in_view(),
            "line": self.hosting.natives.permissions.requests.drawn.iter()
                .find(|drawn| drawn.key == key)
                .and_then(|drawn| drawn.line.as_ref())
                .map(|(_, _, line)| line.shown.text.clone()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{Pressed, QUIET, Typed, lit_at, since, waited};

    #[test]
    fn an_answer_waits_since_what_came_in_front_and_since_the_key_before() {
        let since = Instant::now();
        let at = |ms: u64| since + Duration::from_millis(ms);
        let pressed = |ms, before: Option<u64>| Pressed {
            at: at(ms),
            before: before.map(at),
        };
        assert!(waited(
            since,
            Pressed {
                at: since + QUIET,
                before: None,
            }
        ));
        assert!(!waited(since, pressed(499, None)));
        assert!(!waited(since, pressed(900, Some(500))));
        assert!(waited(since, pressed(1000, Some(500))));
        // What came after the key was pressed was not what it answers.
        assert!(!waited(at(700), pressed(600, None)));
        assert_eq!(lit_at(since, None), since + QUIET);
        assert_eq!(lit_at(since, Some(at(300))), at(300) + QUIET);
        assert_eq!(lit_at(at(300), Some(since)), at(300) + QUIET);
    }

    /// A key pressed before the call it would answer was shown, as another was, or before oso's window
    /// came in front, answers nothing, whatever was noted of point.
    #[test]
    fn a_call_is_answered_by_no_key_pressed_before_it_could_be() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let pressed = Pressed {
            at: at(1000),
            before: None,
        };
        assert!(waited(since(at(0), at(0), at(0)), pressed));
        for late in [(1001, 0, 0), (0, 1001, 0), (0, 0, 1001), (0, 501, 0)] {
            let since = since(at(late.0), at(late.1), at(late.2));
            assert!(!waited(since, pressed), "{late:?}");
        }
    }

    #[test]
    fn a_chord_waits_to_its_first_key() {
        let since = Instant::now();
        let at = |ms: u64| since + Duration::from_millis(ms);
        // `C-c` in the wait, `C-k` after it.
        let typed = Typed {
            last: Some(at(900)),
            before: Some(at(400)),
            first: Some(at(400)),
            before_first: None,
        };
        assert!(!waited(since, Pressed::last(typed, at(901))));
        // Both after it, right after one another.
        let typed = Typed {
            last: Some(at(660)),
            before: Some(at(600)),
            first: Some(at(600)),
            ..typed
        };
        assert!(waited(since, Pressed::last(typed, at(661))));
        // The next key is one of its own.
        assert!(!waited(since, Pressed::next(typed, at(1100))));
        assert!(waited(since, Pressed::next(typed, at(1160))));
    }
}
