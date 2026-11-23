//! Dictation in the app (ADR 0175): the commands' requests, the dictation's thread and what it
//! says, the modeline's mark, the text typed where the dictation started, its fix by Claude
//! (`fix`, step 2), and the guard that holds TypeWhisper's own Command-V while the text is due.
//!
//! Must not depend on: how a frame is drawn.
//!
//! Every call to TypeWhisper and every look at its discovery file runs on the dictation's
//! thread.
//!
//! - One dictation at a time. `s-d` starts one where the person is, and stops it; the place is
//!   taken at the start (`oso_core::editor`'s dictation). Its text is typed as a provisional
//!   span and fixed in the background, unless `C-u` came with either press.
//! - The guard opens as oso asks the dictation to stop, or learns it stopped, and closes
//!   [`GUARD_AFTER`] after its text came, or after it failed once it had stopped, as
//!   TypeWhisper's paste may follow either, and at the latest that long after the text's own
//!   wait.
//! - Once its text settles, fixed or plain, a dictation is watched for what it teaches
//!   (`learning`, step 3).
//! - Nothing is carried across a restart: an exit starts a stop if one records, and does not
//!   wait for it, kills the fixes under way and drops the watches; a composer's send that
//!   waits for them (`sessions/hosting/native/composer/held.rs`) does not go.

use std::time::{Duration, Instant};

use oso_core::command::Effects;
use oso_core::editor::{DictationMark, DictationPlaced, DictationRequest, DictationTarget};

use super::shell::Loop;
use super::{App, UserEvent};
use crate::dictation::api::{Curl, Transport};
use crate::dictation::state::{self, Event, Handle, MakeTransport, Settings, Stopped};
use crate::dictation::{self, MARKER};
use crate::dispatch::PasteGuard;

/// How long after a dictation's text came TypeWhisper's own Command-V is still dropped.
pub const GUARD_AFTER: Duration = Duration::from_secs(3);

mod fix;
mod learning;

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
    target: DictationTarget,
    phase: Phase,
    /// The person asked it to stop before it recorded: it stops as it starts.
    stop_asked: bool,
    /// `dictation-cancel` dropped it: its text goes nowhere.
    cancelled: bool,
    /// A prefix argument came with either press of `s-d`: the text is typed plain, with no
    /// fix (step 2).
    prefix: bool,
}

impl Current {
    /// What the modeline says of it.
    fn mark(&self) -> DictationMark {
        match self.phase {
            Phase::Starting => DictationMark::Off,
            Phase::Recording => DictationMark::Recording,
            Phase::Waiting => DictationMark::Waiting,
        }
    }
}

/// Text that came while an input method composed in its window, and waits for it to end.
struct Due {
    generation: u64,
    target: DictationTarget,
    text: String,
    /// Typed as a provisional span and fixed.
    provisional: bool,
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
    /// The fixes under way, each of a dictation typed already.
    fixes: Vec<fix::Fixing>,
    /// The id the next fix gets.
    next_fix: u64,
    /// The watches, and the learning's thread.
    learning: learning::Learning,
}

