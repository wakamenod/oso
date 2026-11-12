//! Drawing: the window and its renderer, and what a frame shows: every tiled window, the echo
//! area and a prompt's float (ADR 0112).
//!
//! Must not depend on: key presses or commands. Before a frame it takes in what
//! has loaded and fits the gutter to it, as a load is no input; otherwise a frame only
//! reads the editor's state.
//!
//! Whatever changes the modeline or the echo area gets a frame in one place
//! ([`App::ask_for_a_frame_if_status_changed`]), so a failed command, a message from a
//! save and a load's progress need no request of their own (ADR 0063).

use std::time::Instant;

use oso_core::editor::{Editor, MatchCells, Status};
use oso_core::window::{Screen, WindowId};
use oso_render::{Bands, Face, Layer, Pane, PaneContent, PaneId, Redraw, RenderState, Span};

use super::App;
use super::shell::{Loop, Window};
use crate::text;

mod float;
mod tiled;

pub(super) use tiled::statuses_into;

/// What a frame draws besides the editor's state, kept from frame to frame so that it is not
/// allocated again: each tiled window's cells (ADR 0112), the rows of the innermost prompt's
/// answer and of the buffer it previews (ADR 0088), and the cursor flash, which the app sets
/// before a frame.
#[derive(Default)]
pub(super) struct Cells {
    windows: Vec<tiled::WindowCells>,
    /// The cursor flash's strength in this frame, from the app's clock (ADR 0097).
    pub(super) flash: Option<f32>,
    answer: Screen,
    preview: Screen,
    /// The matches a project search's preview shows (ADR 0114).
    preview_matches: MatchCells,
    /// Room for the spans and rows each frame lists, empty between frames.
    room: tiled::Room,
    panes: Vec<Pane<'static>>,
}

/// `room`'s allocation, emptied, for elements that differ from its own only in their
/// lifetimes. Collecting a `Vec`'s own iterator reuses its buffer when the elements have the
/// same size and alignment, as these do.
fn reuse<T, U>(mut room: Vec<T>) -> Vec<U> {
    room.clear();
    room.into_iter().filter_map(|_| None).collect()
}

/// The echo area's pane, apart from every window's and float's.
const ECHO: PaneId = PaneId(2);

