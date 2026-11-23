//! Dictation in the app (ADR 0175): the commands' requests, the dictation's thread and what it
//! says, the modeline's mark, the text typed where the dictation started, and the guard that
//! holds TypeWhisper's own Command-V while the text is due.
//!
//! Must not depend on: how a frame is drawn.
//!
//! Every call to TypeWhisper and every look at its discovery file runs on the dictation's
//! thread.
//!
//! - One dictation at a time. `s-d` starts one where the person is, and stops it; the place is
//!   taken at the start (`dictation::target`).
//! - The guard opens as oso asks the dictation to stop, or learns it stopped, and closes
//!   [`GUARD_AFTER`] after its text came, or after it failed once it had stopped, as
//!   TypeWhisper's paste may follow either, and at the latest that long after the text's own
//!   wait.
//! - Nothing is carried across a restart: an exit starts a stop if one records, and does not
//!   wait for it.

use std::time::{Duration, Instant};

use oso_core::command::Effects;
use oso_core::editor::{DictationMark, DictationRequest};

use super::shell::Loop;
use super::{App, UserEvent};
use crate::dictation::api::{Curl, Transport};
use crate::dictation::state::{self, Event, Handle, MakeTransport, Settings, Stopped};
use crate::dictation::target::{Placed, Target};
use crate::dictation::{self, MARKER};
use crate::dispatch::PasteGuard;

/// How long after a dictation's text came TypeWhisper's own Command-V is still dropped.
pub const GUARD_AFTER: Duration = Duration::from_secs(3);

/// How long an exit waits for a recording dictation's thread to have started its stop.
const EXIT_WAIT: Duration = Duration::from_millis(300);

/// Where a dictation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Its thread asked TypeWhisper to start.
    Starting,
    /// TypeWhisper records.
    Recording,
    /// It stopped, and the text is due.
    Waiting,
}

/// The dictation under way.
struct Current {
    generation: u64,
    handle: Handle,
    target: Target,
    phase: Phase,
    /// The person asked it to stop before it recorded: it stops as it starts.
    stop_asked: bool,
    /// `dictation-cancel` dropped it: its text goes nowhere.
    cancelled: bool,
    /// A prefix argument came with either press of `s-d`, kept for the fix that comes later
    /// (ADR 0175, step 2).
    prefix: bool,
}

/// Text that came while an input method composed in its window, and waits for it to end.
struct Due {
    generation: u64,
    target: Target,
    text: String,
    /// The dictation's thread, kept so that it says when the wait is over.
    _thread: Handle,
}

/// The app's dictations.
#[derive(Default)]
pub(super) struct Dictations {
    /// The generation the next dictation gets: what its thread says is told apart from an
    /// earlier one's.
    next: u64,
    current: Option<Current>,
    due: Option<Due>,
}

impl App {
    /// Carries out what commands asked of dictation since the last event.
    pub(super) fn sync_dictation(&mut self) {
        for request in self.editor.take_dictation_requests() {
            match request {
                DictationRequest::Toggle { prefix } => self.toggle_dictation(prefix),
                DictationRequest::Cancel => self.cancel_dictation(),
            }
        }
        self.place_due_dictation(false);
    }

    fn toggle_dictation(&mut self, prefix: bool) {
        let keys = self.dispatcher.where_is(&self.editor, "dictation-toggle");
        let Some(current) = &mut self.dictations.current else {
            return self.start_dictation(prefix);
        };
        current.prefix |= prefix;
        match current.phase {
            _ if current.cancelled => self.editor.message(format!(
                "TypeWhisper is still finishing the cancelled dictation; {keys} starts another \
                 once it has"
            )),
            Phase::Starting => current.stop_asked = true,
            Phase::Recording => {
                current.handle.stop();
                // From oso's stop on: TypeWhisper may paste before the stop's answer comes.
                self.open_paste_guard();
            }
            Phase::Waiting => self.editor.message(format!(
                "Dictation waits for TypeWhisper's text; M-x dictation-cancel drops it ({keys} \
                 starts another once it came)"
            )),
        }
    }

    /// Opens the guard for the text that is due: at the latest, it closes that long after the
    /// text's own wait, so that a thread that went quiet cannot leave it open.
    fn open_paste_guard(&mut self) {
        let until = Instant::now() + state::TIMING.text_limit + GUARD_AFTER;
        self.dispatcher.guard_pastes(PasteGuard::Until(until));
    }

