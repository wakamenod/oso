//! Keeping each window's wrap of its buffer in step (ADR 0100, ADR 0103): turning wrapping on
//! and off, following a change on clones before it commits, a width or a text that changed
//! under it, and the jobs that wrap off the UI thread and their reports.
//!
//! Must not depend on: `command` or `keymap`, threads or the clock; the host runs the jobs.
//!
//! - Only tiled windows wrap; a prompt's never does.
//! - A window holds a wrap only while its buffer wraps, and drops it when it shows another
//!   buffer; its place keeps the top's offset.
//! - Each command, wheel scroll, resize and report gives each wrap its budget anew, which
//!   its edits and the rows wrapped near the view share (ADR 0102, ADR 0103).

use super::Editor;
use crate::buffer::{BufferId, Rewrap};
use crate::display_map::DisplaySnapshot;
use crate::display_map::wrap_map::{WrapJob, WrapMap, WrapReport};
use crate::selection::{OffsetEdit, follow_offset};
use crate::sum_tree::Bias;
use crate::window::{Rows, Scroll};

/// A window's wrap and view once a change of its buffer is in, by window position.
pub(super) type MovedViews = Vec<(usize, Option<WrapMap>, Scroll)>;

impl Editor {
    /// Wraps the long lines of `id` in the windows that show it, each at its own width, or
    /// stops, as `toggle-truncate-lines` does (ADR 0100). Returns whether that changed.
    pub fn set_wraps(&mut self, id: BufferId, wraps: bool) -> bool {
        let Some(buffer) = self.buffer_mut(id) else {
            return false;
        };
        if buffer.wraps() == wraps {
            return false;
        }
        buffer.set_wraps(wraps);
        for index in 0..self.tiled_count() {
            if self.windows[index].buffer() == id {
                self.follow_window(index);
            }
        }
        true
    }

    /// Brings the windows showing `id` along with text that a load put in, which no splice
    /// led to: a wrap starts over, its top in the text as before.
    pub fn loaded(&mut self, id: BufferId) {
        for index in 0..self.tiled_count() {
            if self.windows[index].buffer() == id {
                self.sync_wrap(index);
                let (window, buffer) = self.window_mut(index);
                let display = buffer.display();
                window.realize_view(&display);
                window.snap_top(&display);
            }
        }
    }

    /// Makes the window at `index` hold a wrap while its buffer wraps, at its width, of its
    /// text as it is now: a text that no splice led to, as a load's, starts it over.
    pub(super) fn sync_wrap(&mut self, index: usize) {
        if index >= self.tiled_count() {
            return;
        }
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let (wraps, revision) = (buffer.wraps(), buffer.revision());
        let columns = window.size().columns;
        match (wraps, window.wrap_map().is_some()) {
            (true, false) => {
                let wrap = WrapMap::provisional(&display, columns);
                window.set_wrap(Some(wrap), &display, revision);
            }
            (false, true) => window.set_wrap(None, &display, revision),
            (true, true) => {
                let stale = window.wrapped_revision() != revision;
                if let Some(wrap) = window.wrap_map_mut() {
                    if stale {
                        wrap.reset(&display);
                    }
                    wrap.set_columns(columns, &display);
                }
                window.set_wrapped_revision(revision);
            }
            (false, false) => {}
        }
    }

    /// Gives each wrap its budget for what comes next: a command, a wheel scroll, a resize or
    /// a report.
    pub(super) fn refill_wraps(&mut self) {
        for window in &mut self.windows {
            if let Some(wrap) = window.wrap_map_mut() {
                wrap.refill();
            }
        }
    }

    /// The wraps and views of the windows showing `id` once a change is in, `display` showing
    /// it then: each wrap follows `rewrap` on a clone, within its budget, and each top follows
    /// `steps` as a marker, put back at its row's start. Nothing here changes the editor;
    /// [`Editor::commit_views`] puts them in.
    pub(super) fn moved_views(
        &self,
        id: BufferId,
        display: &DisplaySnapshot,
        steps: &[Vec<OffsetEdit>],
        rewrap: &Rewrap,
    ) -> MovedViews {
        let mut moved = Vec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            let wrap = window.wrap_map().map(|wrap| {
                let mut wrap = wrap.clone();
                match rewrap {
                    Rewrap::Splices(splices) => wrap.edit(splices),
                    Rewrap::Whole => wrap.reset(display),
                }
                wrap
            });
            let mut scroll = window.scroll();
            for edits in steps {
                scroll.start = follow_offset(scroll.start, edits, Bias::Left);
            }
            let rows = Rows::new(display, wrap.as_ref());
            scroll.start = rows.start(rows.row_of(scroll.start));
            moved.push((index, wrap, scroll));
        }
        moved
    }

    /// Puts in what [`Editor::moved_views`] made, once `id`'s change is in. Nothing here can
    /// panic (ADR 0053).
    pub(super) fn commit_views(&mut self, id: BufferId, moved: MovedViews) {
        let revision = self.buffer(id).map_or(0, |buffer| buffer.revision());
        for (index, wrap, scroll) in moved {
            if let Some(window) = self.windows.get_mut(index) {
                window.follow_change(wrap, scroll, revision);
            }
        }
    }

    /// The jobs the windows' wraps want run off the UI thread, each superseding its window's
    /// running one where it should (ADR 0100). The host runs them and hands back each report
    /// to [`Editor::take_wrap_report`].
    pub fn take_wrap_jobs(&mut self) -> Vec<WrapJob> {
        let mut jobs = Vec::new();
        for index in 0..self.tiled_count() {
            self.sync_wrap(index);
            let (window, buffer) = self.window_mut(index);
            let display = buffer.display();
            if let Some(job) = window
                .wrap_map_mut()
                .and_then(|wrap| wrap.take_job(&display))
            {
                jobs.push(job);
            }
        }
        jobs
    }

    /// Takes a job's report into the window whose wrap made it, if it still shows that wrap,
    /// and puts its top back at its row's start; the rows in view may break otherwise, but
    /// the text at the top stays in the first row (ADR 0100). Returns whether a frame is
    /// needed.
    pub fn take_wrap_report(&mut self, report: WrapReport) -> bool {
        let found = (0..self.tiled_count())
            .find(|&index| (self.windows[index].wrap_map()).is_some_and(|wrap| wrap.owns(&report)));
        let Some(index) = found else {
            return false;
        };
        self.refill_wraps();
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let taken = window
            .wrap_map_mut()
            .is_some_and(|wrap| wrap.take_report(report, &display));
        if taken {
            window.snap_top(&display);
            self.follow_window(index);
        }
        taken
    }
}