/// Calls `draw` with what the frame shows, for the renderer: every tiled window's panes, in
/// the tree's order, with their `statuses` ([`statuses_into`]), the echo area, and a prompt's
/// float over them. What they fill goes in `cells`, which a frame reuses.
pub(super) fn with_panes<R>(
    editor: &Editor,
    statuses: &[(WindowId, Status)],
    cells: &mut Cells,
    draw: impl FnOnce(&[Pane<'_>]) -> R,
) -> R {
    tiled::fill(editor, &mut cells.windows);
    editor.prompt_screen(&mut cells.answer);
    editor.preview_screen(&mut cells.preview);
    editor.preview_cells(&cells.preview, &mut cells.preview_matches);
    let empty = Status::default();
    let status = tiled::tiled_status(editor, statuses).unwrap_or(&empty);
    let echo = [
        Span {
            text: status.echo_warnings(),
            face: Face::Warning,
        },
        Span {
            text: &status.echo[..],
            face: Face::Text,
        },
    ];
    let echo = [&echo[..]];
    let mut room = std::mem::take(&mut cells.room);
    let windows = editor.frame().windows();
    let spans = tiled::spans_of(windows, statuses, &cells.windows, &empty, &mut room);
    let rows = tiled::rows_of(&spans.0, &spans.1, &mut room);
    let drawn = tiled::Drawn {
        cells: &cells.windows,
        fills: &spans.2,
        labels: &spans.3,
        modelines: &rows.0,
        gutters: &rows.1,
        statuses,
        flash: cells.flash,
    };
    // A prompt's float goes over the tiled panes.
    let float = float::Parts::of(editor, &cells.answer);
    let float_spans = float.as_ref().map(float::Parts::spans);
    let lines = float_spans.as_ref().map(|spans| spans.lines());
    let mut list: Vec<Pane<'_>> = reuse(std::mem::take(&mut cells.panes));
    tiled::panes(editor, &drawn, &mut list);
    // The echo area has no background of its own, so that nothing marks the pixels left
    // over below it (ADR 0057). It comes after the windows, so that no amount of text can
    // take its place (ADR 0061).
    list.push(Pane {
        id: ECHO,
        region: editor.frame().echo,
        layer: Layer::TILED,
        background: None,
        panel: None,
        bands: Bands::NONE,
        content: PaneContent::Lines(&echo),
    });
    let preview_fills = [
        (Face::SearchCurrent, &cells.preview_matches.current[..]),
        (Face::SearchMatch, &cells.preview_matches.others[..]),
    ];
    if let (Some(parts), Some(lines)) = (&float, &lines) {
        let screens = float::Screens {
            answer: &cells.answer,
            preview: &cells.preview,
            preview_fills: &preview_fills,
        };
        float::panes(editor, (parts, screens), lines, &mut list);
    }
    let result = draw(&list);
    cells.panes = reuse(list);
    tiled::give_back_rows(&mut room, rows);
    tiled::give_back_spans(&mut room, spans);
    cells.room = room;
    result
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
            // A read done by the first frame applies before it (ADR 0108).
            self.configuration_reported();
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
        statuses_into(&self.editor, &mut self.next_statuses);
        if self.next_statuses != self.statuses {
            std::mem::swap(&mut self.statuses, &mut self.next_statuses);
            self.request_redraw();
        }
    }

    /// Draws a frame of what the editor shows, with what has loaded by now.
    pub(super) fn draw_frame(&mut self) {
        let frame = Instant::now();
        self.poll_loads();
        // A load, a reload or a key may have given the line count another digit (ADR 0097).
        self.editor.fit_gutter();
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
        statuses_into(&self.editor, &mut self.statuses);
        #[cfg(feature = "dev")]
        let presented = view.render_state.presented_frames();
        #[cfg(feature = "dev")]
        let rendering = Instant::now();
        let render_state = &mut view.render_state;
        let cells = &mut view.cells;
        cells.flash = self.flash.strength(Instant::now());
        let result = with_panes(&self.editor, &self.statuses, cells, |panes| {
            render_state.render(panes, &window_size)
        });
        // Recovering from a surface error can configure it to a new size before
        // `Resized` says so, and panes stop at their regions, so the editor catches
        // up now.
        let size = view.render_state.frame_size();
        if size != self.editor.frame().size() {
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

    fn statuses(editor: &Editor) -> Vec<(WindowId, Status)> {
        let mut statuses = Vec::new();
        statuses_into(editor, &mut statuses);
        statuses
    }

    /// The tiled window's gutter pane.
    fn gutter_id(editor: &Editor) -> PaneId {
        tiled::pane_id(editor.tiled().0.id(), tiled::Part::Gutter)
    }

    /// A pane's region, its background, and its one row as spans of text and face.
    type Shown = (frame::CellRect, Option<Face>, Vec<(String, Face)>);

    /// What each pane but the gutter shows of an editor on bidi text, with a message in the
    /// echo area.
    fn panes_of(rows: usize) -> Vec<Shown> {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "\u{202e}"));
        editor.set_frame_size(FrameSize { rows, columns: 80 });
        editor.message("Saved");
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                panes
                    .iter()
                    .filter(|pane| pane.id != gutter_id(&editor))
                    .map(|pane| {
                        let spans = match &pane.content {
                            PaneContent::Text(_)
                            | PaneContent::Masked(_)
                            | PaneContent::Divider => Vec::new(),
                            PaneContent::Lines(lines) => lines[0]
                                .iter()
                                .map(|span| (span.text.to_owned(), span.face))
                                .collect(),
                        };
                        (pane.region, pane.background, spans)
                    })
                    .collect()
            },
        )
    }

    #[test]
    fn panes_are_the_frames_regions_with_their_own_ids() {
        let mut editor = Editor::default();
        let size = FrameSize {
            rows: 24,
            columns: 80,
        };
        editor.set_frame_size(size);
        let layout = editor.frame().clone();
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                let regions: Vec<_> = panes.iter().map(|pane| pane.region).collect();
                let window = editor.tiled_regions();
                assert_eq!(layout.windows(), [(editor.window().id(), window)]);
                assert_eq!(
                    regions,
                    [window.text, window.gutter, window.modeline, layout.echo]
                );
                let ids: Vec<_> = panes.iter().map(|pane| pane.id).collect();
                // The first window's text, gutter and modeline, then the echo area.
                assert_eq!(ids, [PaneId(16), PaneId(17), PaneId(18), ECHO]);
                assert!(panes.iter().all(|pane| pane.layer == Layer::TILED));
            },
        );
    }

    /// Each pane's id, region, background, and for a text the cursor's style, for a split
    /// frame.
    fn split_panes(editor: &Editor) -> Vec<(PaneId, frame::CellRect, Option<Face>, String)> {
        with_panes(editor, &statuses(editor), &mut Cells::default(), |panes| {
            panes
                .iter()
                .map(|pane| {
                    let what = match &pane.content {
                        PaneContent::Text(view) => format!("{:?}", view.cursor_style),
                        PaneContent::Lines(lines) => lines
                            .first()
                            .map(|row| {
                                row.iter()
                                    .map(|span| format!("{}{:?}", span.text, span.face))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        PaneContent::Divider => "divider".to_owned(),
                        PaneContent::Masked(_) => String::new(),
                    };
                    (pane.id, pane.region, pane.background, what)
                })
                .collect()
        })
    }

    #[test]
    fn a_split_frame_draws_each_window_with_its_own_ids_and_the_selected_one_active() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "text\n"));
        editor.set_frame_size(FrameSize {
            rows: 10,
            columns: 41,
        });
        editor.set_agent_channel(AgentChannel::On);
        let first = editor.window().id();
        let second = editor
            .split_window(oso_core::frame::Way::Across, false)
            .unwrap();
        let panes = split_panes(&editor);
        let [left, right] = [first, second].map(|id| *editor.regions(id).unwrap());
        let ids: Vec<_> = panes.iter().map(|pane| pane.0.0).collect();
        // The texts, the selected one's last, then the gutters, the modelines, the divider and
        // the echo area.
        assert_eq!(ids, [20, 16, 17, 21, 18, 22, 19, ECHO.0]);
        assert_eq!(panes[0].1, right.text);
        assert_eq!(panes[0].3, "Hollow", "the other");
        assert_eq!(panes[1].1, left.text);
        assert_eq!(panes[1].3, "Box", "the selected window");
        assert_eq!(panes[4].2, Some(Face::Modeline));
        assert!(panes[4].3.starts_with("Agent  Prompt"), "{}", panes[4].3);
        assert_eq!(panes[5].2, Some(Face::ModelineInactive));
        assert!(panes[5].3.starts_with("Prompt"), "no Agent: {}", panes[5].3);
        assert!(
            panes[5].3.contains("a.txt  ModelineInactive"),
            "{}",
            panes[5].3
        );
        assert_eq!(
            (panes[6].1, panes[6].3.as_str()),
            (left.divider.unwrap(), "divider")
        );
        assert!(right.divider.is_none());

        // Deleted, the first gives its cells to the second, which keeps its ids.
        editor.delete_window().unwrap();
        let ids: Vec<_> = split_panes(&editor).iter().map(|pane| pane.0.0).collect();
        assert_eq!(ids, [20, 21, 22, ECHO.0]);
    }

    #[test]
    fn beneath_a_prompt_no_window_is_drawn_as_selected() {
        let mut editor = Editor::default();
        editor.set_frame_size(FrameSize {
            rows: 10,
            columns: 40,
        });
        editor
            .split_window(oso_core::frame::Way::Down, false)
            .unwrap();
        editor.ask("Exit?", "keyboard-quit").unwrap();
        let panes = split_panes(&editor);
        let tiled: Vec<_> = panes.iter().filter(|pane| pane.0.0 >= 16).collect();
        // The window the prompt came from, the first, has its text last.
        assert_eq!((tiled[0].0.0, tiled[1].0.0), (20, 16));
        assert_eq!(tiled[0].3, "Hollow");
        assert_eq!(tiled[1].3, "Hollow");
        assert_eq!(tiled[4].2, Some(Face::ModelineInactive));
        assert_eq!(tiled[5].2, Some(Face::ModelineInactive));
    }

    #[test]
    fn the_gutters_room_is_kept_from_frame_to_frame() {
        let room: Vec<&'static str> = Vec::with_capacity(64);
        let pointer = room.as_ptr().cast::<u8>();
        let kept: Vec<&str> = reuse(room);
        assert_eq!(kept.capacity(), 64);
        assert_eq!(kept.as_ptr().cast::<u8>(), pointer, "the same buffer");
    }

    #[test]
    fn the_gutter_shows_the_numbers_in_view_the_current_brighter() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", "a\nb\n"));
        editor.set_frame_size(FrameSize {
            rows: 6,
            columns: 20,
        });
        editor.set_visual_aids(oso_core::editor::VisualAids {
            line_numbers: true,
            ..oso_core::editor::VisualAids::default()
        });
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                let gutter = panes
                    .iter()
                    .find(|pane| pane.id == gutter_id(&editor))
                    .unwrap();
                assert_eq!(gutter.region.columns, 4);
                let PaneContent::Lines(lines) = &gutter.content else {
                    panic!("the gutter has lines");
                };
                let shown: Vec<_> = lines.iter().map(|row| (row[0].text, row[0].face)).collect();
                assert_eq!(
                    shown,
                    [
                        ("  1 ", Face::LineNumberCurrent),
                        ("  2 ", Face::LineNumber),
                        ("  3 ", Face::LineNumber),
                    ]
                );
            },
        );
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
            with_panes(
                editor,
                &statuses(editor),
                &mut Cells::default(),
                |panes| match &panes[2].content {
                    PaneContent::Lines(lines) => (lines[0][0].text.to_owned(), lines[0][0].face),
                    _ => panic!("a modeline has lines"),
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
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
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
            },
        );
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
                with_panes(
                    &editor,
                    &statuses(&editor),
                    &mut Cells::default(),
                    |panes| {
                        let text = editor.tiled_regions().text;
                        for pane in panes
                            .iter()
                            .filter(|pane| pane.layer == Layer::FLOAT_CONTENT)
                        {
                            let region = pane.region;
                            assert!(region.row + region.rows <= text.row + text.rows, "{rows}");
                            // Below the windows' ids, which start at 16.
                            assert!(pane.id.0 < 16, "{:?}", pane.id);
                        }
                    },
                );
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
            with_panes(
                &editor,
                &statuses(&editor),
                &mut Cells::default(),
                |panes| {
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
                },
            );
        }
    }
}
