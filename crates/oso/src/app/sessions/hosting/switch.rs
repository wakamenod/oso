//! `session-switch-mode` in the app (ADR 0165, step 4, part 4d): a session handed between its
//! modes, native and Claude Code's TUI, as ADR 0130's design has it. The rules and where each
//! switch is are `sessions::native::switch`'s; this carries them out.
//!
//! Must not depend on: key presses or how a frame is drawn; the processes end and start on
//! threads of their own.
//!
//! - **Native to the TUI** (part 4d-1): the session's `claude -p`, idle, ends as a stop ends
//!   it (standard input closed, `SIGTERM`, `SIGKILL` after 5 s); once its stream said it
//!   ended, after its reap, its transcript is set aside (`native::switching`), the record says
//!   `tui`, and the TUI resumes the conversation in a new terminal, `*claude: NAME*`, as
//!   `session-resume` resumes a stopped terminal session: through the start thread's look
//!   at Claude Code's registry, which refuses a live process of the conversation elsewhere.
//!   ADR 0121's screen rules and the tracker's screen entry take over from its start, and
//!   agents read its terminal only once the person lets them, as any session's. A stopped
//!   session is handed at once.
//! - Either way, each window of the frame's tree that showed the session shows it in its other
//!   mode, the side window and docked ones apart; the resume then shows it in the selected
//!   window, as `session-resume` does. The trees kept behind the frame are not changed.
//! - The TUI may ask Claude Code's own trust question, which `claude -p` skipped.
//! - A TUI start that is refused leaves the session a stopped TUI one, whose `session-resume`
//!   starts it again.
//! - **Back to native** (part 4d-2) is `back`'s.

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;
use oso_core::window::{Window, WindowId};

use super::resume::How;
use crate::app::App;
use crate::sessions::native::switch::{Then, Way};

mod back;
use crate::sessions::worker::SessionKey;

impl App {
    /// `session-switch-mode` in `buffer`: the session whose terminal, native transcript or
    /// composer it is switches to its other mode.
    pub(in crate::app) fn switch_session_mode(&mut self, buffer: BufferId) {
        let key = (self.composer_session(buffer))
            .or_else(|| self.aside_session(buffer))
            .or_else(|| self.hosting.table.by_buffer(buffer).map(|s| s.key));
        let Some(key) = key else {
            return self.editor.message("This buffer holds no session");
        };
        let Some(name) = self.hosting.table.get(key).map(|s| s.name.clone()) else {
            return self.editor.message("That session is gone");
        };
        match self.hosting.natives.has(key) {
            true => self.native_hand_to_tui(key, &name),
            false => self.tui_hand_to_native(key, &name),
        }
    }

    /// Hands native session `key`, `name`, to the TUI: its process ends first if it runs.
    fn native_hand_to_tui(&mut self, key: SessionKey, name: &str) {
        let Some(at) = self.native_switch_at(key) else {
            return;
        };
        match self.hosting.natives.switches.hand_to_tui(key, &at) {
            Err(refusal) => self.editor.message(refusal.message(name)),
            Ok(Then::End) => {
                self.end_native_for_tui(key);
                self.editor.message(format!(
                    "Session {name} goes to the TUI once its claude ended"
                ));
            }
            Ok(Then::Start) => self.start_tui_for(key),
        }
    }

    /// Native session `key`'s process ended for its switch to the TUI, which starts now,
    /// unless it was `refused` meanwhile: its process not seen reaped (and so killed then), a
    /// stop made, or a resume held since, by a stop asked, oso waiting to quit or restarting,
    /// or its orchestrator's replacement. Then the session stays a stopped native one.
    pub(in crate::app::sessions::hosting) fn native_ended_for_tui(
        &mut self,
        key: SessionKey,
        refused: bool,
    ) {
        if self.hosting.natives.switches.ended(key).is_none() {
            return;
        }
        if !refused {
            return self.start_tui_for(key);
        }
        self.hosting.natives.switches.over(key);
        self.native_switch_given_up(key);
        // An orchestrator stopped for its successor: that starts now (ADR 0136, step 5).
        self.replaced_orchestrator_ended(key, true);
    }

