//! The questions the agent channel caused, on the UI thread (ADR 0085): which connection each
//! is, and the questions the person's `y` to one of them opens; keys that must not answer one
//! until a quiet moment; and closing those whose connection went.
//!
//! Must not depend on: sockets or threads, or winit; this only watches the prompts and keys.

use std::time::{Duration, Instant};

use oso_core::key::KeyPress;
use oso_core::minibuffer::PromptId;

use super::super::App;
use super::QUIET;
use crate::channel::Connection;

/// Whether `key` must not reach a question the agent caused, `since` the last key or its
/// opening: it takes none until [`QUIET`] has passed. `C-g` always goes through.
fn holds(since: Option<Duration>, key: KeyPress) -> bool {
    key != oso_core::keymap::CANCEL && !quiet(since)
}

/// Whether [`QUIET`] has passed `since` the last key or the question's opening, if either.
fn quiet(since: Option<Duration>) -> bool {
    since.is_none_or(|since| since >= QUIET)
}

/// Logs in a dev build that `by` restarted the wait of a question the agent caused, `since`
/// it last started, so that an e2e flow's refused answer can be traced to it. Only the kind.
#[cfg(feature = "dev")]
fn log_restart(by: &str, since: Option<Duration>) {
    let since = since.map_or_else(
        || "none".to_owned(),
        |since| format!("{} ms", since.as_millis()),
    );
    log::debug!("agent question: the quiet wait restarted by {by}, {since} after the last start");
}

/// When the person typed in the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::app) struct Typed {
    /// The last key or text, or a press of the mouse's button.
    pub(in crate::app) last: Option<Instant>,
    /// The one before the last key.
    pub(in crate::app) before: Option<Instant>,
    /// The first key of the last sequence of keys, as a chord's or one after an argument's, and the
    /// one before it: what an answer's wait is measured to (ADR 0182).
    pub(in crate::app) first: Option<Instant>,
    pub(in crate::app) before_first: Option<Instant>,
}

impl Typed {
    /// A key came at `now`: one that goes on with a sequence under way if `goes_on`.
    fn key(&mut self, now: Instant, goes_on: bool) {
        self.before = self.last.replace(now);
        if !goes_on || self.first.is_none() {
            (self.first, self.before_first) = (Some(now), self.before);
        }
    }
}

impl App {
    /// Notes the prompts a channel command opened, those newer than `newest`, which are the
    /// agent's, and returns the innermost one's label if it opened any.
    pub(super) fn note_caused(
        &mut self,
        newest: Option<PromptId>,
        connection: Connection,
    ) -> Option<String> {
        let opened: Vec<PromptId> = self
            .editor
            .prompts()
            .iter()
            .filter(|prompt| prompt.from_agent() && newest.is_none_or(|n| prompt.id() > n))
            .map(|prompt| prompt.id())
            .collect();
        for &id in &opened {
            self.agent.caused.push((id, connection));
            self.agent.quiet_since = Some(Instant::now());
        }
        opened.last()?;
        self.editor.prompt().map(|prompt| prompt.label().to_owned())
    }

