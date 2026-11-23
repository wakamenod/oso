//! Asking before a session starts where another live one runs (ADR 0121): one question at a
//! time, the starts that would ask meanwhile waiting their turn in order, as held starts
//! released together can (ADR 0173), and the start each checking session under way began
//! with, by its key, so that its start thread's late finding asks about that one.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::{HashMap, VecDeque};

use oso_core::minibuffer::PromptId;

use super::{Begin, Then};
use crate::app::App;
use crate::sessions::worker::SessionKey;

/// The shared-directory questions.
#[derive(Debug, Default)]
pub(super) struct Shared {
    /// The question open, and the start its yes begins.
    asked: Option<(PromptId, Begin)>,
    /// Starts to ask about once it closes, in the order they came.
    waiting: VecDeque<Begin>,
    /// The waiting start whose turn it is, judged again now: it asks rather than waits.
    turn: bool,
    /// The checking sessions under way, by key: what each began with.
    checking: HashMap<SessionKey, Begin>,
}

impl Shared {
    /// Gives `begin` back to be asked about now, or keeps it waiting behind the question open
    /// or the starts that came before it.
    fn ask_or_wait(&mut self, begin: Begin) -> Option<Begin> {
        if self.asked.is_some() || (!self.waiting.is_empty() && !self.turn) {
            self.waiting.push_back(begin);
            return None;
        }
        Some(begin)
    }

    /// Notes that `prompt` asks about `begin`.
    fn asked(&mut self, prompt: PromptId, begin: Begin) {
        self.asked = Some((prompt, begin));
    }

    /// The start the open question's yes begins.
    fn take_yes(&mut self) -> Option<Begin> {
        self.asked.take().map(|(_, begin)| begin)
    }

    /// Forgets the question asked if `open` says its prompt closed, with no yes as its start
    /// is still there, and with it the starts that wait for the same directory: the person
    /// said no to starting there.
    fn closed(&mut self, open: impl Fn(PromptId) -> bool) {
        let Some((prompt, begin)) = &self.asked else {
            return;
        };
        if open(*prompt) {
            return;
        }
        let there = |other: &Begin| match (&begin.directory, &other.directory) {
            (Some(asked), Some(other)) => oso_core::project::same_place(asked, other),
            _ => false,
        };
        self.waiting.retain(|other| !there(other));
        self.asked = None;
    }

    /// Has the start asked about, which waited, show on a yes only as it would have: it left
    /// the queue behind, in case it started unasked.
    fn shown_on_yes(&mut self, behind: bool) {
        if let Some((_, begin)) = &mut self.asked {
            begin.behind = behind;
        }
    }

    /// The next waiting start, once no question is open.
    fn next(&mut self) -> Option<Begin> {
        if self.asked.is_some() {
            return None;
        }
        self.waiting.pop_front()
    }

    /// Forgets the question asked and every start that waits.
    fn drop_all(&mut self) {
        self.asked = None;
        self.waiting.clear();
    }

    /// Keeps what the checking session `key` began with, until its start thread reports.
    pub(super) fn under_way(&mut self, key: SessionKey, begin: Begin) {
        self.checking.insert(key, begin);
    }

    /// Forgets the session `key`'s start, and gives what it began with.
    pub(super) fn take_under_way(&mut self, key: SessionKey) -> Option<Begin> {
        self.checking.remove(&key)
    }
}

