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
//! - The `C-x b` preview's buffer is shown by no window, so the editor keeps its wrap at the
//!   preview's width: provisional, one row a logical line, made in O(log n) when the preview
//!   changes buffer, width or text, and wrapped near the preview's start within each
//!   command's budget. No job wraps the rest, as nothing scrolls a preview (ADR 0104).

use super::{Editor, Preview};
use crate::buffer::{BufferId, Rewrap};
use crate::display_map::DisplaySnapshot;
use crate::display_map::wrap_map::{COMMAND_BUDGET, WrapJob, WrapMap, WrapReport};
use crate::selection::{OffsetEdit, follow_offset};
use crate::sum_tree::Bias;
use crate::window::{Scroll, Window};

/// The preview's rows of the buffer it shows.
pub(super) struct PreviewWrap {
    id: BufferId,
    /// The buffer's revision and the preview's width the rows were made for.
    revision: u64,
    columns: usize,
    wrap: WrapMap,
}

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
        self.refill_wraps();
        // A previewed buffer's part too.
        self.fit_preview();
        for index in 0..self.tiled_count() {
            if self.windows[index].buffer() == id {
                self.sync_wrap(index);
                let (window, buffer) = self.window_mut(index);
                let display = buffer.display();
                window.realize_view(&display);
            }
        }
    }

    /// Makes the window at `index` hold a wrap while its buffer wraps, at its width, of its
    /// text as it is now: a text that no splice led to, as a load's, starts it over.
    pub(super) fn sync_wrap(&mut self, index: usize) {
        if index >= self.tiled_count() {
            return;
        }
        // What it wraps here, and as it follows next, spends from the command's one budget,
        // a new wrap's own included (ADR 0112).
        let budget = self.gather_wrap_budget();
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let (wraps, revision) = (buffer.wraps(), buffer.revision());
        let columns = window.size().columns;
        let unspent = match (wraps, window.wrap_map().is_some()) {
            (true, false) => {
                let mut wrap = WrapMap::provisional(&display, columns);
                wrap.take_budget();
                wrap.give_budget(budget);
                window.set_wrap(Some(wrap), &display, revision);
                0
            }
            (false, true) => {
                window.set_wrap(None, &display, revision);
                budget
            }
            (true, true) => {
                let stale = window.wrap_revision() != revision;
                if let Some(wrap) = window.wrap_map_mut() {
                    wrap.give_budget(budget);
                    if stale {
                        wrap.reset(&display);
                    }
                    wrap.set_columns(columns, &display);
                }
                window.set_wrap_revision(revision);
                if stale {
                    window.realize_top(&display);
                }
                // Wide enough to wrap again, it shows from the left edge (ADR 0104).
                let scroll = window.scroll();
                if scroll.left != 0 && window.rows(&display).wrapped() {
                    window.set_scroll(Scroll { left: 0, ..scroll });
                }
                0
            }
            (false, false) => budget,
        };
        self.wrap_pool = unspent;
    }

    /// Gives the wraps their budget for what comes next: a command, a wheel scroll, a resize or
    /// a report. All windows share one (ADR 0112): the selected tiled window holds it first,
    /// and each window that wraps or follows its cursor after it takes what is left
    /// ([`Editor::pass_wrap_budget`]). Between them it waits in the editor's pool.
    pub(super) fn refill_wraps(&mut self) {
        self.gather_wrap_budget();
        self.wrap_pool = COMMAND_BUDGET;
        self.pass_wrap_budget(self.tiled_index());
    }

    /// Takes what is left of the command's wrap budget, from the pool and every wrap, which
    /// keep none of it.
    pub(super) fn gather_wrap_budget(&mut self) -> usize {
        let mut left = std::mem::take(&mut self.wrap_pool);
        for window in &mut self.windows {
            if let Some(wrap) = window.wrap_map_mut() {
                left = left.saturating_add(wrap.take_budget());
            }
        }
        left.min(COMMAND_BUDGET)
    }

    /// Moves what is left of the command's wrap budget into the wrap of the window at `index`,
    /// or into the pool while it has none (ADR 0112).
    pub(super) fn pass_wrap_budget(&mut self, index: usize) {
        let left = self.gather_wrap_budget();
        match self.windows.get_mut(index).and_then(Window::wrap_map_mut) {
            Some(wrap) => wrap.give_budget(left),
            None => self.wrap_pool = left,
        }
    }

    /// The wraps and views of the windows showing `id` once a change is in, `display` showing
    /// it then: each wrap follows `rewrap` on a clone, within its budget, and each top follows
    /// `steps` as a marker. Nothing here changes the editor; [`Editor::commit_views`] puts
    /// them in.
    pub(super) fn moved_views(
        &self,
        id: BufferId,
        display: &DisplaySnapshot,
        steps: &[Vec<OffsetEdit>],
        rewrap: &Rewrap,
    ) -> MovedViews {
        let mut moved = Vec::new();
        let revision = self.buffer(id).map_or(0, |buffer| buffer.revision());
        for (index, window) in self.windows.iter().enumerate() {
            if window.buffer() != id {
                continue;
            }
            // A wrap that missed a change since cannot follow this one's splices.
            let whole = window.wrap_revision() != revision;
            let mut scroll = window.scroll();
            for edits in steps {
                scroll.start = follow_offset(scroll.start, edits, Bias::Left);
            }
            let wrap = window.wrap_map().map(|wrap| {
                let mut wrap = wrap.clone();
                match rewrap {
                    Rewrap::Splices(splices) if !whole => wrap.edit(splices),
                    _ => wrap.reset(display),
                }
                // A provisional row at the top is wrapped, so that the view does not show
                // from its logical line's start.
                wrap.realize(display, scroll.start, 0);
                wrap
            });
            moved.push((index, wrap, scroll));
        }
        moved
    }

    /// Puts in what [`Editor::moved_views`] made, once `id`'s change is in. Nothing here can
    /// panic (ADR 0053).
    pub(super) fn commit_views(&mut self, id: BufferId, moved: MovedViews) {
        let revision = self.buffer(id).map_or(0, |buffer| buffer.revision());
        let tiled = self.tiled_count();
        for (index, wrap, scroll) in moved {
            if let Some(window) = self.windows.get_mut(index) {
                window.follow_change(wrap, scroll, revision);
                // Another tiled window follows its cursor when the command ends (ADR 0112).
                let id = window.id();
                if index < tiled && index != self.selected && !self.moved_windows.contains(&id) {
                    self.moved_windows.push(id);
                }
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

    /// Takes a job's report into the window whose wrap made it, if it still shows that wrap.
    /// The rows in view may break otherwise, but the text at the top stays in the first row
    /// (ADR 0100). Returns whether a frame is needed.
    pub fn take_wrap_report(&mut self, report: WrapReport) -> bool {
        let found = (0..self.tiled_count())
            .find(|&index| (self.windows[index].wrap_map()).is_some_and(|wrap| wrap.owns(&report)));
        let Some(index) = found else {
            return false;
        };
        self.refill_wraps();
        // A wrap that missed a change starts over, and drops the report.
        self.sync_wrap(index);
        let (window, buffer) = self.window_mut(index);
        let display = buffer.display();
        let taken = window
            .wrap_map_mut()
            .is_some_and(|wrap| wrap.take_report(report, &display));
        if taken {
            window.realize_view(&display);
            self.follow_window(index);
        }
        taken
    }

    /// The preview's wrap of `id` at `revision`, `columns` wide, if it has that one.
    pub(super) fn preview_wrap_of(
        &self,
        id: BufferId,
        revision: u64,
        columns: usize,
    ) -> Option<&WrapMap> {
        self.preview_wrap
            .as_ref()
            .filter(|kept| kept.id == id && kept.revision == revision && kept.columns == columns)
            .map(|kept| &kept.wrap)
    }

    /// Brings the preview's wrap in step with what it previews, after each command: made anew
    /// for another buffer, width or text, dropped for a buffer that does not wrap, and wrapped
    /// near the preview's start within the command's budget.
    pub(super) fn fit_preview(&mut self) {
        let previewed = match self.preview() {
            Some(Preview::Buffer { id, start, .. }) => Some((id, start)),
            _ => None,
        };
        let region = self.prompt_layout().map(|layout| layout.regions.preview);
        let wanted = previewed
            .zip(region)
            .filter(|(_, region)| region.rows > 0 && region.columns > 0)
            .and_then(|((id, start), region)| {
                let buffer = self.buffer(id).filter(|buffer| buffer.wraps())?;
                Some((id, start, region, buffer.display(), buffer.revision()))
            });
        let Some((id, start, region, display, revision)) = wanted else {
            self.preview_wrap = None;
            return;
        };
        let kept = self.preview_wrap.take().filter(|kept| {
            kept.id == id && kept.revision == revision && kept.columns == region.columns
        });
        let mut kept = kept.unwrap_or_else(|| PreviewWrap {
            id,
            revision,
            columns: region.columns,
            wrap: WrapMap::provisional(&display, region.columns),
        });
        kept.wrap.refill();
        kept.wrap.realize(&display, start, region.rows);
        self.preview_wrap = Some(kept);
    }
}
