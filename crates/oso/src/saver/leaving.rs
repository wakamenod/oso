//! An exit agreed to while saves run: whether to go, keep waiting, or take it back
//! (ADR 0059). The state and every decision are here, with no window, and the app only acts
//! on what they say.
//!
//! Must not depend on: `app`, winit or the GPU.

use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

/// How long a save still running may hold the process once the window is hidden. It
/// is long enough for many gigabytes on a slow disk, and abandoning a save that is
/// getting somewhere is the worse mistake: a process with no window costs little. A
/// save that hangs past it (a stuck network file system) cannot keep it for good.
const FINISH_TIMEOUT: Duration = Duration::from_secs(600);

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
}

impl Leaving {
    /// Whether an exit is waiting for saves.
    pub fn is_under_way(&self) -> bool {
        self.under_way.is_some()
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
        Some(deadline)
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