    /// Whether `key` from the window must not reach a question the channel caused yet:
    /// it takes no answer until [`QUIET`] has passed with no key, and every key starts that
    /// wait again. `C-g` always goes through.
    pub(in crate::app) fn note_key_for_agent(&mut self, key: KeyPress) -> bool {
        // A native session's request is answered only after a quiet moment (ADR 0165).
        let goes_on = self.dispatcher.keys_in_progress();
        self.agent.typed.key(Instant::now(), goes_on);
        if !self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent())
        {
            return false;
        }
        let since = self.agent.quiet_since.map(|at| at.elapsed());
        #[cfg(feature = "dev")]
        log_restart("a key", since);
        self.agent.quiet_since = Some(Instant::now());
        if !holds(since, key) {
            return false;
        }
        self.editor.prompt_says("Agent: wait a moment, then answer");
        self.request_redraw();
        true
    }

    /// Whether the innermost question the agent caused, if one is open, takes an answer now:
    /// its quiet moment has passed since the last key or start. For end-to-end flows, which
    /// wait for it before they answer instead of sleeping a while.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn agent_question_takes_answer(&self) -> Option<bool> {
        self.editor.prompt().filter(|prompt| prompt.from_agent())?;
        Some(quiet(self.agent.quiet_since.map(|at| at.elapsed())))
    }

    /// Why keys from the dev socket, which stand in for the person's, must not reach a
    /// question the agent caused, if they must not: they come one at a time, so that each one
    /// restarts its wait, and each one waits as a key from the window does. Each is the
    /// person's key for a native session's card too.
    #[cfg(feature = "dev")]
    pub(in crate::app) fn agent_holds_keys(&mut self, keys: &[KeyPress]) -> Option<&'static str> {
        let agents = self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent());
        if agents && keys.len() > 1 {
            return Some("send keys one at a time to a question the agent caused");
        }
        let held = keys
            .first()
            .is_some_and(|&key| self.note_key_for_agent(key));
        held.then_some("a question the agent caused takes no answer yet")
    }

    /// When the person last typed in the window, if they did.
    pub(in crate::app) fn agent_person_typed(&self) -> Option<Instant> {
        self.agent.typed.last
    }

    /// When the person typed the key before the last one, if they did.
    pub(in crate::app) fn agent_key_before(&self) -> Option<Instant> {
        self.agent.typed.before
    }

    /// When the person typed in the window.
    pub(in crate::app) fn person_typed_at(&self) -> Typed {
        self.agent.typed
    }

    /// Text or a composition came from the window: the person is typing, and a question
    /// the channel caused waits longer. `by` names what came, never its text: `"text"`, or
    /// the input method's report by its kind, which a dev build logs.
    pub(in crate::app) fn person_typed(&mut self, by: &'static str) {
        self.agent.typed.last = Some(Instant::now());
        if self
            .editor
            .prompt()
            .is_some_and(|prompt| prompt.from_agent())
        {
            #[cfg(feature = "dev")]
            log_restart(by, self.agent.quiet_since.map(|at| at.elapsed()));
            #[cfg(not(feature = "dev"))]
            let _ = by;
            self.agent.quiet_since = Some(Instant::now());
        }
    }

    /// Forgets the questions the agent caused that have closed, and takes on those that the
    /// person's `y` to one of them opened: they are the same connection's, so that its
    /// `cancel` closes them and they close when it goes (ADR 0085).
    pub(super) fn track_follow_up_questions(&mut self) {
        let open: Vec<(PromptId, bool)> = self
            .editor
            .prompts()
            .iter()
            .map(|prompt| (prompt.id(), prompt.from_agent()))
            .collect();
        let is_open = |id: PromptId| open.iter().any(|&(open, _)| open == id);
        // The connection whose question closed last is the one a follow-up comes from.
        if let Some(&(_, by)) = self
            .agent
            .caused
            .iter()
            .rev()
            .find(|&&(id, _)| !is_open(id))
        {
            self.agent.last_answered = Some(by);
        }
        self.agent.caused.retain(|&(id, _)| is_open(id));
        let asked = self
            .agent
            .asking
            .as_ref()
            .and_then(|asking| asking.question);
        // A session tool's question is its own: no connection's follow-up (ADR 0121, step 6).
        let tools = self.session_tool_question();
        for &(id, from_agent) in &open {
            let known = self.agent.caused.iter().any(|&(caused, _)| caused == id)
                || self.agent.orphaned.contains(&id)
                || asked == Some(id)
                || tools == Some(id);
            if !from_agent || known {
                continue;
            }
            match self
                .agent
                .last_answered
                .filter(|by| self.agent.connected.contains(by))
            {
                Some(by) => self.agent.caused.push((id, by)),
                None => self.agent.orphaned.push(id),
            }
        }
        self.close_orphaned_questions();
    }

    /// The questions `connection` caused, or every connection if `None`, take no answer any
    /// more: each closes once it is innermost.
    pub(super) fn orphan_questions(&mut self, connection: Option<Connection>) {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agent.caused)
            .into_iter()
            .partition(|(_, by)| connection.is_none_or(|c| c == *by));
        self.agent.caused = kept;
        self.agent
            .orphaned
            .extend(gone.into_iter().map(|(id, _)| id));
        self.close_orphaned_questions();
    }

    /// The question `id` takes no answer any more: it closes once it is innermost.
    pub(in crate::app) fn close_question(&mut self, id: PromptId) {
        self.agent.orphaned.push(id);
        self.close_orphaned_questions();
    }

    /// Closes the orphaned questions that are innermost, as `C-g` would.
    pub(super) fn close_orphaned_questions(&mut self) {
        while let Some(id) = self.editor.prompt().map(|prompt| prompt.id())
            && self.agent.orphaned.contains(&id)
        {
            self.editor.quit();
            self.request_redraw();
        }
        let open = self.editor.prompts();
        self.agent
            .orphaned
            .retain(|id| open.iter().any(|prompt| prompt.id() == *id));
    }
}

#[cfg(test)]
mod tests {
    use super::{QUIET, Typed, holds};
    use oso_core::key::{Key, KeyPress};
    use oso_core::keymap::CANCEL;
    use std::time::{Duration, Instant};

    #[test]
    fn a_sequence_is_timed_by_its_first_key_and_the_one_before_it() {
        let start = Instant::now();
        let at = |ms| Some(start + Duration::from_millis(ms));
        let mut typed = Typed::default();
        typed.key(at(0).unwrap(), false);
        assert_eq!((typed.first, typed.before_first), (at(0), None));
        // Text typed, then a chord, an argument's key before it.
        typed.last = at(100);
        for (ms, goes_on) in [(200, false), (300, true), (900, true)] {
            typed.key(at(ms).unwrap(), goes_on);
            assert_eq!((typed.first, typed.before_first), (at(200), at(100)));
        }
        assert_eq!((typed.last, typed.before), (at(900), at(300)));
        typed.key(at(950).unwrap(), false);
        assert_eq!((typed.first, typed.before_first), (at(950), at(900)));
    }

    #[test]
    fn a_question_takes_no_key_until_a_quiet_moment_but_c_g() {
        let y = KeyPress::plain(Key::Char('y'));
        assert!(holds(Some(Duration::from_millis(100)), y));
        assert!(holds(Some(QUIET - Duration::from_millis(1)), y));
        assert!(!holds(Some(QUIET), y));
        assert!(!holds(None, y));
        assert!(!holds(Some(Duration::ZERO), CANCEL));
    }
}
