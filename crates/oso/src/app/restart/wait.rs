//! `oso-restart` waits for the native sessions (ADR 0165, part 4b): while one has a turn under
//! way, asks the person, is starting or is being interrupted, or while git or a file operation
//! runs, the restart
//! waits, with no question, and the echo area names them and says that `C-g` cancels. Once
//! nothing has held it for the hold (`IdleClock`), with no prompt open, the restart begins as
//! it would have. Terminals do not hold it, as they are carried. Meanwhile a session's queue
//! waits, as for an exit that waits (`Turn::leaving`), and the person's send starts the hold
//! again. The editor holds the wait (`Editor::restart_when_idle`), so that `C-g` and `C-x C-c`
//! cancel it as they cancel an exit's.
//!
//! Must not depend on: the renderer or the window. [`due_now`] is told the time, so it is
//! tested with no clock.

use std::time::Instant;

use super::super::App;
use super::super::idle_quit::IdleClock;
use super::super::shell::Loop;
use crate::sessions::hosted::Run;

impl App {
    /// Whether the restart waits now, saying for what if it does: the native sessions in a
    /// turn, asking, starting or interrupted, git's runs, and any other session starting.
    pub(super) fn wait_to_restart(&mut self) -> bool {
        if !self.restart_held() {
            return false;
        }
        self.editor.set_restart_when_idle(true);
        let mut holding = self.natives_holding_restart();
        holding.extend(self.editor.git_runs());
        holding.extend(self.file_operation_running());
        let named = crate::commands::session::names(&self.editor, &holding);
        self.editor
            .message(waiting_for(&named, self.others_starting()));
        true
    }

    /// Before the loop waits: begins the restart that waits once nothing has held it for the
    /// hold.
    pub(in crate::app) fn restart_if_idle(&mut self, event_loop: &Loop<'_>) {
        // A restart that ends its native sessions first is no longer waiting (part 4c).
        self.restart_ending_due(Instant::now());
        let waits = self.editor.restart_when_idle();
        if !waits || self.is_leaving(event_loop) || self.restart_holds() {
            self.restart.wait.stop();
            return;
        }
        // A prompt open, text being composed, or a search, undo, jump or macro under way would
        // refuse the restart, and ends by itself: it waits for them, as an exit that waits
        // does for a prompt. Anything else refuses it once it begins, saying why.
        let deferred = self.editor.prompt().is_some()
            || self.editor.preedit().is_some()
            || crate::carry::under_way(&self.editor).is_some();
        let held = self.restart_held();
        if !due_now(&mut self.restart.wait, held, deferred, Instant::now()) {
            return;
        }
        self.editor.set_restart_when_idle(false);
        self.begin_restart();
    }

    /// When the loop wakes for the restart that waits, if one does, or for one whose native
    /// sessions are ending.
    pub(in crate::app) fn restart_wait_deadline(&self) -> Option<Instant> {
        match (
            self.restart.wait.next_wake(),
            self.restart_ending_deadline(),
        ) {
            (Some(wait), Some(ending)) => Some(wait.min(ending)),
            (wait, ending) => wait.or(ending),
        }
    }

    /// Whether anything holds a restart back, with nothing allocated: a native session, a
    /// session starting, or git running or a file operation (ADR 0127, Q7; ADR 0134), which a
    /// restart would end part way.
    fn restart_held(&self) -> bool {
        self.sessions_starting()
            || self.natives_hold_restart()
            || self.editor.git_runs().next().is_some()
            || self.file_operation_running().is_some()
    }

    /// Whether a session other than a native one, which is named, is being started or waits
    /// for the configuration's first look (ADR 0173).
    fn others_starting(&self) -> bool {
        self.starts_held()
            || (self.hosting.table().iter()).any(|session| {
                session.run == Run::Starting && !self.hosting.natives.has(session.key)
            })
    }
}

/// Whether the restart that waits is due at `now`, as `clock` has it, with something `held`
/// or not. Due while it is `deferred`, the clock starts again: it is tried once more after a
/// whole hold.
fn due_now(clock: &mut IdleClock, held: bool, deferred: bool, now: Instant) -> bool {
    if !clock.due(!held, now) {
        return false;
    }
    clock.stop();
    !deferred
}

/// What the echo area says while the restart waits for the buffers `named`, and for another
/// session that is `starting`.
fn waiting_for(named: &str, starting: bool) -> String {
    let what = match (named.is_empty(), starting) {
        (false, false) => named.to_owned(),
        (false, true) => format!("{named} and a session starting"),
        (true, true) => "a session starting".to_owned(),
        (true, false) => "the sessions".to_owned(),
    };
    format!("oso-restart waits for {what} to be idle; C-g cancels")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use oso_core::buffer::BufferId;
    use oso_core::editor::Editor;

    use super::{IdleClock, due_now, waiting_for};
    use crate::commands::session::names;

    fn sessions(count: usize) -> (Editor, Vec<BufferId>) {
        let mut editor = Editor::default();
        let ids = (0..count)
            .map(|n| editor.add_native_session_buffer(format!("*claude: s{n}*")))
            .collect();
        (editor, ids)
    }

    #[test]
    fn the_echo_names_three_sessions_and_counts_the_rest() {
        let (editor, ids) = sessions(5);
        assert_eq!(
            waiting_for(&names(&editor, &ids[..1]), false),
            "oso-restart waits for *claude: s0* to be idle; C-g cancels"
        );
        assert_eq!(
            waiting_for(&names(&editor, &ids), false),
            "oso-restart waits for *claude: s0*, *claude: s1*, *claude: s2* and 2 more to be \
             idle; C-g cancels"
        );
        assert_eq!(
            waiting_for(&names(&editor, &ids[..2]), true),
            "oso-restart waits for *claude: s0*, *claude: s1* and a session starting to be \
             idle; C-g cancels"
        );
        assert_eq!(
            waiting_for("", true),
            "oso-restart waits for a session starting to be idle; C-g cancels"
        );
    }

    #[test]
    fn the_restart_is_due_after_a_whole_hold_with_nothing_held_or_deferred() {
        let hold = Duration::from_secs(2);
        let start = Instant::now();
        let mut clock = IdleClock::default();
        // Held: never due, and the loop is not woken for it.
        assert!(!due_now(&mut clock, true, false, start));
        assert_eq!(clock.next_wake(), None);
        // Idle: due after the hold.
        assert!(!due_now(&mut clock, false, false, start));
        assert_eq!(clock.next_wake(), Some(start + hold));
        // A prompt open when it is due: not yet, and the clock stops until the next look.
        assert!(!due_now(&mut clock, false, true, start + hold));
        assert_eq!(clock.next_wake(), None);
        let later = start + hold + Duration::from_secs(1);
        assert!(!due_now(&mut clock, false, false, later));
        assert!(
            !due_now(&mut clock, true, false, later + hold),
            "held again"
        );
        assert!(!due_now(&mut clock, false, false, later + hold));
        assert!(due_now(&mut clock, false, false, later + hold + hold));
        assert_eq!(clock.next_wake(), None, "tried once");
    }
}
