//! An exit agreed to while saves run: whether to go, keep waiting, or take it back
//! (ADR 0059). The state and every decision are here, with no window, and the app only acts
//! on what they say.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

/// How long a save still running may hold the process once the window is hidden. It
/// is long enough for many gigabytes on a slow disk, and abandoning a save that is
/// getting somewhere is the worse mistake: a process with no window costs little. A
/// save that hangs past it (a stuck network file system) cannot keep it for good.
const FINISH_TIMEOUT: Duration = Duration::from_secs(600);

/// How long the end of the session waits for running saves, then lets it go: far below what
/// macOS allows an app, so that oso never holds up a logout for long (ADR 0077). A save not
/// done by then has an emergency copy of its buffer.
const SESSION_END_WAIT: Duration = Duration::from_secs(5);

/// What an exit under way does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterSave {
    /// Another save is still running.
    KeepWaiting,
    /// Nothing is left to wait for.
    Exit,
    /// Take the exit back: the window returns, with the buffer and a message.
    StayOpen,
}

/// What this instance has agreed to lose, should it end now. An end nobody agreed to is the
/// OS's (Dock quit, logout), or a panic, and the text it would lose gets an emergency copy
/// (ADR 0062).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agreement {
    /// Nothing: nobody has asked to go.
    Nothing,
    /// Modified buffers, but not the saves it is waiting for. Their text is meant to reach
    /// the disk, and an end now could stop that.
    ModifiedButNotSaves,
    /// Everything: the event loop has been told to end.
    Everything,
}

impl Agreement {
    const fn number(self) -> u8 {
        match self {
            Self::Nothing => 0,
            Self::ModifiedButNotSaves => 1,
            Self::Everything => 2,
        }
    }

    /// The agreement behind a number. Anything else would write nothing, though nothing else
    /// is ever stored.
    const fn from_number(number: u8) -> Self {
        match number {
            0 => Self::Nothing,
            1 => Self::ModifiedButNotSaves,
            _ => Self::Everything,
        }
    }
}

/// The [`Agreement`] now, for whoever must read it in a panic. Only [`Leaving`] writes it in
/// the running app, on every change, so that no path through the app can leave it stale.
#[derive(Clone, Default)]
pub struct AgreementCell(Arc<AtomicU8>);

impl AgreementCell {
    /// The agreement now.
    pub fn get(&self) -> Agreement {
        Agreement::from_number(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn set(&self, agreement: Agreement) {
        self.0.store(agreement.number(), Ordering::Relaxed);
    }
}

/// An exit agreed to, waiting for saves.
struct UnderWay {
    /// When to stop waiting.
    deadline: Instant,
    /// Go whatever happens, as the socket's `exit` asks (ADR 0055).
    anyway: bool,
    /// The buffers known to differ from their files when the person agreed. Others that
    /// come to differ are text they were never told of.
    modified: Vec<BufferId>,
}

/// Whether an exit is waiting for saves, and which saves are no longer worth waiting for.
#[derive(Default)]
pub struct Leaving {
    under_way: Option<UnderWay>,
    /// Buffers whose save outlived an exit, which the next exit does not wait for again.
    stuck: Vec<BufferId>,
    /// Nothing is left to wait for, and the event loop has been told to end.
    gone: bool,
    /// Where the agreement is kept for the panic hook and the end of the event loop.
    cell: AgreementCell,
    /// The session is ending, and waits for running saves until then (ADR 0084).
    session_end: Option<Instant>,
}

impl Leaving {
    /// No exit under way, keeping what is agreed to in `cell`.
    pub fn with_cell(cell: AgreementCell) -> Self {
        Self {
            cell,
            ..Self::default()
        }
    }

    /// Whether an exit is waiting for saves.
    pub fn is_under_way(&self) -> bool {
        self.under_way.is_some()
    }

    /// Keeps what is now agreed to where the panic hook can read it.
    fn update_cell(&self) {
        self.cell.set(if self.gone {
            Agreement::Everything
        } else if self.under_way.is_some() {
            Agreement::ModifiedButNotSaves
        } else {
            Agreement::Nothing
        });
    }

    /// Nothing is left to wait for: the app is about to end the event loop.
    pub fn go(&mut self) {
        self.under_way = None;
        self.gone = true;
        self.update_cell();
    }

    /// An exit was agreed to. `None` to go now, or when to stop waiting for the saves
    /// running. `anyway` is for the socket's `exit`, which means to go whatever the buffers
    /// hold. A person's exit comes back if it turns out the text is only here.
    pub fn begin(&mut self, editor: &Editor, anyway: bool, now: Instant) -> Option<Instant> {
        if !self.any_worth_waiting_for(editor) {
            return None;
        }
        let deadline = now + FINISH_TIMEOUT;
        self.under_way = Some(UnderWay {
            deadline,
            anyway,
            modified: modified_buffers(editor).collect(),
        });
        self.update_cell();
        Some(deadline)
    }

    /// The session is ending: a logout, a restart or a shutdown, which is never refused.
    /// `None` to let it go now, or when to stop waiting for the saves running. Nothing is
    /// agreed to, so what is at risk gets copies when the process ends.
    #[cfg_attr(
        all(not(test), not(all(target_os = "macos", feature = "native-window"))),
        expect(
            dead_code,
            reason = "only oso's own macOS layer asks the app whether to end"
        )
    )]
    pub fn begin_session_end(&mut self, editor: &Editor, now: Instant) -> Option<Instant> {
        if !self.any_worth_waiting_for(editor) {
            return None;
        }
        let deadline = now + SESSION_END_WAIT;
        self.session_end = Some(deadline);
        Some(deadline)
    }

