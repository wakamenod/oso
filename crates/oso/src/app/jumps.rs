//! The time a jump's string is read for (ADR 0116): after each input while a jump reads its
//! string, its labels show once `[jump] timeout` passes with no other. What a jump finds and
//! labels is the editor's; this only keeps time.
//!
//! Must not depend on: the renderer or the window. [`JumpTimer`] is told the time, so it is
//! tested with no clock.

use std::time::{Duration, Instant};

use oso_core::editor::{Editor, MacroEvent};
use oso_core::jump::Stage;
use oso_core::key::{Key, KeyPress, NamedKey};

use super::App;
use super::flash::EARLY;

/// How long a jump reads its string after the last key, as avy's `avy-timeout-seconds`.
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// When a jump reading its string shows its labels, if one does.
#[derive(Debug)]
pub(super) struct JumpTimer {
    timeout: Duration,
    due: Option<Instant>,
}

impl Default for JumpTimer {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            due: None,
        }
    }
}

impl JumpTimer {
    /// Sets how long a string is read after its last key, as `[jump] timeout` gives it.
    pub(super) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// After an input at `now`: the time starts again while a jump `reading` its string is
    /// under way, and stops otherwise.
    fn after_input(&mut self, reading: bool, now: Instant) {
        self.due = reading.then(|| now + self.timeout);
    }

    /// When the loop must wake for it.
    pub(super) fn next_wake(&self) -> Option<Instant> {
        self.due
    }

    /// Whether its time has come at `now`, which it then forgets.
    fn take_due(&mut self, now: Instant) -> bool {
        match self.due {
            Some(due) if now + EARLY >= due => {
                self.due = None;
                true
            }
            _ => false,
        }
    }
}

impl App {
    /// After an input the dispatcher handled, if one came: the jump's time starts again.
    pub(super) fn time_jump(&mut self, input: bool) {
        if input {
            let reading = (self.editor.jump()).is_some_and(|jump| jump.stage() == Stage::Reading);
            self.jump_timer.after_input(reading, Instant::now());
        }
    }

    /// Shows a jump's labels once its time has passed. A string not begun waits for a key.
    /// A replay's own time never passes.
    pub(super) fn show_jump_labels_if_due(&mut self) {
        if !self.jump_timer.take_due(Instant::now()) || self.editor.macro_replaying() {
            return;
        }
        show_labels_as_timed(&mut self.editor);
        self.request_redraw();
    }
}

/// Shows a jump's labels as its time passing does. A keyboard macro being recorded keeps that
/// as the `RET` that does the same, so that its replay shows the labels where they showed,
/// however fast it runs.
fn show_labels_as_timed(editor: &mut Editor) {
    let reading =
        |editor: &Editor| (editor.jump()).is_some_and(|jump| jump.stage() == Stage::Reading);
    let was_reading = reading(editor);
    editor.jump_show_labels();
    if was_reading && !reading(editor) {
        editor.macro_record([MacroEvent::Key(KeyPress::plain(Key::Named(
            NamedKey::Return,
        )))]);
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TIMEOUT, JumpTimer, show_labels_as_timed};
    use crate::dispatch::Dispatcher;
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::window::ViewSize;
    use std::time::{Duration, Instant};

    fn send(d: &mut Dispatcher, editor: &mut Editor, keys: &str) {
        let keys: KeySequence = keys.parse().unwrap();
        d.handle_keys(editor, keys.0);
    }

    #[test]
    fn a_macro_keeps_the_time_passing_so_its_replay_jumps_to_the_same_place() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "xab ab\nab\n"));
        editor.set_view_size(ViewSize::new(10, 20));
        let mut d = Dispatcher::new();
        send(&mut d, &mut editor, "C-x ( M-j a b");
        show_labels_as_timed(&mut editor);
        // Nearest first: 1 on the cursor's row, then 4.
        send(&mut d, &mut editor, "s C-x )");
        let cursor = |editor: &Editor| editor.window().selections.primary().cursor;
        assert_eq!(cursor(&editor), 4);
        send(&mut d, &mut editor, "C-a C-x e");
        let until = Instant::now() + Duration::from_secs(10);
        while editor.macro_replaying() {
            d.replay(&mut editor, until);
        }
        assert!(
            editor.jump().is_none(),
            "the replay showed the labels and took one"
        );
        assert_eq!(cursor(&editor), 4);
    }

    #[test]
    fn each_key_while_reading_starts_the_time_again_and_others_stop_it() {
        let start = Instant::now();
        let mut timer = JumpTimer::default();
        assert_eq!(timer.next_wake(), None);
        timer.after_input(true, start);
        assert_eq!(timer.next_wake(), Some(start + DEFAULT_TIMEOUT));
        let later = start + Duration::from_millis(300);
        timer.after_input(true, later);
        assert!(!timer.take_due(start + DEFAULT_TIMEOUT));
        assert!(timer.take_due(later + DEFAULT_TIMEOUT));
        assert_eq!(timer.next_wake(), None, "once");
        timer.after_input(true, start);
        timer.after_input(false, start);
        assert_eq!(timer.next_wake(), None);
        timer.set_timeout(Duration::from_secs(2));
        timer.after_input(true, start);
        assert_eq!(timer.next_wake(), Some(start + Duration::from_secs(2)));
    }
}
