//! The emergency copies on offer: starting what the offer asks for, and taking in what its
//! threads report (ADR 0064).
//!
//! Must not depend on: key presses or how a frame is drawn. The directory and the copies are
//! read on threads of their own, never here.

use oso_core::buffer::BufferId;

use super::{App, UserEvent};
use crate::emergency::{self, CopiesEvent, CopyWork};

impl App {
    /// Finds out soon after startup whether copies could be written, and what waits.
    pub fn check_emergency_directory(&mut self) {
        self.offer.startup_scan_begins();
        let (report, found) = (self.waker.clone(), self.waker.clone());
        self.emergency.check_directory(
            &self.editor,
            move |problem| {
                // It fails only once the event loop is gone, when there is nobody to tell.
                let _ = report.send(UserEvent::NoEmergencyCopies(problem));
            },
            move |event| {
                let _ = found.send(UserEvent::Copies(event));
            },
        );
    }

    /// Does what a person asked of the copies since the last command. The usual command asks
    /// nothing, and this costs it one look at an empty list.
    pub(super) fn answer_about_copies(&mut self) {
        let after = self.offer.after_command(&self.editor);
        self.start_copy_work(after);
        let actions = self.editor.take_recovery_actions();
        if actions.is_empty() {
            return;
        }
        for action in actions {
            let work = self.offer.answer(&mut self.editor, action);
            self.start_copy_work(work);
            if let Some(load) = self.offer.take_load() {
                self.load_copy(load);
            }
        }
        // A recovery changed a buffer, outside the command that asked for it.
        self.dispatcher.publish_for_emergency(&self.editor);
        self.request_redraw();
    }

    /// A thread working on the copies has reported.
    pub(super) fn copies_event(&mut self, event: CopiesEvent) {
        let loading = self.editor.buffers().any(|(_, b)| b.loading().is_some());
        let work = self.offer.take_event(&mut self.editor, event, loading);
        self.start_copy_work(work);
        self.request_redraw();
    }

    /// A save has ended: its copy may have done its work, or what rows say may have changed.
    pub(super) fn copies_after_a_save(&mut self, buffer: BufferId, confirmed: bool) {
        let work = self.offer.save_ended(&mut self.editor, buffer, confirmed);
        self.start_copy_work(work);
    }

    /// A file has loaded since this was last asked, so copies may belong to it.
    pub(super) fn scan_if_a_file_has_loaded(&mut self) {
        if !std::mem::take(&mut self.scan_for_copies) {
            return;
        }
        let work = self.offer.ask_for_scan(&self.editor);
        self.start_copy_work(work);
    }

    /// A copy that failed to load is no use. It goes off this thread, however much had arrived.
    pub(super) fn free_copy_buffer(&mut self, buffer: BufferId) {
        if let Some(buffer) = self.editor.remove_internal_buffer(buffer) {
            self.start_copy_work(vec![CopyWork::Free(Box::new(buffer))]);
        }
    }

    fn start_copy_work(&mut self, work: Vec<CopyWork>) {
        for work in work {
            let waker = self.waker.clone();
            emergency::spawn_work(work, move |event| {
                // It fails only once the event loop is gone, when there is nobody to tell.
                let _ = waker.send(UserEvent::Copies(event));
            });
        }
    }
}
