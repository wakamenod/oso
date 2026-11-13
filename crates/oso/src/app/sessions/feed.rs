//! The screen feed (ADR 0121 step 3, Q6): when a hosted session's screen, as the UI keeps it
//! for its terminal (ADR 0120), goes to the sessions worker. At most once every
//! [`FEED_EVERY`] a session, only when a row, the title or the progress changed, and never
//! while the person has scrolled its terminal back: rules read the live screen only.
//!
//! Must not depend on: the worker's thread, the terminal's thread, or how a frame is drawn.
//! The instants come from the caller.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::sessions::screen::Progress;
use crate::sessions::worker::SessionKey;

/// The least time between two looks at one session's screen that go to the worker.
pub(in crate::app) const FEED_EVERY: Duration = Duration::from_millis(100);

/// What the feed sent of each session.
#[derive(Debug, Default)]
pub(in crate::app) struct Feed {
    fed: HashMap<SessionKey, Fed>,
}

/// What went to the worker last of one session.
#[derive(Debug, Default)]
struct Fed {
    /// The kept grid's generation then: a row that changed since raises it.
    generation: u64,
    title: String,
    progress: Option<Progress>,
    /// When it went.
    at: Option<Instant>,
    /// A change waits for [`FEED_EVERY`] to pass.
    waiting: bool,
}

/// A session's screen as the UI keeps it now.
#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct Now<'a> {
    pub(in crate::app) generation: u64,
    pub(in crate::app) title: &'a str,
    pub(in crate::app) progress: Option<Progress>,
    /// Lines the person has scrolled its terminal back.
    pub(in crate::app) scrolled: usize,
}

impl Feed {
    /// Whether session `key`'s screen, as `screen` says it is at `now`, goes to the worker
    /// now. If so, it counts as sent; a change that must wait makes [`Feed::due`] say when.
    pub(in crate::app) fn look(&mut self, key: SessionKey, screen: Now<'_>, now: Instant) -> bool {
        let fed = self.fed.entry(key).or_default();
        let changed = fed.at.is_none()
            || fed.generation != screen.generation
            || fed.title != screen.title
            || fed.progress != screen.progress;
        if !changed || screen.scrolled > 0 {
            // Back at the bottom, the next snapshot raises the generation and looks again.
            fed.waiting = false;
            return false;
        }
        if fed.at.is_some_and(|at| now < at + FEED_EVERY) {
            fed.waiting = true;
            return false;
        }
        fed.generation = screen.generation;
        screen.title.clone_into(&mut fed.title);
        fed.progress = screen.progress;
        fed.at = Some(now);
        fed.waiting = false;
        true
    }

    /// Forgets the sessions that `kept` does not keep: a session or terminal that went must
    /// not leave a change waiting, which would make [`Feed::due`] stay in the past.
    pub(in crate::app) fn keep(&mut self, kept: impl Fn(SessionKey) -> bool) {
        self.fed.retain(|key, _| kept(*key));
    }

    /// When a change that waits may go: the soonest. `None` while none waits.
    pub(in crate::app) fn due(&self) -> Option<Instant> {
        self.fed
            .values()
            .filter(|fed| fed.waiting)
            .filter_map(|fed| fed.at.map(|at| at + FEED_EVERY))
            .min()
    }
}

/// The OSC 9;4 progress a terminal reported, as a session's evidence holds it.
pub(in crate::app) fn progress(reported: oso_term::Progress) -> Option<Progress> {
    use oso_term::Progress as P;
    match reported {
        P::Remove => Progress::new(0, None),
        P::Set(value) => Progress::new(1, Some(value)),
        P::Error(value) => Progress::new(2, value),
        P::Indeterminate => Progress::new(3, None),
        P::Paused(value) => Progress::new(4, value),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{FEED_EVERY, Feed, Now, progress};

    fn screen(generation: u64, title: &str) -> Now<'_> {
        Now {
            generation,
            title,
            progress: None,
            scrolled: 0,
        }
    }

    #[test]
    fn the_first_screen_goes_at_once_and_an_unchanged_one_never() {
        let (mut feed, t0) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(0, "claude"), t0));
        assert!(!feed.look(1, screen(0, "claude"), t0 + Duration::from_secs(1)));
        assert_eq!(feed.due(), None);
    }

    #[test]
    fn changes_within_the_pace_wait_and_go_once_it_has_passed() {
        let (mut feed, t0) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(1, "a"), t0));
        let soon = t0 + Duration::from_millis(30);
        assert!(!feed.look(1, screen(2, "a"), soon));
        assert!(!feed.look(1, screen(3, "a"), soon));
        assert_eq!(feed.due(), Some(t0 + FEED_EVERY));
        assert!(feed.look(1, screen(3, "a"), t0 + FEED_EVERY));
        assert_eq!(feed.due(), None);
    }

    #[test]
    fn a_title_or_progress_alone_is_a_change() {
        let (mut feed, t0) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(1, "a"), t0));
        let later = t0 + FEED_EVERY;
        assert!(feed.look(1, screen(1, "b"), later));
        let with = Now {
            progress: progress(oso_term::Progress::Set(40)),
            ..screen(1, "b")
        };
        assert!(feed.look(1, with, later + FEED_EVERY));
    }

    #[test]
    fn nothing_goes_while_the_person_has_scrolled_back() {
        let (mut feed, t0) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(1, "a"), t0));
        let back = Now {
            scrolled: 3,
            ..screen(2, "a")
        };
        assert!(!feed.look(1, back, t0 + FEED_EVERY));
        assert_eq!(feed.due(), None, "no wake while scrolled back");
        assert!(feed.look(1, screen(2, "a"), t0 + FEED_EVERY * 2));
    }

    #[test]
    fn sessions_are_paced_apart() {
        let (mut feed, t0) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(1, "a"), t0));
        assert!(feed.look(2, screen(1, "a"), t0));
        assert!(!feed.look(1, screen(2, "a"), t0));
    }

    #[test]
    fn a_session_that_went_leaves_nothing_due() {
        let (mut feed, now) = (Feed::default(), Instant::now());
        assert!(feed.look(1, screen(1, ""), now));
        assert!(!feed.look(1, screen(2, ""), now));
        assert!(feed.due().is_some());
        feed.keep(|key| key != 1);
        assert_eq!(feed.due(), None);
    }

    #[test]
    fn progress_states_keep_their_osc_numbers() {
        use oso_term::Progress as P;
        let shown = |p| progress(p).map(|p| p.to_string());
        assert_eq!(shown(P::Remove).as_deref(), Some("0"));
        assert_eq!(shown(P::Set(40)).as_deref(), Some("1;40"));
        assert_eq!(shown(P::Error(None)).as_deref(), Some("2"));
        assert_eq!(shown(P::Indeterminate).as_deref(), Some("3"));
        assert_eq!(shown(P::Paused(Some(7))).as_deref(), Some("4;7"));
    }
}