    /// Whether the end of the session waits no longer, as its saves have ended or its time
    /// is up. Once it says so, no end of the session waits.
    pub fn session_end_ready(&mut self, editor: &Editor, now: Instant) -> bool {
        let Some(deadline) = self.session_end else {
            return false;
        };
        let ready = now >= deadline || !self.any_worth_waiting_for(editor);
        if ready {
            self.session_end = None;
        }
        ready
    }

    /// Whether the end of the session waits for saves.
    #[cfg(test)]
    fn session_end_waits(&self) -> bool {
        self.session_end.is_some()
    }

    /// Ends the wait of the end of the session, as the app goes anyway. Returns whether one
    /// waited.
    pub fn take_session_end(&mut self) -> bool {
        self.session_end.take().is_some()
    }

    /// A save has ended, and its report has been taken in. `None` if no exit is under way.
    pub fn save_ended(&mut self, editor: &Editor, failed: bool) -> Option<AfterSave> {
        self.stuck
            .retain(|id| editor.buffer(*id).is_some_and(|buffer| buffer.is_saving()));
        let still_saving = self.any_worth_waiting_for(editor);
        let under_way = self.under_way.as_ref()?;
        let newly_modified = modified_buffers(editor).any(|id| !under_way.modified.contains(&id));
        let step = next_step(failed || newly_modified, under_way.anyway, still_saving);
        if step != AfterSave::KeepWaiting {
            self.under_way = None;
            self.update_cell();
        }
        Some(step)
    }

    /// Time has passed. `None` unless an exit has now waited for too long. Nobody can tell
    /// whether the text reached the disk, so a person gets the window and the buffer back,
    /// and exiting again goes without waiting for what is running now.
    pub fn check_deadline(&mut self, editor: &Editor, now: Instant) -> Option<AfterSave> {
        let under_way = self.under_way.as_ref()?;
        if now < under_way.deadline {
            return None;
        }
        let step = next_step(true, under_way.anyway, false);
        if step == AfterSave::StayOpen {
            self.stuck = editor
                .buffers()
                .filter(|(_, buffer)| buffer.is_saving())
                .map(|(id, _)| id)
                .collect();
        }
        self.under_way = None;
        self.update_cell();
        Some(step)
    }

    /// Whether a save is running that has not already outlived an exit.
    fn any_worth_waiting_for(&self, editor: &Editor) -> bool {
        editor
            .buffers()
            .any(|(id, buffer)| buffer.is_saving() && !self.stuck.contains(&id))
    }
}

fn modified_buffers(editor: &Editor) -> impl Iterator<Item = BufferId> + '_ {
    editor
        .buffers()
        .filter(|(_, buffer)| buffer.is_modified())
        .map(|(id, _)| id)
}

