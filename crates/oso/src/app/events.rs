//! The event loop's handler: sends each event from the window and from other
//! threads to the part of the app that deals with it. Events come in oso's own
//! types, `oso_platform::window`'s, from winit's adapter or the native layer (ADR 0070).
//! Each callback of the loop goes to one method of the app, here or in the concern it
//! belongs to (`open_window`, `ending`), so a window layer only translates.
//!
//! Must not depend on: what a key does or how a frame is drawn. A new event adds
//! an arm here, and its handling goes in a module beside this one.

use std::time::Instant;

use oso_platform::window::Event;

use super::shell::Loop;
use super::{App, UserEvent};

/// Whether `event` keeps what the app knows of the window true, rather than being input or a
/// frame. Missing a change of modifiers would leave Control held after a leave is taken back,
/// and every letter would then run a command.
fn keeps_state_true(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::ModifiersChanged(_) | Event::Resized { .. } | Event::ScaleFactorChanged(_)
    )
}

impl App {
    /// What follows every event, before the next is read: the input method's part, then the
    /// agent channel's (ADR 0069, ADR 0085).
    pub(super) fn after_event(&mut self, event_loop: &Loop<'_>) {
        self.sync_ime_after_event(event_loop);
        self.sync_agent();
        // At once, not at the next wait: a buffer that turned secret gets no slice after this.
        self.drop_syntax();
    }

    /// A thread sent the event loop something.
    pub(super) fn handle_user_event(&mut self, event_loop: &Loop<'_>, event: UserEvent) {
        self.begin_logging();
        match event {
            UserEvent::Load(buffer) => {
                self.load_updated(buffer);
            }
            UserEvent::Save(report) => self.save_ended(event_loop, report),
            UserEvent::Watch => self.watch_reported(),
            UserEvent::Configuration => self.configuration_reported(),
            UserEvent::NoEmergencyCopies(problem) => {
                // Only this level reaches a terminal with no `RUST_LOG`.
                log::error!("{}", problem.escape_debug());
                self.editor.message(problem);
            }
            UserEvent::Copies(event) => self.copies_event(event),
            UserEvent::Clipboard(event) => self.clipboard_event(event_loop, event),
            UserEvent::Trim(report) => self.trim_reported(report),
            UserEvent::Search(report) => self.isearch_reported(report),
            UserEvent::Wrap(report) => self.wrap_reported(report),
            UserEvent::Syntax(report) => self.syntax_reported(report),
            // `before_waiting` asks for the job again.
            UserEvent::WrapLost => {}
            UserEvent::Completion(report) => self.completion_reported(event_loop, report),
            UserEvent::CompletionFailed => self.completion_failed(),
            UserEvent::Listed => self.listed(),
            UserEvent::Projects => self.projects_found(),
            UserEvent::Walked(walked) => self.walked(event_loop, walked),
            UserEvent::Grepped(found) => self.grepped(event_loop, found),
            UserEvent::Trust => self.trust_reported(),
            #[cfg(feature = "dev")]
            UserEvent::Control(request) => self.handle_control(event_loop, request),
            UserEvent::Agent(request) => self.handle_agent(event_loop, request),
            UserEvent::AgentStarted(started) => self.agent_started(started),
            // `after_event` looks at the deadlines.
            UserEvent::AgentDeadline => {}
        }
        self.after_event(event_loop);
        self.note_logging();
    }

    /// The event loop is about to wait for the next events.
    pub(super) fn before_waiting(&mut self, event_loop: &Loop<'_>) {
        self.begin_logging();
        self.check_leaving(event_loop);
        self.scan_if_a_file_has_loaded();
        // Before the projects, whose worktrees are checked against the record as it is now.
        self.write_trust();
        self.find_projects();
        self.check_disk();
        self.free_replaced_keymap();
        // A load may have grown the buffer under isearch.
        self.start_searches();
        // `!` replaces a slice a turn (ADR 0113).
        self.replace_slice(event_loop);
        // Commands, a resize or a load may have left rows to wrap.
        self.start_wraps();
        // And text to highlight, or buffers to drop.
        self.start_syntax();
        self.add_quiet_walk(event_loop);
        self.start_completions(event_loop);
        // A list built here after a listing ends a wait with no report.
        self.run_held_completion(event_loop);
        self.ask_for_a_frame_if_status_changed();
        self.draw_flash_if_due();
        // Last, once every deadline is known.
        self.set_wake(event_loop);
        self.note_logging();
    }

