//! Whether a composition was made in isearch, so that its text goes nowhere once that search is
//! gone (ADR 0158).
//!
//! Must not depend on: the app's state or a window system; the app says what was reported, and
//! whether a composition is open and isearch on.
//!
//! - The window system reports one's commit as an empty preedit, then the commit, as two events,
//!   and the search may end between them. So the mark outlives the composition, up to that commit.
//! - A preedit with text while none is open begins another composition, which takes its own mark:
//!   one cancelled in a search never takes the next with it.

use oso_platform::window::ImeEvent;

/// Whether the composition open, or the one whose commit may come next, was made in isearch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct MadeIn {
    search: bool,
}

impl MadeIn {
    /// The input method reports `event`, with a composition open or not (`composing`) and
    /// isearch on or not (`searching`). Whether it is of one whose search is gone, so that what
    /// it holds goes nowhere.
    pub(super) fn lost(&mut self, event: &ImeEvent, composing: bool, searching: bool) -> bool {
        let preedit = matches!(event, ImeEvent::Preedit(text, _) if !text.is_empty());
        if preedit && !composing {
            self.search = searching;
        }
        let lost = self.search && !searching;
        if matches!(event, ImeEvent::Commit(_)) || (preedit && lost) {
            self.search = false;
        }
        lost
    }

    /// The composition is over with no commit to come.
    pub(super) fn over(&mut self) {
        self.search = false;
    }
}

#[cfg(test)]
mod tests {
    use oso_platform::window::ImeEvent;

    use super::MadeIn;

    fn preedit(text: &str) -> ImeEvent {
        ImeEvent::Preedit(text.to_owned(), None)
    }

    fn commit() -> ImeEvent {
        ImeEvent::Commit("日本".to_owned())
    }

    #[test]
    fn a_commit_goes_to_the_search_it_was_made_in_and_nowhere_once_that_is_gone() {
        let mut made_in = MadeIn::default();
        assert!(!made_in.lost(&preedit("に"), false, true));
        assert!(!made_in.lost(&preedit("にほ"), true, true));
        let in_search = made_in;
        assert!(!made_in.lost(&preedit(""), true, true));
        assert!(!made_in.lost(&commit(), false, true));
        assert!(
            !made_in.lost(&commit(), false, false),
            "another, in a buffer"
        );
        // The search ends between the two reports of a commit.
        let mut made_in = in_search;
        assert!(!made_in.lost(&preedit(""), true, true));
        assert!(made_in.lost(&commit(), false, false));
        assert!(!made_in.lost(&commit(), false, false), "once");
        // Or before both, or as the composition goes on.
        let mut made_in = in_search;
        assert!(made_in.lost(&preedit(""), true, false));
        assert!(made_in.lost(&commit(), false, false));
        let mut made_in = in_search;
        assert!(made_in.lost(&preedit("にほん"), true, false));
    }

    #[test]
    fn one_cancelled_in_a_search_takes_no_later_one_with_it() {
        let mut made_in = MadeIn::default();
        assert!(!made_in.lost(&preedit("に"), false, true));
        assert!(!made_in.lost(&preedit(""), true, true));
        // The search ends, and the next is composed in the buffer.
        assert!(!made_in.lost(&preedit("ほ"), false, false));
        assert!(!made_in.lost(&preedit(""), true, false));
        assert!(!made_in.lost(&commit(), false, false));
    }

    #[test]
    fn one_that_is_over_leaves_no_mark() {
        let mut made_in = MadeIn::default();
        assert!(!made_in.lost(&preedit("に"), false, true));
        made_in.over();
        assert!(!made_in.lost(&commit(), false, false));
    }
}
