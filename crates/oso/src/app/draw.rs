//! Drawing: the window and its renderer, and what a frame shows.
//!
//! Must not depend on: key presses or commands. Before a frame it takes in what
//! has loaded; otherwise a frame only reads the editor's state.
//!
//! Whatever changes the modeline or the echo area gets a frame in one place
//! ([`App::ask_for_a_frame_if_status_changed`]), so a failed command, a message from a
//! save and a load's progress need no request of their own (ADR 0063).

use std::time::Instant;

use oso_core::editor::{Editor, Status};
use oso_core::frame;
use oso_render::{Face, Layer, Pane, PaneContent, PaneId, Redraw, RenderState, Span, TextView};

use super::App;
use super::shell::{Loop, Window};
use crate::text;

/// Calls `draw` with what the frame shows, for the renderer. The cells of the regions
/// go in `region_cells`, which a frame reuses.
pub(super) fn with_panes<R>(
    editor: &Editor,
    status: &Status,
    region_cells: &mut Vec<frame::CellRect>,
    draw: impl FnOnce(&[Pane<'_>]) -> R,
) -> R {
    let span = |text, face| Span { text, face };
    // Warnings first, so that nothing can push them off the row. A name and the rest are
    // shaped apart, so that neither changes how the other reads.
    let modeline = [
        span(&status.warnings[..], Face::Warning),
        span(&status.name[..], Face::Modeline),
        span(&status.state[..], Face::Modeline),
    ];
    let echo = [
        span(status.echo_warnings(), Face::Warning),
        span(&status.echo[..], Face::Text),
    ];
    editor.region_cells(region_cells);
    draw(&panes(editor, region_cells, &[&modeline], &[&echo]))
}

/// The selected window, then the rows reserved below it. The later panes come first if
/// a frame runs out of room, so no amount of text can take their place (ADR 0061).
fn panes<'a>(
    editor: &'a Editor,
    region_cells: &'a [frame::CellRect],
    modeline: &'a [&'a [Span<'a>]],
    echo: &'a [&'a [Span<'a>]],
) -> [Pane<'a>; 3] {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    let frame = editor.frame();
    let text = PaneContent::Text(TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        scroll: window.scroll(),
        cursor,
        preedit: editor.preedit(),
        region_cells,
    });
    let pane = |id, region, background, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::TILED,
        background,
        content,
    };
    [
        pane(0, frame.window.text, None, text),
        pane(
            1,
            frame.window.modeline,
            Some(Face::Modeline),
            PaneContent::Lines(modeline),
        ),
        // The echo area has no background of its own, so that nothing marks the
        // pixels left over below it (ADR 0057).
        pane(2, frame.echo, None, PaneContent::Lines(echo)),
    ]
}

/// The window and its renderer. The tip crate owns the window; `oso-render`
/// only draws into it and asks for redraws through [`Redraw`].
pub(super) struct View {
    pub(super) window: Window,
    pub(super) render_state: RenderState,
    /// When the window was requested, until the first frame is presented.
    started: Option<Instant>,
    /// The cells of the regions, kept from frame to frame so they are not allocated
    /// again.
    region_cells: Vec<frame::CellRect>,
}

impl View {
    /// Opens the window and sets up its renderer.
    pub(super) fn open(event_loop: &Loop<'_>) -> Self {
        let started = Instant::now();
        let window = event_loop.open_window();
        let (width, height) = window.inner_size();
        let text_system = text::load(window.scale_factor());
        let render_state = window.start_renderer(width, height, text_system);
        // Under `ControlFlow::Wait` nothing is drawn until requested, and
        // not every platform sends an initial resize or redraw.
        window.request_redraw();
        Self {
            window,
            render_state,
            started: Some(started),
            region_cells: Vec::new(),
        }
    }

    /// Requests a redraw if the renderer asked for one.
    pub(super) fn redraw_if(&self, redraw: Redraw) {
        match redraw {
            Redraw::Needed => self.window.request_redraw(),
            Redraw::WhenVisible => self.window.redraw_when_visible(),
            Redraw::NotNeeded => {}
        }
    }
}

impl App {
    /// Opens the window, unless it is open already.
    pub(super) fn open_window(&mut self, event_loop: &Loop<'_>) {
        if self.view.is_none() {
            if let Some(started) = self.started {
                log::debug!("resumed {:?} after start", started.elapsed());
            }
            self.view = Some(View::open(event_loop));
            // At once: it costs under 1 µs and does not slow the first frame.
            self.sync_ime();
            self.update_frame_size();
            // Content that loaded while the window opened shows in the
            // first frame.
            self.poll_loads();
        }
    }

    /// Gives the window's new size to the renderer and the editor.
    pub(super) fn resize(&mut self, width: u32, height: u32) {
        if let Some(view) = &mut self.view {
            let redraw = view.render_state.resize(width, height);
            view.redraw_if(redraw);
            self.update_frame_size();
        }
    }

    /// Reloads the font for a display with another scale. The surface still has
    /// the old display's pixels, so cells counted now would give a frame of the
    /// wrong size for one event, and a taller one moves the view. The editor learns
    /// the size from the `Resized` that follows, or from `draw_frame` if none does.
    pub(super) fn set_scale(&mut self, scale_factor: f64) {
        if let Some(view) = &mut self.view {
            let redraw = view.render_state.set_text_system(text::load(scale_factor));
            view.redraw_if(redraw);
        }
    }

    /// Tells the editor how many whole cells the window holds.
    fn update_frame_size(&mut self) {
        if let Some(view) = &self.view {
            self.editor.set_frame_size(view.render_state.frame_size());
        }
    }

    /// Asks for a frame if the modeline or the echo area would read differently from
    /// the last frame drawn or asked for. Comparing the text means no caller has anything
    /// to get wrong, and a load's progress asks only when its number changes. What was
    /// asked for counts as drawn, so a window that draws nothing is not asked again.
    pub(super) fn ask_for_a_frame_if_status_changed(&mut self) {
        if self.view.is_none() {
            return;
        }
        self.editor.status_into(&mut self.next_status);
        if self.next_status != self.status {
            std::mem::swap(&mut self.status, &mut self.next_status);
            self.request_redraw();
        }
    }

    /// Draws a frame of what the editor shows, with what has loaded by now.
    pub(super) fn draw_frame(&mut self) {
        let frame = Instant::now();
        self.poll_loads();
        let Some(view) = self.view.as_mut() else {
            return;
        };
        let window = &view.window;
        let window_size = || window.inner_size();
        self.editor.status_into(&mut self.status);
        #[cfg(feature = "dev")]
        let presented = view.render_state.presented_frames();
        let render_state = &mut view.render_state;
        let cells = &mut view.region_cells;
        let result = with_panes(&self.editor, &self.status, cells, |panes| {
            render_state.render(panes, &window_size)
        });
        // Recovering from a surface error can configure it to a new size before
        // `Resized` says so, and panes stop at their regions, so the editor catches
        // up now.
        let size = view.render_state.frame_size();
        if frame::layout(size) != self.editor.frame() {
            self.editor.set_frame_size(size);
            view.window.request_redraw();
        }
        match result {
            Ok(redraw) => {
                view.redraw_if(redraw);
                #[cfg(feature = "dev")]
                if view.render_state.presented_frames() > presented {
                    let complete = redraw == Redraw::NotNeeded;
                    let withheld = self.editor.shows_secret();
                    self.timings.frame_presented(complete, withheld);
                }
                // A frame skipped while the window is hidden draws
                // nothing yet.
                if view.render_state.presented_frames() == 0 {
                    return;
                }
                if let Some(started) = view.started.take() {
                    log::debug!("first frame {:?} after the window", started.elapsed());
                }
                if let Some(started) = self.started.take() {
                    log::debug!("first frame {:?} after start", started.elapsed());
                }
                log::trace!("frame drawn in {:?}", frame.elapsed());
                self.note_first_screens();
            }
            // Not fatal: the frame is skipped and the next one retries.
            Err(e) => log::error!("render error: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::buffer::Buffer;
    use oso_core::frame::{self, FrameSize};

    /// A pane's region, its background, and its one row as spans of text and face.
    type Shown = (frame::CellRect, Option<Face>, Vec<(String, Face)>);

    /// What each pane shows of an editor on bidi text, with a message in the echo area.
    fn panes_of(rows: usize) -> Vec<Shown> {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "\u{202e}"));
        editor.set_frame_size(FrameSize { rows, columns: 80 });
        editor.message("Saved");
        let mut status = Status::default();
        editor.status_into(&mut status);
        with_panes(&editor, &status, &mut Vec::new(), |panes| {
            panes
                .iter()
                .map(|pane| {
                    let spans = match &pane.content {
                        PaneContent::Text(_) => Vec::new(),
                        PaneContent::Lines(lines) => lines[0]
                            .iter()
                            .map(|span| (span.text.to_owned(), span.face))
                            .collect(),
                    };
                    (pane.region, pane.background, spans)
                })
                .collect()
        })
    }

    #[test]
    fn panes_are_the_frames_regions_with_their_own_ids() {
        let mut editor = Editor::default();
        let size = FrameSize {
            rows: 24,
            columns: 80,
        };
        editor.set_frame_size(size);
        let layout = frame::layout(size);
        let status = Status::default();
        with_panes(&editor, &status, &mut Vec::new(), |panes| {
            let regions: Vec<_> = panes.iter().map(|pane| pane.region).collect();
            assert_eq!(
                regions,
                [layout.window.text, layout.window.modeline, layout.echo]
            );
            let ids: Vec<_> = panes.iter().map(|pane| pane.id).collect();
            assert_eq!(ids, [PaneId(0), PaneId(1), PaneId(2)]);
            assert!(panes.iter().all(|pane| pane.layer == Layer::TILED));
        });
    }

    #[test]
    fn the_modeline_has_its_background_and_its_warnings_first() {
        let panes = panes_of(24);
        assert_eq!(panes[1].1, Some(Face::Modeline));
        assert_eq!(panes[2].1, None, "the echo area has none (ADR 0057)");
        let modeline = &panes[1].2;
        assert_eq!(modeline[0], ("[bidi controls] ".to_owned(), Face::Warning));
        assert_eq!(modeline[1], ("-- a.txt  ".to_owned(), Face::Modeline));
        assert_eq!(modeline[2].1, Face::Modeline);
        let echo = &panes[2].2;
        assert_eq!(echo[0], (String::new(), Face::Warning), "not said twice");
        assert_eq!(echo[1], ("Saved".to_owned(), Face::Text));
    }

    #[test]
    fn a_frame_with_no_modeline_has_its_warnings_first_on_the_echo_row() {
        let panes = panes_of(2);
        assert_eq!(panes[1].0.rows, 0);
        let echo = &panes[2].2;
        assert_eq!(echo[0], ("[bidi controls] ".to_owned(), Face::Warning));
        assert_eq!(echo[1], ("Saved".to_owned(), Face::Text));
    }

    #[test]
    fn a_frame_of_one_row_has_only_the_text() {
        let panes = panes_of(1);
        assert_eq!(panes[0].0.rows, 1);
        assert_eq!((panes[1].0.rows, panes[2].0.rows), (0, 0));
    }
}