    fn start_dictation(&mut self, prefix: bool) {
        if self.is_leaving_now() {
            return;
        }
        let target = match dictation::target::capture(&self.editor) {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let settings = self.configuration.dictation();
        let home = super::home();
        let discovery = match &settings.discovery {
            Some(written) => crate::config::expand(written, home.as_deref()),
            None => home.as_deref().and_then(dictation::discovery::default_path),
        };
        let Some(discovery) = discovery else {
            return self.editor.message(
                "Dictation needs TypeWhisper's discovery file: [dictation] discovery names it",
            );
        };
        let settings_now = Settings {
            discovery,
            workflow: settings.workflow.clone(),
            max_recording: settings.max_recording(),
        };
        let curl = settings.curl();
        // Looking at curl's file touches the disk, so the thread does it.
        let make: MakeTransport =
            Box::new(move || Curl::at(&curl).map(|curl| Box::new(curl) as Box<dyn Transport>));
        let generation = self.dictations.next;
        self.dictations.next += 1;
        let waker = self.waker.clone();
        let started = state::start(settings_now, make, state::TIMING, move |event| {
            waker.send(UserEvent::Dictation(generation, event));
        });
        let handle = match started {
            Ok(handle) => handle,
            Err(e) => {
                log::error!("cannot start a dictation's thread: {e}");
                return self.editor.message("Dictation could not start");
            }
        };
        // Text still waiting on a composition goes as it would at its limit.
        self.place_due_dictation(true);
        self.dictations.current = Some(Current {
            generation,
            handle,
            target,
            phase: Phase::Starting,
            stop_asked: false,
            cancelled: false,
            prefix,
        });
    }

    fn cancel_dictation(&mut self) {
        if self.dictations.due.take().is_some() {
            return self.editor.message("Dictation cancelled");
        }
        let Some(current) = self
            .dictations
            .current
            .as_mut()
            .filter(|current| !current.cancelled)
        else {
            return self.editor.message("No dictation to cancel");
        };
        current.cancelled = true;
        current.handle.stop();
        if current.phase == Phase::Recording {
            self.open_paste_guard();
        }
        self.editor.set_dictation_mark(DictationMark::Off);
        self.editor.message("Dictation cancelled");
        self.request_redraw();
    }

    /// Takes in what a dictation's thread said.
    pub(super) fn dictation_event(&mut self, generation: u64, event: Event) {
        if event == Event::Settled {
            return self.dictation_settled(generation);
        }
        let Some(current) = self
            .dictations
            .current
            .as_mut()
            .filter(|current| current.generation == generation)
        else {
            return;
        };
        let cancelled = current.cancelled;
        match event {
            Event::Recording => {
                current.phase = Phase::Recording;
                if current.stop_asked {
                    current.handle.stop();
                    self.open_paste_guard();
                }
                if !cancelled {
                    let keys = self.dispatcher.where_is(&self.editor, "dictation-toggle");
                    self.editor.set_dictation_mark(DictationMark::Recording);
                    self.editor.message(format!("Dictating: {keys} stops"));
                }
            }
            Event::Waiting(stopped) => {
                current.phase = Phase::Waiting;
                self.open_paste_guard();
                if !cancelled {
                    self.editor.set_dictation_mark(DictationMark::Waiting);
                    if stopped == Stopped::Limit {
                        let limit = self.configuration.dictation().max_recording().as_secs();
                        self.editor
                            .message(format!("Dictation stopped after {limit} s"));
                    }
                }
            }
            Event::Text(text) => {
                let Some(current) = self.dictations.current.take() else {
                    return;
                };
                self.close_paste_guard_soon();
                self.editor.set_dictation_mark(DictationMark::Off);
                if !current.cancelled {
                    self.dictated(current, &text);
                }
            }
            Event::Failed(why) => {
                let Some(current) = self.dictations.current.take() else {
                    return;
                };
                if current.phase == Phase::Waiting {
                    self.close_paste_guard_soon();
                }
                self.editor.set_dictation_mark(DictationMark::Off);
                // It may quote what TypeWhisper said, which stays out of the log (ADR 0138).
                if !current.cancelled {
                    self.editor.message_unlogged(why);
                }
            }
            Event::Settled => {}
        }
        self.request_redraw();
    }

    /// TypeWhisper's paste may still follow the text: the guard closes a moment later.
    fn close_paste_guard_soon(&mut self) {
        let until = Instant::now() + GUARD_AFTER;
        self.dispatcher.guard_pastes(PasteGuard::Until(until));
    }

    /// Types the text of the dictation `current` at its target, or keeps it, and its thread,
    /// until a composition there ends.
    fn dictated(&mut self, current: Current, text: &str) {
        let (text, cut) = dictation::clean(text);
        if text.is_empty() {
            return self.editor.message("TypeWhisper heard nothing");
        }
        if current.target.waits(&self.editor) {
            self.dictations.due = Some(Due {
                generation: current.generation,
                target: current.target,
                text,
                _thread: current.handle,
            });
            return;
        }
        self.place(&current.target, &text, cut);
    }

    /// Types `text` at `target`, or puts it in the kill ring, and says so.
    fn place(&mut self, target: &Target, text: &str, cut: bool) {
        match target.place(&mut self.editor, text) {
            Placed::Typed if cut => self.editor.message(format!(
                "Dictation typed its first {} KiB",
                dictation::MAX_TEXT >> 10
            )),
            Placed::Typed => {}
            Placed::Kept(why) => self.kept_dictation(why),
        }
        self.editor.follow_cursor();
        self.dispatcher.publish_for_emergency(&self.editor);
        self.request_redraw();
    }

    fn kept_dictation(&mut self, why: &str) {
        let yank = self.dispatcher.where_is(&self.editor, "yank");
        self.editor.message(format!(
            "Dictation kept in the kill ring: {why}; {yank} yanks it"
        ));
    }

    /// Types the text that waited for a composition once it ended, or, at `now` or past its
    /// wait, puts it in the kill ring.
    fn place_due_dictation(&mut self, now: bool) {
        let Some(due) = &self.dictations.due else {
            return;
        };
        if !now && due.target.waits(&self.editor) {
            return;
        }
        let Some(due) = self.dictations.due.take() else {
            return;
        };
        if now && due.target.waits(&self.editor) {
            due.target
                .keep(&mut self.editor, &due.text, "a composition stayed open");
            self.kept_dictation("a composition stayed open");
            self.request_redraw();
            return;
        }
        self.place(&due.target, &due.text, false);
    }

    /// A dictation's text came long enough ago: if it still waits for a composition, it goes to
    /// the kill ring.
    fn dictation_settled(&mut self, generation: u64) {
        if self
            .dictations
            .due
            .as_ref()
            .is_some_and(|due| due.generation == generation)
        {
            self.place_due_dictation(true);
        }
    }

    /// Asks the clipboard about an `M-v` the guard holds: only whether it carries TypeWhisper's
    /// marker. With no clipboard, or for keys from the dev socket over the system's, which no
    /// client may look at (ADR 0074), it is answered at once with no answer, and the key runs:
    /// then it returns what that did, which the caller follows with what it may have yanked.
    /// `None` once nothing more is answered here.
    pub(super) fn sync_paste_check(&mut self) -> Option<Effects> {
        let from_socket = self.dispatcher.take_paste_check()?;
        match &self.clipboard {
            Some(clipboard)
                if super::clipboard::may_read(
                    from_socket,
                    super::clipboard::is_memory(clipboard),
                ) =>
            {
                clipboard.has_type(MARKER);
                None
            }
            _ => Some(self.dispatcher.paste_checked(&mut self.editor, None)),
        }
    }

    /// The clipboard said whether it carries TypeWhisper's marker, for the `M-v` held.
    pub(super) fn paste_checked(&mut self, event_loop: &Loop<'_>, marked: Option<bool>) {
        let effects = self.dispatcher.paste_checked(&mut self.editor, marked);
        self.apply(event_loop, effects);
    }

    /// As oso exits: a dictation that records gets a stop, which is not waited for, and
    /// nothing is carried to the next instance.
    pub(super) fn leave_dictation(&mut self) {
        self.dictations.due = None;
        if let Some(current) = self.dictations.current.take() {
            current.handle.leave(EXIT_WAIT);
        }
    }

    /// Whether this instance is on its way out, when no new dictation starts.
    fn is_leaving_now(&self) -> bool {
        self.leaving.is_under_way()
    }
}