    /// Native session `key`, with no process, is handed to the TUI: its transcript set aside,
    /// the record saying `tui`, and the TUI started in a new terminal with what it held. A
    /// start refused at once is said, and the switch is over.
    fn start_tui_for(&mut self, key: SessionKey) {
        // The windows that showed its transcript show its terminal once it is there.
        let transcript = self
            .hosting
            .table
            .get(key)
            .and_then(|session| session.buffer);
        let windows = match transcript {
            Some(transcript) => tree_windows_showing(&self.editor, transcript),
            None => Vec::new(),
        };
        let Some(chosen) = self.set_native_aside(key) else {
            self.hosting.natives.switches.over(key);
            return;
        };
        let Some(session) = self.hosting.table.get_mut(key) else {
            self.hosting.natives.switches.over(key);
            return;
        };
        session.native = false;
        self.remember_sessions();
        // A stopped terminal of its own, `*claude: NAME*`, which the resume starts over.
        self.bring_back_stopped(key);
        self.show_session_in(key, &windows);
        let how = How {
            chosen: Some(chosen),
            ..How::BY_CLAIM
        };
        if let Err(why) = self.resume_hosted_with(key, true, how) {
            self.hosting.natives.switches.over(key);
            self.editor.message(format!("The TUI did not start: {why}"));
        }
    }

    /// Shows session `key`'s buffer, its other mode's now, in each of `windows`: those that
    /// showed it, so that a switch leaves no window of a split on another buffer. One that went
    /// meanwhile shows nothing.
    fn show_session_in(&mut self, key: SessionKey, windows: &[WindowId]) {
        let Some(buffer) = (self.hosting.table.get(key)).and_then(|session| session.buffer) else {
            return;
        };
        for &id in windows {
            self.editor.show_in(id, buffer);
        }
    }

    /// Start `token`'s start thread reported it `started`, was refused or found its
    /// conversation live: a switch that started it is over. A TUI a hand-off started is noted,
    /// as one that exits cleanly of itself comes back natively (Q4-6 (a)).
    pub(in crate::app::sessions::hosting) fn switch_start_reported(
        &mut self,
        token: SessionKey,
        started: bool,
    ) {
        if self.hosting.natives.switches.starting(token).is_none() {
            return;
        }
        if self.hosting.natives.switches.over(token) == Some(Way::ToTui) && started {
            self.hosting.natives.switches.handed(token);
        }
    }
}

/// The windows of the frame's tree that show `buffer`, the side window and docked ones apart
/// (`carry::tiled_windows`): those a session's buffer follows to its other mode. The trees
/// kept behind the frame (a zoom's, other spaces', the review's) are not among them.
fn tree_windows_showing(editor: &Editor, buffer: BufferId) -> Vec<WindowId> {
    (crate::carry::tiled_windows(editor).into_iter())
        .filter(|window| window.buffer() == buffer)
        .map(Window::id)
        .collect()
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::frame::{FrameSize, Way};
    use oso_core::window::DockSlot;

    use super::tree_windows_showing;

    /// Both halves of a split that show a transcript follow it; its docked composer, and a
    /// window on another buffer, do not.
    #[test]
    fn the_windows_that_follow_a_session_are_the_trees_that_show_it() {
        let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\n"));
        editor.set_frame_size(FrameSize {
            rows: 40,
            columns: 80,
        });
        let transcript = editor.window().buffer();
        let composer = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
        editor.set_dock(transcript, DockSlot::Composer, Some(composer));
        let top = editor.window().id();
        let bottom = editor.split_window(Way::Down, false).unwrap();
        let other = editor.add_buffer(Buffer::from_text("notes", "x\n"));
        let third = editor.split_window(Way::Across, true).unwrap();
        editor.show(other);
        let docked = editor.dock_window(transcript, DockSlot::Composer).unwrap();
        let mut shown = tree_windows_showing(&editor, transcript);
        shown.sort_unstable_by_key(|id| id.get());
        let mut expected = vec![top, bottom];
        expected.sort_unstable_by_key(|id| id.get());
        assert_eq!(shown, expected);
        assert!(!shown.contains(&docked) && !shown.contains(&third));
    }
}