impl App {
    /// Carries out what commands asked of dictation since the last event.
    pub(super) fn sync_dictation(&mut self) {
        for request in self.editor.take_dictation_requests() {
            match request {
                DictationRequest::Toggle { prefix } => self.toggle_dictation(prefix),
                DictationRequest::Cancel => self.cancel_dictation(),
                DictationRequest::Forget => self.forget_dictation_learning(),
            }
        }
        self.place_due_dictation(false);
        self.land_dictation_fixes();
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
        let target = match self.editor.dictation_target() {
            Ok(target) => target,
            Err(why) => return self.editor.message(why),
        };
        let Some(discovery) = self.discovery_path() else {
            return self.editor.message(
                "Dictation needs TypeWhisper's discovery file: [dictation] discovery names it",
            );
        };
        let settings = self.configuration.dictation();
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

    /// Where TypeWhisper's discovery file is: `[dictation] discovery`, else the release
    /// build's.
    fn discovery_path(&self) -> Option<std::path::PathBuf> {
        let home = super::home();
        match &self.configuration.dictation().discovery {
            Some(written) => crate::config::expand(written, home.as_deref()),
            None => home.as_deref().and_then(dictation::discovery::default_path),
        }
    }

    fn cancel_dictation(&mut self) {
        if self.dictations.due.take().is_some() {
            return self.editor.message("Dictation cancelled");
        }
        if self.dictations.current.is_none() && self.stop_dictation_fixes() {
            self.editor
                .message("Dictation fix cancelled: the text stays as typed");
            // A composer's send that waited for it does not go with the text unfixed.
            self.drop_held_sends();
            return self.request_redraw();
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
        self.show_dictation_mark();
        self.editor.message("Dictation cancelled");
        self.request_redraw();
    }

    /// Takes in what a dictation's thread said.
    pub(super) fn dictation_event(&mut self, generation: u64, event: Event) {
        if event == Event::Settled {
            self.dictation_settled(generation);
            return self.land_dictation_fixes();
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
                    self.show_dictation_mark();
                    self.editor.message(format!("Dictating: {keys} stops"));
                }
            }
            Event::Waiting(stopped) => {
                current.phase = Phase::Waiting;
                self.open_paste_guard();
                if !cancelled {
                    self.show_dictation_mark();
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
                if !current.cancelled {
                    self.dictated(current, &text);
                }
                self.show_dictation_mark();
            }
            Event::Failed(why) => {
                let Some(current) = self.dictations.current.take() else {
                    return;
                };
                if current.phase == Phase::Waiting {
                    self.close_paste_guard_soon();
                }
                self.show_dictation_mark();
                // It may quote what TypeWhisper said, which stays out of the log (ADR 0138).
                if !current.cancelled {
                    self.editor.message_unlogged(why);
                }
            }
            Event::Settled => {}
        }
        self.land_dictation_fixes();
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
        let provisional = !current.prefix;
        if self.editor.dictation_waits(&current.target) {
            self.dictations.due = Some(Due {
                generation: current.generation,
                target: current.target,
                text,
                provisional,
                _thread: current.handle,
            });
            return;
        }
        self.place(&current.target, &text, cut, provisional);
    }

    /// Types `text` at `target`, or puts it in the kill ring, and says so; with `provisional`,
    /// as a span that Claude then fixes.
    fn place(&mut self, target: &DictationTarget, text: &str, cut: bool, provisional: bool) {
        match self.editor.place_dictation(target, text, provisional) {
            DictationPlaced::Typed { span } => {
                if cut {
                    self.editor.message(format!(
                        "Dictation typed its first {} KiB",
                        dictation::MAX_TEXT >> 10
                    ));
                }
                match span {
                    Some(span) if provisional => self.start_dictation_fix(span),
                    // Only text a rewrite is looked for in is copied to watch.
                    Some(span) if text.len() <= dictation::learning::rewrites::MAX_SIDE => {
                        let text = text.to_owned();
                        self.watch_dictation(span, text.clone(), text, Vec::new(), false);
                    }
                    Some(span) => self.editor.end_dictation_span(span),
                    None => {}
                }
            }
            DictationPlaced::Kept(why) => self.kept_dictation(why),
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
        if !now && self.editor.dictation_waits(&due.target) {
            return;
        }
        let Some(due) = self.dictations.due.take() else {
            return;
        };
        if now && self.editor.dictation_waits(&due.target) {
            self.editor
                .keep_dictation(&due.target, &due.text, "a composition stayed open");
            self.kept_dictation("a composition stayed open");
            self.request_redraw();
            return;
        }
        self.place(&due.target, &due.text, false, due.provisional);
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

    /// As oso exits: a dictation that records gets a stop, which is not waited for, the fixes
    /// under way are killed, and nothing is carried to the next instance.
    pub(super) fn leave_dictation(&mut self) {
        self.dictations.due = None;
        self.leave_dictation_fixes();
        self.drop_dictation_watches();
        if let Some(current) = self.dictations.current.take() {
            current.handle.leave(EXIT_WAIT);
        }
    }

    /// Whether this instance is on its way out, when no new dictation starts.
    fn is_leaving_now(&self) -> bool {
        self.leaving.is_under_way()
    }
}