/// What to do once a save has ended. `at_risk` says text is only in this process that the
/// person did not agree to lose: a save failed, or an undo made a buffer differ from what
/// has just reached the disk. They get it back at once, whatever else runs.
fn next_step(at_risk: bool, anyway: bool, still_saving: bool) -> AfterSave {
    if at_risk && !anyway {
        AfterSave::StayOpen
    } else if still_saving {
        AfterSave::KeepWaiting
    } else {
        AfterSave::Exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AfterSave::{Exit, KeepWaiting, StayOpen};
    use oso_core::buffer::Buffer;
    use oso_core::editor::SaveRefused;

    /// An editor with a save of its modified buffer running.
    fn saving() -> (Editor, BufferId) {
        let mut editor = Editor::with_buffer(Buffer::visiting("/p/notes.txt".into()));
        let id = editor.window().buffer();
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.finish_empty();
        buffer.set_baseline(oso_core::buffer::Baseline {
            location: "/p/notes.txt".into(),
            file: None,
        });
        editor
            .apply_edits(
                id,
                &[oso_core::buffer::Replacement {
                    range: 0..0,
                    text: "text\n",
                }],
            )
            .unwrap();
        assert!(!matches!(editor.start_save(), Err(SaveRefused::Unmodified)));
        (editor, id)
    }

    #[test]
    fn the_end_of_the_session_waits_for_saves_a_short_while_and_agrees_to_nothing() {
        let now = Instant::now();
        let mut leaving = Leaving::default();
        let idle = Editor::with_buffer(Buffer::from_text("t", "x"));
        assert_eq!(
            leaving.begin_session_end(&idle, now),
            None,
            "nothing to wait for"
        );
        let (editor, _) = saving();
        let deadline = leaving.begin_session_end(&editor, now).unwrap();
        assert_eq!(deadline, now + SESSION_END_WAIT);
        assert!(leaving.session_end_waits());
        assert_eq!(leaving.cell.get(), Agreement::Nothing, "copies at the end");
        assert!(!leaving.session_end_ready(&editor, now), "the save runs");
        assert!(
            leaving.session_end_ready(&editor, deadline),
            "the time is up"
        );
        assert!(!leaving.session_end_waits());
        assert!(!leaving.session_end_ready(&editor, deadline), "once");
        // Its saves ending lets it go at once.
        leaving.begin_session_end(&editor, now).unwrap();
        assert!(
            leaving.session_end_ready(&idle, now),
            "no save runs any more"
        );
        leaving.begin_session_end(&editor, now).unwrap();
        assert!(leaving.take_session_end(), "the app goes by itself");
        assert!(!leaving.take_session_end() && !leaving.session_end_waits());
    }

    #[test]
    fn nothing_is_agreed_to_until_a_person_or_the_socket_asks() {
        assert_eq!(Leaving::default().cell.get(), Agreement::Nothing);
    }

    #[test]
    fn a_number_stands_for_its_agreement() {
        for agreement in [
            Agreement::Nothing,
            Agreement::ModifiedButNotSaves,
            Agreement::Everything,
        ] {
            assert_eq!(Agreement::from_number(agreement.number()), agreement);
        }
        assert_eq!(Agreement::from_number(9), Agreement::Everything);
    }

    #[test]
    fn an_exit_with_nothing_to_wait_for_agrees_to_everything() {
        let mut leaving = Leaving::default();
        let editor = Editor::default();
        assert_eq!(leaving.begin(&editor, false, Instant::now()), None);
        leaving.go();
        assert_eq!(leaving.cell.get(), Agreement::Everything);
    }

    #[test]
    fn a_save_held_exit_keeps_the_saves_until_it_goes_or_is_taken_back() {
        let (editor, _) = saving();
        let mut leaving = Leaving::default();
        assert!(leaving.begin(&editor, false, Instant::now()).is_some());
        assert_eq!(leaving.cell.get(), Agreement::ModifiedButNotSaves);

        // Taken back: nothing is agreed to any more.
        assert_eq!(leaving.save_ended(&editor, true), Some(StayOpen));
        assert_eq!(leaving.cell.get(), Agreement::Nothing);

        assert!(leaving.begin(&editor, true, Instant::now()).is_some());
        assert_eq!(leaving.cell.get(), Agreement::ModifiedButNotSaves);
        leaving.go();
        assert_eq!(leaving.cell.get(), Agreement::Everything);

        // Taken back when it has waited for too long, and the next exit does not wait.
        let mut leaving = Leaving::default();
        let deadline = leaving.begin(&editor, false, Instant::now()).unwrap();
        assert_eq!(leaving.check_deadline(&editor, deadline), Some(StayOpen));
        assert_eq!(leaving.cell.get(), Agreement::Nothing);
        assert_eq!(leaving.begin(&editor, false, Instant::now()), None);
    }

    #[test]
    fn text_at_risk_takes_a_persons_exit_back_at_once_and_not_the_sockets() {
        // At risk, the socket's `exit`, another save running.
        assert_eq!(next_step(false, false, false), Exit);
        assert_eq!(next_step(false, false, true), KeepWaiting);
        assert_eq!(next_step(true, false, false), StayOpen);
        assert_eq!(next_step(true, false, true), StayOpen, "at once");
        assert_eq!(next_step(true, true, false), Exit);
        assert_eq!(next_step(true, true, true), KeepWaiting);
        assert_eq!(next_step(false, true, false), Exit);
    }
}
