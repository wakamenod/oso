//! The kind of keyboard input source the person types with, as oso last knew it (ADR 0182, "Point"
//! 2): what the window system told, but for oso's own switch to ASCII for a prompt (ADR 0106, ADR
//! 0158), which it tells of as of any other.
//!
//! Must not depend on: the app's state or a window system; the app says what it was told, and when it wants
//! ASCII.
//!
//! - While oso wants ASCII or secure input, and for [`QUIET`] after, an `Other` does not replace a Japanese known
//!   before: it may be oso's own switch. Where the person chose it, oso knows Japanese until it is
//!   told of another change, which only costs them a key.
//! - It is told only while oso is in front; what is known behind may be old.

use std::time::Instant;

use oso_platform::window::InputSource;

use crate::app::agent::QUIET;

/// The kind of input source, as oso knows it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Source {
    japanese: bool,
    /// When that last changed.
    changed: Option<Instant>,
    /// oso wants ASCII, or secure input.
    ascii: bool,
    secure: bool,
    /// When it last wanted one.
    until: Option<Instant>,
}

impl Source {
    /// The window system told of `source` at `now`.
    pub(super) fn told(&mut self, source: InputSource, now: Instant) {
        let japanese = source == InputSource::Japanese;
        let after = (self.until).is_some_and(|until| now < until + QUIET);
        let osos = self.ascii || self.secure || after;
        if japanese == self.japanese || (!japanese && osos) {
            return;
        }
        self.japanese = japanese;
        self.changed = Some(now);
    }

    /// oso asks the window for an ASCII input source, or, not `wanted`, for it no more, at `now`.
    pub(super) fn ascii(&mut self, wanted: bool, now: Instant) {
        self.wants(now, |source| source.ascii = wanted);
    }

    /// oso asks the window for secure input, or, not `wanted`, for it no more, at `now`: the system may
    /// choose an ASCII input source for it.
    pub(super) fn secure(&mut self, wanted: bool, now: Instant) {
        self.wants(now, |source| source.secure = wanted);
    }

    fn wants(&mut self, now: Instant, set: impl FnOnce(&mut Self)) {
        let before = self.ascii || self.secure;
        set(self);
        if before && !(self.ascii || self.secure) {
            self.until = Some(now);
        }
    }

    /// Whether it is a Japanese input mode, and since when that is settled, if it ever was not: since it
    /// last changed, and since oso last wanted ASCII or secure input.
    pub(super) fn japanese(self) -> (bool, Option<Instant>) {
        (self.japanese, self.changed.max(self.until))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use oso_platform::window::InputSource::{Japanese, Other};

    use super::Source;

    #[test]
    fn a_source_not_told_of_is_not_japanese() {
        assert_eq!(Source::default().japanese(), (false, None));
    }

    #[test]
    fn the_source_is_what_was_told_last() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut source = Source::default();
        source.told(Other, at(0));
        assert_eq!(source.japanese(), (false, None), "as it was taken to be");
        source.told(Japanese, at(10));
        source.told(Japanese, at(20));
        assert_eq!(source.japanese(), (true, Some(at(10))));
        source.told(Other, at(30));
        assert_eq!(source.japanese(), (false, Some(at(30))));
    }

    /// A prompt opens, oso chooses ASCII, the prompt closes and the person's Japanese is given back,
    /// each told of when the system likes.
    #[test]
    fn osos_own_switch_to_ascii_is_not_the_persons() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let japanese = || {
            let mut source = Source::default();
            source.told(Japanese, at(0));
            source
        };
        let mut source = japanese();
        source.ascii(true, at(10));
        source.told(Other, at(20));
        assert!(source.japanese().0, "while the prompt is open");
        source.ascii(false, at(1000));
        source.told(Other, at(1499));
        assert!(source.japanese().0, "told late, the prompt closed");
        source.told(Other, at(1500));
        assert!(!source.japanese().0, "the person's own");

        // Chosen by the person while oso wants ASCII.
        let mut source = Source::default();
        source.ascii(true, at(10));
        source.told(Japanese, at(20));
        assert_eq!(source.japanese(), (true, Some(at(20))));

        // As secure input is, whichever of the two ends last.
        let mut source = japanese();
        source.ascii(true, at(10));
        source.secure(true, at(20));
        source.ascii(false, at(30));
        source.told(Other, at(2000));
        assert!(source.japanese().0);
        source.secure(false, at(3000));
        source.told(Other, at(3499));
        assert_eq!(source.japanese(), (true, Some(at(3000))), "settled since");

        // A prompt that never opened holds nothing back.
        let mut source = japanese();
        source.ascii(false, at(10));
        source.told(Other, at(20));
        assert!(!source.japanese().0);
    }
}
