//! Drawing: the window and its renderer, and what a frame shows.
//!
//! Must not depend on: key presses or commands. Before a frame it takes in what
//! has loaded; otherwise a frame only reads the editor's state.
//!
//! Whatever changes the modeline or the echo area gets a frame in one place
//! ([`App::ask_for_a_frame_if_status_changed`]), so a failed command, a message from a
//! save and a load's progress need no request of their own (ADR 0063).

use std::time::Instant;

use oso_core::editor::{Editor, MatchCells, Status};
use oso_core::frame;
use oso_render::{
    Bands, CursorStyle, Face, Layer, Pane, PaneContent, PaneId, Redraw, RenderState, Span, TextView,
};

use super::App;
use super::shell::{Loop, Window};
use crate::text;

mod float;

/// The cells a frame fills behind the text, kept from frame to frame so they are not
/// allocated again: the regions' (ADR 0071) and isearch's matches' (ADR 0082).
#[derive(Default)]
pub(super) struct Cells {
    region: Vec<frame::CellRect>,
    matches: MatchCells,
}

/// Calls `draw` with what the frame shows, for the renderer. The cells behind the text go
/// in `cells`, which a frame reuses.
pub(super) fn with_panes<R>(
    editor: &Editor,
    status: &Status,
    cells: &mut Cells,
    draw: impl FnOnce(&[Pane<'_>]) -> R,
) -> R {
    let span = |text, face| Span { text, face };
    // Warnings first, so that nothing can push them off the row, after `Agent` while the
    // agent channel is on: dim, and bright while an agent is connected (ADR 0085). A name and
    // the rest are shaped apart, so that neither changes how the other reads.
    let agent = if status.agent_connected {
        Face::Warning
    } else {
        Face::Prompt
    };
    let modeline = [
        span(status.agent, agent),
        span(&status.warnings[..], Face::Warning),
        span(&status.name[..], Face::Modeline),
        span(&status.state[..], Face::Modeline),
    ];
    let echo = [
        span(status.echo_warnings(), Face::Warning),
        span(&status.echo[..], Face::Text),
    ];
    editor.region_cells(&mut cells.region);
    editor.isearch_cells(&mut cells.matches);
    // The current match first, so that it always has room, and the regions over the matches.
    let fills = [
        (Face::SearchCurrent, &cells.matches.current[..]),
        (Face::SearchMatch, &cells.matches.others[..]),
        (Face::Region, &cells.region[..]),
    ];
    let (modeline, echo) = ([&modeline[..]], [&echo[..]]);
    let tiled = panes(editor, &fills, &modeline, &echo);
    let Some(parts) = float::Parts::of(editor) else {
        return draw(&tiled);
    };
    // A prompt's float goes over the tiled panes.
    let spans = parts.spans();
    let lines = spans.lines();
    let mut list = Vec::with_capacity(tiled.len() + float::MAX_PANES);
    list.extend(tiled);
    float::panes(editor, &parts, &lines, &mut list);
    draw(&list)
}

/// The tiled window, then the rows reserved below it. The later panes come first if
/// a frame runs out of room, so no amount of text can take their place (ADR 0061).
/// Beneath a prompt, the window's cursor is hollow and it shows no preedit, as it is
/// not selected (ADR 0072).
fn panes<'a>(
    editor: &'a Editor,
    fills: &'a [(Face, &'a [frame::CellRect])],
    modeline: &'a [&'a [Span<'a>]],
    echo: &'a [&'a [Span<'a>]],
) -> [Pane<'a>; 3] {
    let (window, buffer) = editor.tiled();
    let beneath = editor.prompt().is_some();
    let display = buffer.display();
    let cursor = window.cursor_point(&display);
    let frame = editor.frame();
    let text = PaneContent::Text(TextView {
        display,
        buffer: window.buffer(),
        revision: buffer.revision(),
        scroll: window.scroll(),
        cursor,
        cursor_style: if beneath {
            CursorStyle::Hollow
        } else {
            CursorStyle::Box
        },
        preedit: editor.preedit().filter(|_| !beneath),
        fills,
    });
    let pane = |id, region, background, content| Pane {
        id: PaneId(id),
        region,
        layer: Layer::TILED,
        background,
        panel: None,
        bands: Bands::NONE,
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
    /// The cells behind the text, kept from frame to frame.
    cells: Cells,
    /// The theme the renderer draws in, by name (ADR 0089).
    theme: Option<&'static str>,
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
            cells: Cells::default(),
            theme: None,
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
            // Before the first frame, so that it is drawn in the remembered theme.
            self.take_remembered_theme();
            // At once: it costs under 1 µs and does not slow the first frame.
            self.sync_ime(event_loop);
            self.sync_secure_input();
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
        // A switch of theme costs this frame only: nothing is shaped again (ADR 0089). The
        // editor's names are the renderer's, so it never refuses one.
        let wanted = self.editor.theme_name();
        if let Some(name) = wanted.filter(|_| wanted != view.theme) {
            match view.render_state.set_theme(name) {
                Some(_) => view.theme = Some(name),
                None => debug_assert!(false, "the renderer has no theme named {name}"),
            }
        }
        let window = &view.window;
        let window_size = || window.inner_size();
        self.editor.status_into(&mut self.status);
        #[cfg(feature = "dev")]
        let presented = view.render_state.presented_frames();
        #[cfg(feature = "dev")]
        let rendering = Instant::now();
        let render_state = &mut view.render_state;
        let cells = &mut view.cells;
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
                    let times = view.render_state.last_frame_times();
                    self.timings.frame_spans(crate::dev::FrameSpans {
                        started: frame,
                        rendering,
                        prepared: times.prepared,
                        acquired: times.acquired,
                        presented: times.presented,
                    });
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
    use oso_core::editor::AgentChannel;
    use oso_core::frame::{self, FrameSize};
    use oso_core::minibuffer::Kind;

    /// A pane's region, its background, and its one row as spans of text and face.
    type Shown = (frame::CellRect, Option<Face>, Vec<(String, Face)>);

    /// What each pane shows of an editor on bidi text, with a message in the echo area.
    fn panes_of(rows: usize) -> Vec<Shown> {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "\u{202e}"));
        editor.set_frame_size(FrameSize { rows, columns: 80 });
        editor.message("Saved");
        let mut status = Status::default();
        editor.status_into(&mut status);
        with_panes(&editor, &status, &mut Cells::default(), |panes| {
            panes
                .iter()
                .map(|pane| {
                    let spans = match &pane.content {
                        PaneContent::Text(_) | PaneContent::Masked(_) => Vec::new(),
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
        with_panes(&editor, &status, &mut Cells::default(), |panes| {
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
        // `Agent` comes first while the agent channel is on, and is empty here (ADR 0085).
        assert_eq!(modeline[0], (String::new(), Face::Prompt));
        assert_eq!(modeline[1], ("[bidi controls] ".to_owned(), Face::Warning));
        assert_eq!(modeline[2], ("-- a.txt  ".to_owned(), Face::Modeline));
        assert_eq!(modeline[3].1, Face::Modeline);
        let echo = &panes[2].2;
        assert_eq!(echo[0], (String::new(), Face::Warning), "not said twice");
        assert_eq!(echo[1], ("Saved".to_owned(), Face::Text));
    }

    #[test]
    fn the_agent_channel_shows_dim_and_bright_while_an_agent_is_connected() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", ""));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        let first = |editor: &Editor| {
            let mut status = Status::default();
            editor.status_into(&mut status);
            with_panes(
                editor,
                &status,
                &mut Cells::default(),
                |panes| match &panes[1].content {
                    PaneContent::Lines(lines) => (lines[0][0].text.to_owned(), lines[0][0].face),
                    PaneContent::Text(_) | PaneContent::Masked(_) => panic!("a modeline has lines"),
                },
            )
        };
        editor.set_agent_channel(AgentChannel::On);
        assert_eq!(first(&editor), ("Agent  ".to_owned(), Face::Prompt));
        editor.set_agent_channel(AgentChannel::Connected);
        assert_eq!(first(&editor), ("Agent  ".to_owned(), Face::Warning));
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

    #[test]
    fn a_float_too_short_for_its_label_keeps_the_keys_and_nothing_overlaps() {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize {
            rows: 4,
            columns: 40,
        });
        let long = "a question long enough to take three rows of a narrow float";
        editor.ask(long, "keyboard-quit").unwrap();
        let status = Status::default();
        with_panes(&editor, &status, &mut Cells::default(), |panes| {
            let inside: Vec<_> = panes
                .iter()
                .filter(|pane| pane.layer == Layer::FLOAT_CONTENT)
                .collect();
            for (i, a) in inside.iter().enumerate() {
                for b in &inside[..i] {
                    let rows = |p: &Pane<'_>| p.region.row..p.region.row + p.region.rows;
                    let columns =
                        |p: &Pane<'_>| p.region.column..p.region.column + p.region.columns;
                    let meet = |x: std::ops::Range<usize>, y: std::ops::Range<usize>| {
                        x.start < y.end && y.start < x.end
                    };
                    assert!(
                        !(meet(rows(a), rows(b)) && meet(columns(a), columns(b))),
                        "{:?} and {:?} overlap",
                        a.region,
                        b.region
                    );
                }
            }
            let shown: String = inside
                .iter()
                .filter_map(|pane| match &pane.content {
                    PaneContent::Lines(lines) => lines.first().copied(),
                    _ => None,
                })
                .flat_map(|row| row.iter().map(|span| span.text))
                .collect();
            assert!(shown.contains(" y ") && shown.contains(" n "), "{shown}");
        });
    }

    #[test]
    fn a_float_in_a_frame_of_few_rows_or_none_draws_what_fits() {
        for rows in 0..4 {
            for number in [false, true] {
                let mut editor = Editor::default();
                editor.set_frame_size(FrameSize { rows, columns: 60 });
                if number {
                    let kind = Kind::Number { history: None };
                    editor.open_prompt("Go to line: ", kind).unwrap();
                } else {
                    editor.ask("Exit?", "keyboard-quit").unwrap();
                }
                let status = Status::default();
                with_panes(&editor, &status, &mut Cells::default(), |panes| {
                    let text = editor.frame().window.text;
                    for pane in panes
                        .iter()
                        .filter(|pane| pane.layer == Layer::FLOAT_CONTENT)
                    {
                        let region = pane.region;
                        assert!(region.row + region.rows <= text.row + text.rows, "{rows}");
                    }
                });
            }
        }
    }

    #[test]
    fn panes_in_a_float_keep_bands_only_into_its_padding() {
        for rows in [3, 4, 5, 6, 24] {
            let mut editor = Editor::default();
            editor.set_frame_size(FrameSize { rows, columns: 60 });
            let long = "a question long enough to take two rows of the float, at least";
            editor.ask(long, "keyboard-quit").unwrap();
            let regions = editor.prompt_layout().unwrap().regions;
            let inner = regions.inner;
            // The rows of padding: the gap above the answer's row, and the row below it. Each
            // goes to the panes on the answer's row alone, never to the label's.
            let last = inner.row + inner.rows - 1;
            let padding = |row: usize, from: usize| {
                from == last
                    && ((regions.gap && row + 1 == last)
                        || (regions.padded_below() && row == last + 1))
            };
            let status = Status::default();
            with_panes(&editor, &status, &mut Cells::default(), |panes| {
                for pane in panes
                    .iter()
                    .filter(|pane| pane.layer == Layer::FLOAT_CONTENT)
                {
                    let region = pane.region;
                    let above = region
                        .row
                        .checked_sub(1)
                        .is_some_and(|row| padding(row, region.row));
                    let below = padding(region.row + region.rows, region.row);
                    assert_eq!(pane.bands.above, above, "{rows} rows: {region:?}");
                    assert_eq!(pane.bands.below, below, "{rows} rows: {region:?}");
                }
            });
        }
    }
}