    /// An event from the window.
    pub(super) fn handle_window_event(&mut self, event_loop: &Loop<'_>, event: Event<'_>) {
        self.begin_logging();
        // An exit only sets a flag, and the loop still delivers what is queued. Input
        // handled now, by any path, would be lost with the process. But a leave can be
        // taken back, so what the window's state depends on still counts.
        if self.view.is_none() || (self.is_leaving(event_loop) && !keeps_state_true(&event)) {
            return;
        }

        match event {
            // ---- Close window ----
            Event::CloseRequested => self.close_requested(event_loop),

            // ---- Resize ----
            Event::Resized { width, height } => self.resize(width, height),

            // ---- Moved to a display with another scale: reload the font ----
            Event::ScaleFactorChanged(scale_factor) => self.set_scale(scale_factor),

            // ---- Visible again: frames skipped while occluded need a redraw ----
            Event::Occluded(false) => self.request_redraw(),

            // ---- In front again: another program may have changed open files ----
            Event::Focused => {
                self.window_focused();
                self.look_at_configuration_again();
            }

            // ---- Redraw ----
            Event::RedrawRequested => self.draw_frame(),

            // ---- Mouse wheel and trackpad ----
            Event::Wheel(wheel) => self.handle_wheel(wheel),

            // ---- Keyboard input ----
            Event::ModifiersChanged(held) => self.set_modifiers(held),
            Event::Key(input) => self.handle_key_event(event_loop, &input),

            // ---- Input method ----
            Event::Ime(ime) => {
                self.ime_event(event_loop, ime);
            }

            // A flash ends with the window out of sight (ADR 0097).
            Event::Occluded(true) => self.end_flash(),
            Event::InputSource(source) => self.input_source_chosen(source),
            Event::Other => {}
        }
        self.after_event(event_loop);
        self.note_logging();
    }
}

/// The earliest of `deadlines`, or none.
fn earliest(deadlines: impl IntoIterator<Item = Option<Instant>>) -> Option<Instant> {
    deadlines.into_iter().flatten().min()
}

impl App {
    /// Sets when the event loop wakes with no event: the earliest of the deadlines it is told
    /// to wake for, or never (ADR 0097). The only place that sets it, as `wake_at` holds one
    /// deadline and a second caller would cancel the first's. The agent channel's opens
    /// wake it through an event of their own.
    pub(super) fn set_wake(&self, event_loop: &Loop<'_>) {
        let deadlines = [
            self.leaving.next_wake(),
            self.flash.next_wake(),
            self.reload_deadline(),
            self.quiet_walk_deadline(),
        ];
        match earliest(deadlines) {
            Some(deadline) => event_loop.wake_at(deadline),
            None => event_loop.sleep_when_idle(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{earliest, keeps_state_true};
    use oso_platform::window::{Event, Held};

    #[test]
    fn only_what_keeps_the_windows_state_true_passes_while_leaving() {
        assert!(keeps_state_true(&Event::ModifiersChanged(Held::NONE)));
        assert!(keeps_state_true(&Event::Resized {
            width: 1,
            height: 1
        }));
        assert!(keeps_state_true(&Event::ScaleFactorChanged(2.0)));
        assert!(!keeps_state_true(&Event::RedrawRequested));
        assert!(!keeps_state_true(&Event::CloseRequested));
        assert!(!keeps_state_true(&Event::Other), "focus, among others");
    }

    #[test]
    fn the_wake_is_the_earliest_deadline_or_none() {
        let now = std::time::Instant::now();
        let later = now + std::time::Duration::from_secs(1);
        assert_eq!(earliest([Some(later), None, Some(now)]), Some(now));
        assert_eq!(earliest([None, Some(later)]), Some(later));
        assert_eq!(earliest([None, None]), None);
    }
}
