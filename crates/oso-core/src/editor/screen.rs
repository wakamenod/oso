//! The rows a frame shows (ADR 0100, ADR 0104): the tiled window's, with a preedit laid out in
//! its line, and the preview's of the buffer a prompt previews, wrapped at the preview's width
//! while that buffer wraps.
//!
//! Must not depend on: `command`, `keymap` or the host's window.
//!
//! The preview's buffer is shown by no window, so the editor keeps its wrap (`wraps.rs`).

use super::{Editor, Preview};
use crate::frame::CellRect;
use crate::window::{Rows, Screen, ScreenSource, Scroll};

impl Editor {
    /// The rows a frame shows of the tiled window, also beneath a prompt, into `out`: with the
    /// preedit laid out in its line while no prompt is open, as only then does it show there
    /// (ADR 0069, ADR 0072).
    pub fn screen(&self, out: &mut Screen) {
        let (window, buffer) = self.tiled();
        let preedit = self.preedit.as_ref().filter(|_| self.prompts.is_empty());
        window.screen_into(&buffer.display(), preedit, out);
    }

    /// The tiled window's rows as its buffer's text alone lays them out, with no preedit, for
    /// what reads the text rather than draws it, as the dev socket does, which shows nothing of
    /// a composition.
    pub fn text_screen(&self, out: &mut Screen) {
        let (window, buffer) = self.tiled();
        window.screen_into(&buffer.display(), None, out);
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

    /// The area the selected window is drawn in: the innermost prompt's answer while one is
    /// open, and the tiled window's text area otherwise.
    pub(super) fn selected_region(&self) -> CellRect {
        match self.prompt_layout() {
            Some(layout) => layout.input,
            None => self.tiled_regions().text,
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
        let revision = buffer.revision();
        let wrap = self.preview_wrap_of(id, revision, region.columns);
        let source = ScreenSource {
            rows: Rows::new(&display, wrap.map(|wrap| (wrap, revision))),
            scroll: Scroll { start, left: 0 },
            size: region.view_size(),
            cursor,
        };
        out.fill(source, &display, None);
    }
}

#[cfg(test)]
mod tests;