impl App {
    /// Asks before `begin` starts a session where another live one runs, or, while another
    /// such question is open or other starts wait, has it wait its turn: a question asked
    /// over another would leave the one beneath with no start to begin.
    pub(super) fn ask_shared(&mut self, begin: Begin) {
        let Some(begin) = self.hosting.shared.ask_or_wait(begin) else {
            return;
        };
        let worktree = self.dispatcher.where_is(&self.editor, "session-worktree");
        let question = format!(
            "Another session runs in this directory. A worktree keeps their changes apart \
             ({worktree}). Start here anyway?"
        );
        match self.editor.ask(&question, "session-start-shared") {
            Ok(prompt) => self.hosting.shared.asked(prompt, begin),
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// The person's `y` to starting where another session runs. Nothing starts in an
    /// instance that is going (ADR 0133).
    pub(in crate::app) fn start_shared(&mut self) {
        match self.hosting.shared.take_yes() {
            Some(_) if self.leaving.is_under_way() => {}
            Some(begin) => {
                let begin = Begin {
                    check_shared: false,
                    ..begin
                };
                self.begin_session(begin, Then::Nothing);
            }
            None => self.editor.message("No session waits to start"),
        }
    }

    /// After the session requests of an event and the other questions' sweeps: a question
    /// that closed with no yes drops its start and those that wait for its directory, and the
    /// other starts that waited go in turn, each
    /// judged again, until one asks: one asks only if another session still runs where it
    /// would. Once an exit is under way, they are dropped (ADR 0133).
    pub(in crate::app) fn ask_next_shared(&mut self) {
        if self.leaving.is_under_way() {
            return self.hosting.shared.drop_all();
        }
        let prompts = self.editor.prompts();
        self.hosting
            .shared
            .closed(|asked| prompts.iter().any(|prompt| prompt.id() == asked));
        // One that starts unasked, as the session that made it ask ended meanwhile, starts
        // behind: the person's keys went to the question, not to it.
        while let Some(begin) = self.hosting.shared.next() {
            let behind = begin.behind;
            self.hosting.shared.turn = true;
            self.begin_session(
                Begin {
                    behind: true,
                    ..begin
                },
                Then::Nothing,
            );
            self.hosting.shared.turn = false;
            self.hosting.shared.shown_on_yes(behind);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn begin(directory: &str) -> Begin {
        Begin {
            directory: Some(PathBuf::from(directory)),
            check_shared: true,
            ..Begin::default()
        }
    }

    /// A prompt's id, from a question asked of an editor of its own.
    fn prompt() -> PromptId {
        oso_core::editor::Editor::default()
            .ask("Start here anyway?", "session-start-shared")
            .unwrap()
    }

    fn directory(begin: Option<Begin>) -> Option<PathBuf> {
        begin.and_then(|begin| begin.directory)
    }

    #[test]
    fn a_second_ask_waits_and_the_starts_come_out_in_order() {
        let mut shared = Shared::default();
        let first = shared.ask_or_wait(begin("/a")).unwrap();
        shared.asked(prompt(), first);
        assert!(shared.ask_or_wait(begin("/b")).is_none());
        assert!(shared.ask_or_wait(begin("/c")).is_none());
        assert!(shared.next().is_none(), "nothing comes out while asked");
        assert_eq!(directory(shared.take_yes()), Some("/a".into()));
        assert_eq!(directory(shared.next()), Some("/b".into()));
        assert_eq!(directory(shared.next()), Some("/c".into()));
        assert!(shared.next().is_none());
    }

    #[test]
    fn a_start_behind_waiting_ones_waits_unless_it_is_its_turn() {
        let mut shared = Shared::default();
        shared.waiting.push_back(begin("/a"));
        assert!(
            shared.ask_or_wait(begin("/b")).is_none(),
            "no question open"
        );
        shared.turn = true;
        assert!(shared.ask_or_wait(begin("/c")).is_some());
    }

    #[test]
    fn a_question_closed_with_no_yes_drops_the_starts_for_its_directory() {
        let mut shared = Shared::default();
        shared.asked(prompt(), begin("/a"));
        for waiting in ["/a", "/b", "/a/", "/c"] {
            shared.waiting.push_back(begin(waiting));
        }
        shared.closed(|_| true);
        assert!(shared.asked.is_some(), "still open");
        shared.closed(|_| false);
        assert!(shared.take_yes().is_none());
        assert_eq!(directory(shared.next()), Some("/b".into()));
        assert_eq!(directory(shared.next()), Some("/c".into()));
        assert!(shared.next().is_none());
    }

    #[test]
    fn a_yes_keeps_the_waiting_starts_and_shows_its_own_as_it_would() {
        let mut shared = Shared::default();
        shared.asked(prompt(), begin("/a"));
        shared.waiting.push_back(begin("/a"));
        shared.shown_on_yes(false);
        let yes = shared.take_yes().unwrap();
        assert!(!yes.behind);
        shared.closed(|_| false);
        assert_eq!(directory(shared.next()), Some("/a".into()));
    }

    #[test]
    fn a_start_under_way_keeps_its_own_begin() {
        let mut shared = Shared::default();
        let (a, b): (SessionKey, SessionKey) = (1, 2);
        shared.under_way(a, begin("/a"));
        shared.under_way(b, begin("/b"));
        assert_eq!(directory(shared.take_under_way(a)), Some("/a".into()));
        assert!(shared.take_under_way(a).is_none());
        assert_eq!(directory(shared.take_under_way(b)), Some("/b".into()));
    }

    #[test]
    fn dropping_all_ends_the_wait() {
        let mut shared = Shared::default();
        shared.asked(prompt(), begin("/a"));
        shared.waiting.push_back(begin("/b"));
        shared.drop_all();
        assert!(shared.next().is_none());
        assert!(shared.take_yes().is_none());
    }
}
