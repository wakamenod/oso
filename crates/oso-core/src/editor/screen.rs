//! The rows a frame shows (ADR 0100, ADR 0104): the tiled window's, with a preedit laid out in
//! its line, and the preview's of the buffer a prompt previews, wrapped at the preview's width
//! while that buffer wraps.
//!
//! Must not depend on: `command`, `keymap` or the host's window.
//!
//! The preview's buffer is shown by no window, so its wrap is kept here: provisional, one row a
//! logical line, made in O(log n) when the preview changes buffer, width or text, and wrapped
//! near the preview's start within each command's budget. No job wraps the rest, as nothing
//! scrolls a preview.

use super::{Editor, Preview};
use crate::buffer::BufferId;
use crate::display_map::wrap_map::WrapMap;
use crate::frame::CellRect;
use crate::window::{Rows, Screen, Scroll, screen};

/// The preview's rows of the buffer it shows.
pub(super) struct PreviewWrap {
    id: BufferId,
    /// The buffer's revision and the preview's width the rows were made for.
    revision: u64,
    columns: usize,
    wrap: WrapMap,
}

impl Editor {
    /// The rows a frame shows of the tiled window, also beneath a prompt, into `out`: with the
    /// preedit laid out in its line while no prompt is open, as only then does it show there
    /// (ADR 0069, ADR 0072).
    pub fn screen(&self, out: &mut Screen) {
        let (window, buffer) = self.tiled();
        let preedit = self.preedit.as_ref().filter(|_| self.prompts.is_empty());
        window.screen_into(&buffer.display(), preedit, out);
    }

    /// The rows a frame shows of the innermost prompt's answer, its window's, with the preedit
    /// at its cursor, into `out`; none while no prompt is open (ADR 0072). An answer never
    /// wraps.
    pub fn prompt_screen(&self, out: &mut Screen) {
        if self.prompts.is_empty() {
            out.clear();
            return;
        }
        let window = &self.windows[self.selected];
        let display = self.shown(window).display();
        window.screen_into(&display, self.preedit.as_ref(), out);
    }

    /// The rows of the selected window as a frame shows them, and the area they are in: the
    /// innermost prompt's answer while one is open, and the tiled window's text area
    /// otherwise.
    pub(super) fn selected_screen(&self) -> (Screen, CellRect) {
        let mut screen = Screen::default();
        match self.prompt_layout() {
            Some(layout) => {
                self.prompt_screen(&mut screen);
                (screen, layout.input)
            }
            None => {
                self.screen(&mut screen);
                (screen, self.frame.window.text)
            }
        }
    }

    /// The tiled window's rows as [`Editor::screen`] gives them, for tests.
    #[cfg(test)]
    pub(crate) fn screen_now(&self) -> Screen {
        let mut screen = Screen::default();
        self.screen(&mut screen);
        screen
    }

    /// The rows a frame shows of the buffer the innermost prompt previews, in the preview's
    /// region, into `out`; none while no buffer is previewed (ADR 0088).
    pub fn preview_screen(&self, out: &mut Screen) {
        let previewed = match self.preview() {
            Some(Preview::Buffer { id, start, cursor }) => Some((id, start, cursor)),
            _ => None,
        };
        let region = self.prompt_layout().map(|layout| layout.regions.preview);
        let (Some((id, start, cursor)), Some(region)) = (previewed, region) else {
            out.clear();
            return;
        };
        let Some(buffer) = self.buffer(id) else {
            out.clear();
            return;
        };
        let display = buffer.display();
        let wrap = self
            .preview_wrap
            .as_ref()
            .filter(|kept| {
                kept.id == id
                    && kept.revision == buffer.revision()
                    && kept.columns == region.columns
            })
            .map(|kept| &kept.wrap);
        let view = screen::View {
            rows: Rows::new(&display, wrap),
            scroll: Scroll { start, left: 0 },
            size: region.view_size(),
            cursor,
        };
        screen::fill(view, &display, None, out);
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

#[cfg(test)]
mod tests;
