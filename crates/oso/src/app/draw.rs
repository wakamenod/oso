//! Drawing: the window and its renderer, and what a frame shows: every tiled window, the echo
//! area, a server's description at point (ADR 0138) and a prompt's float (ADR 0112).
//!
//! Must not depend on: key presses or commands. Before a frame it takes in what
//! has loaded and fits the gutter to it, as a load is no input, and sets `*Spaces*`' modeline
//! note for whether the side window is selected (ADR 0140), as any key may have changed that,
//! and names the keys of an empty composer's placeholder and of a session's footer (ADR 0170);
//! otherwise a frame only reads the editor's state.
//!
//! Whatever changes the modeline or the echo area gets a frame in one place
//! ([`App::ask_for_a_frame_if_status_changed`]), so a failed command, a message from a
//! save and a load's progress need no request of their own (ADR 0063).

use std::time::Instant;

use oso_core::editor::{Editor, MatchCells, Status};
use oso_core::frame::CellRect;
use oso_core::window::{Screen, WindowId};
use oso_render::{
    Bands, Face, GridRow, Layer, Pane, PaneContent, PaneId, Redraw, RenderState, Rules, Span,
};

use super::App;
use super::shell::{Loop, Window};
use crate::app::sessions::NativeSessions;
use crate::dispatch::OpenMenu;
use crate::sessions::list::SpaceBands;
use crate::terminal::hosted::Terminals;
use crate::text;

mod card;
mod chip;
mod float;
mod menu;
mod spaces;
mod tiled;
mod transcript;

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
    /// The rows of a server's description at point, and the cells of its first row, which
    /// names the server and is drawn dim (ADR 0138).
    hover: Screen,
    hover_named: Vec<CellRect>,
    /// The matches a project search's preview shows (ADR 0114).
    preview_matches: MatchCells,
    /// Room for the spans and rows each frame lists, empty between frames.
    room: tiled::Room,
    panes: Vec<Pane<'static>>,
    /// An open menu's float, worked out again only when it changes (ADR 0117).
    menu: menu::MenuCells,
    /// Room for the rows of the terminals shown, and which of them each one's are
    /// (ADR 0120).
    grid_rows: Vec<GridRow<'static>>,
    grids: Vec<(oso_core::buffer::BufferId, std::ops::Range<usize>)>,
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
#[cfg(test)]
pub(super) fn with_panes<R>(
    editor: &Editor,
    statuses: &[(WindowId, Status)],
    cells: &mut Cells,
    draw: impl FnOnce(&[Pane<'_>]) -> R,
) -> R {
    let terminals = Terminals::default();
    let bands = SpaceBands::default();
    let shown = (editor, &terminals, &bands, None);
    with_frame(shown, None, statuses, cells, draw)
}

/// What [`with_frame`] reads: the editor, the terminals, `*Spaces*`' bands, and the native
/// sessions, whose empty composers' placeholders and whose footers show (ADR 0170).
type Shows<'a> = (
    &'a Editor,
    &'a Terminals,
    &'a SpaceBands,
    Option<&'a NativeSessions>,
);

/// As [`with_panes`], with the open menu's float over the tiled panes, if one is open
/// (ADR 0117).
pub(super) fn with_frame<R>(
    (editor, terminals, bands, natives): Shows<'_>,
    open_menu: Option<&OpenMenu<'_>>,
    statuses: &[(WindowId, Status)],
    cells: &mut Cells,
    draw: impl FnOnce(&[Pane<'_>]) -> R,
) -> R {
    let menu_shown = cells.menu.update(editor, open_menu);
    tiled::fill(editor, bands, &mut cells.windows);
    editor.prompt_screen(&mut cells.answer);
    fill_float_screens(editor, cells);
    let empty = Status::default();
    let status = tiled::tiled_status(editor, statuses).unwrap_or(&empty);
    let echo = echo_spans(status);
    let echo = [&echo[..]];
    let mut room = std::mem::take(&mut cells.room);
    let hints = tiled::placeholder::rows(editor, &cells.windows, natives, &mut room.hints);
    let hint_lines = tiled::placeholder::lines(&hints, &mut room.hints);
    let feet = tiled::footer::rows(editor, natives, &mut room.feet);
    let foot_lines = tiled::footer::lines(&feet, &mut room.feet);
    let windows = editor.frame().windows();
    let spans = tiled::spans_of(windows, statuses, &cells.windows, &empty, &mut room);
    let rows = tiled::rows_of(&spans.0, &spans.1, &mut room);
    let mut grid_rows: Vec<GridRow<'_>> = reuse(std::mem::take(&mut cells.grid_rows));
    let mut grids = std::mem::take(&mut cells.grids);
    shown_grids((editor, terminals), &mut grid_rows, &mut grids);
    let drawn = tiled::Drawn {
        terminals: tiled::Grids::new(terminals, &grid_rows, &grids),
        cells: &cells.windows,
        behind: &spans.2,
        labels: &spans.3,
        modelines: &rows.0,
        gutters: &rows.1,
        statuses,
        placeholders: (&hints, &hint_lines),
        footers: (&feet, &foot_lines),
        natives,
        flash: cells.flash,
    };
    // A prompt's float goes over the tiled panes.
    let float = float::Parts::of(editor, &cells.answer);
    let float_spans = float.as_ref().map(float::Parts::spans);
    let lines = float_spans.as_ref().map(|spans| spans.lines());
    let mut list: Vec<Pane<'_>> = reuse(std::mem::take(&mut cells.panes));
    tiled::panes(editor, &drawn, &mut list);
    // After the windows, so that no amount of text can take its place (ADR 0061).
    list.push(echo_pane(editor, &echo));
    let preview_fills = [
        (Face::SearchCurrent, &cells.preview_matches.current[..]),
        (Face::SearchMatch, &cells.preview_matches.others[..]),
    ];
    // A description at point goes over the tiled panes, under a prompt's float, which closes
    // it anyway.
    float::hover_panes(editor, (&cells.hover, &cells.hover_named), &mut list);
    if let (Some(parts), Some(lines)) = (&float, &lines) {
        let screens = float::Screens {
            answer: &cells.answer,
            preview: &cells.preview,
            preview_fills: &preview_fills,
        };
        float::panes(editor, (parts, screens), lines, &mut list);
    }
    // The completion popup at point goes over the tiled panes too (ADR 0138, step 9).
    let popup = tiled::selected_screen(editor, &cells.windows)
        .and_then(|screen| float::popup::PopupSpans::of(editor, screen));
    let popup_lines = popup.as_ref().map(float::popup::PopupSpans::lines);
    if let (Some(popup), Some(lines)) = (&popup, &popup_lines) {
        popup.panes(editor, lines, &mut list);
    }
    // A menu never shows with a prompt, which closes it (ADR 0117, Q20).
    let menu_open = open_menu.filter(|_| menu_shown).map(OpenMenu::open);
    let mut menu_room = cells.menu.take_room();
    let menu_spans =
        menu_open.and_then(|open| menu::MenuSpans::of(&cells.menu, open, &mut menu_room));
    let menu_lines = menu_spans.as_ref().map(|spans| spans.lines(&mut menu_room));
    if let Some(lines) = &menu_lines {
        menu::panes(editor, &cells.menu, lines, &mut list);
    }
    let result = draw(&list);
    cells.panes = reuse(list);
    (cells.grid_rows, cells.grids) = (reuse(grid_rows), grids);
    let menu_room = menu::lines_back(menu_room, menu_lines);
    cells
        .menu
        .give_back(menu::spans_back(menu_room, menu_spans));
    tiled::footer::give_back_lines(&mut room.feet, foot_lines);
    tiled::footer::give_back_rows(&mut room.feet, feet);
    tiled::placeholder::give_back_lines(&mut room.hints, hint_lines);
    tiled::placeholder::give_back_rows(&mut room.hints, hints);
    tiled::give_back_rows(&mut room, rows);
    tiled::give_back_spans(&mut room, spans);
    cells.room = room;
    result
}

/// What a frame's first footer says, for the dev socket (ADR 0170).
#[cfg(feature = "dev")]
pub(super) fn footer_said(editor: &Editor, natives: &NativeSessions) -> Option<String> {
    tiled::footer::said(editor, natives)
}

/// What the echo area says, its warnings first.
fn echo_spans(status: &Status) -> [Span<'_>; 2] {
    let span = |text, face| Span { text, face };
    [
        span(status.echo_warnings(), Face::Warning),
        span(&status.echo, Face::Text),
    ]
}

/// The rows the floats over the windows show into `cells`: a prompt's preview with the matches
/// of a search in it, and a server's description at point (ADR 0138).
fn fill_float_screens(editor: &Editor, cells: &mut Cells) {
    editor.preview_screen(&mut cells.preview);
    editor.preview_cells(&cells.preview, &mut cells.preview_matches);
    editor.hover_screen(&mut cells.hover);
    cells.hover_named.clear();
    let named = editor.hover().filter(|shown| shown.top() == 0);
    if let (Some(regions), Some(shown)) = (editor.hover_layout(), named) {
        cells.hover_named.push(CellRect {
            rows: 1,
            columns: shown.label().min(regions.inner.columns),
            ..regions.inner
        });
    }
}

/// The echo area's pane, showing `echo`. It has no background of its own, so that nothing marks
/// the pixels left over below it (ADR 0057).
fn echo_pane<'a>(editor: &Editor, echo: &'a [&'a [Span<'a>]]) -> Pane<'a> {
    Pane {
        id: ECHO,
        region: editor.frame().echo,
        layer: Layer::TILED,
        background: None,
        panel: None,
        bands: Bands::NONE,
        rules: Rules::NONE,
        content: PaneContent::Lines(echo),
    }
}

/// Every terminal a window shows, its rows once however many windows show it, into `rows`,
/// and which of them each one's are into `grids`, emptied first (ADR 0120).
fn shown_grids<'a>(
    (editor, terminals): (&Editor, &'a Terminals),
    rows: &mut Vec<GridRow<'a>>,
    grids: &mut Vec<(oso_core::buffer::BufferId, std::ops::Range<usize>)>,
) {
    grids.clear();
    for (id, _) in editor.frame().windows() {
        let Some(buffer) = editor
            .window_by_id(*id)
            .map(oso_core::window::Window::buffer)
        else {
            continue;
        };
        if grids.iter().any(|(shown, _)| *shown == buffer) {
            continue;
        }
        if let Some(hosted) = terminals.get(buffer) {
            let start = rows.len();
            hosted.grid.rows_into(rows);
            grids.push((buffer, start..rows.len()));
        }
    }
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
        // While work goes on over turns, its pace asks for frames (ADR 0119).
        if self.view.is_none() || self.slices_go_on() {
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
        // At most one snapshot per terminal per frame (ADR 0120, Q4); one more may wait.
        if self.take_terminal_snapshots() {
            self.request_redraw();
        }
        // Sessions' ages move on with a frame drawn anyway (ADR 0121, Q4).
        self.age_sessions();
        self.note_pane_focus();
        self.note_placeholder();
        // A load, a reload or a key may have given the line count another digit (ADR 0097).
        self.editor.fit_gutter();
        // What this frame shows of a request, from the state it is built of (ADR 0182).
        self.note_frame_of_requests();
        let Some(view) = self.view.as_mut() else {
            return;
        };
        take_theme(view, &self.editor, &self.terminals);
        let window = &view.window;
        let window_size = || window.inner_size();
        statuses_into(&self.editor, &mut self.statuses);
        let presented = view.render_state.presented_frames();
        let fits = view.render_state.frame_size() == self.editor.frame().size();
        #[cfg(feature = "dev")]
        let rendering = Instant::now();
        let render_state = &mut view.render_state;
        let cells = &mut view.cells;
        cells.flash = self.flash.strength(Instant::now());
        let open_menu = self.dispatcher.menu();
        let (bands, natives) = (self.watch.pane_bands(), &self.hosting.natives);
        let shown = (&self.editor, &self.terminals.map, bands, Some(natives));
        let result = with_frame(shown, open_menu.as_ref(), &self.statuses, cells, |panes| {
            render_state.render(panes, &window_size)
        });
        // Recovering from a surface error can configure it to a new size before
        // `Resized` says so, and panes stop at their regions, so the editor catches
        // up now.
        let size = view.render_state.frame_size();
        // Every glyph of it, at the size it was laid out for, before and after.
        let whole = fits
            && matches!(result, Ok(Redraw::NotNeeded))
            && view.render_state.presented_frames() > presented
            && size == self.editor.frame().size();
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
                    self.timings.session_marks(self.watch.needs_input());
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
        self.frame_of_requests_drawn(whole);
    }
}

/// Draws in the theme the editor names, from this frame on. A switch costs this frame only: nothing is
/// shaped again (ADR 0089). The editor's names are the renderer's, so it never refuses one.
fn take_theme(view: &mut View, editor: &Editor, terminals: &super::terminals::Host) {
    let wanted = editor.theme_name();
    let Some(name) = wanted.filter(|_| wanted != view.theme) else {
        return;
    };
    match view.render_state.set_theme(name) {
        Some(_) => view.theme = Some(name),
        None => debug_assert!(false, "the renderer has no theme named {name}"),
    }
    super::terminals::retheme(&terminals.map, &terminals.palettes, editor.theme());
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::buffer::{Buffer, Mode, Replacement};
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
                            | PaneContent::Grid(_)
                            | PaneContent::Signs(_)
                            | PaneContent::Rule { .. }
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

    /// An editor on a session's buffer with a composer docked under it that holds `text`, and
    /// the composer's window.
    fn composer(text: &str) -> (Editor, WindowId) {
        use oso_core::window::DockSlot;
        let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\n"));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        let host = editor.window().buffer();
        let mut input = Buffer::from_text("*compose: s*", text);
        input.set_mode(Mode::Composer);
        let input = editor.add_buffer(input);
        editor.set_dock(host, DockSlot::Composer, Some(input));
        let docked = editor.dock_window(host, DockSlot::Composer).unwrap();
        (editor, docked)
    }

    /// A docked composer (ADR 0170) is drawn as a window inside its host's rect: its text over
    /// the float's face, its mark in its gutter, and no modeline of its own.
    #[test]
    fn a_docked_composer_is_drawn_inside_its_host_with_no_modeline() {
        let (editor, docked) = composer("hi");
        let regions = *editor.regions(docked).unwrap();
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                let text = panes
                    .iter()
                    .find(|pane| pane.id == tiled::pane_id(docked, tiled::Part::Text))
                    .unwrap();
                assert_eq!(text.region, regions.text);
                assert_eq!(text.background, Some(Face::Float));
                let gutter = panes
                    .iter()
                    .find(|pane| pane.id == tiled::pane_id(docked, tiled::Part::Gutter))
                    .unwrap();
                let PaneContent::Lines(lines) = &gutter.content else {
                    panic!("a gutter of lines");
                };
                assert_eq!(lines[0][0].text, "› ");
                let modeline = tiled::pane_id(docked, tiled::Part::Modeline);
                assert!(panes.iter().all(|pane| pane.id != modeline));
                let ids: Vec<_> = panes.iter().map(|pane| pane.id).collect();
                let unique = ids
                    .iter()
                    .enumerate()
                    .all(|(at, id)| !ids[..at].contains(id));
                assert!(unique, "each pane its own id");
            },
        );
    }

    /// The cursor's style in the text pane of `window`, as drawn.
    fn cursor_style(editor: &Editor, window: WindowId) -> oso_render::CursorStyle {
        with_panes(editor, &statuses(editor), &mut Cells::default(), |panes| {
            panes
                .iter()
                .find(|pane| pane.id == tiled::pane_id(window, tiled::Part::Text))
                .map(|pane| match &pane.content {
                    PaneContent::Text(view) => view.cursor_style,
                    _ => panic!("a text pane"),
                })
                .unwrap()
        })
    }

    /// The cursor is filled in the window keys go to, docked or not, though a docked window's
    /// host has the active modeline (ADR 0170).
    #[test]
    fn a_selected_dock_has_the_filled_cursor_and_its_host_the_hollow_one() {
        use oso_render::CursorStyle;
        let (mut editor, docked) = composer("");
        let host = editor.window().id();
        assert_eq!(cursor_style(&editor, host), CursorStyle::Box);
        assert_eq!(cursor_style(&editor, docked), CursorStyle::Hollow);
        editor.select_window(docked);
        assert_eq!(cursor_style(&editor, host), CursorStyle::Hollow);
        assert_eq!(cursor_style(&editor, docked), CursorStyle::Box);
        let status = statuses(&editor);
        let active = status.iter().find(|(id, _)| *id == host).unwrap();
        assert!(active.1.selected, "the host's modeline speaks for the dock");
        // Beneath a prompt neither is selected.
        editor.ask("Exit?", "keyboard-quit").unwrap();
        assert_eq!(cursor_style(&editor, host), CursorStyle::Hollow);
        assert_eq!(cursor_style(&editor, docked), CursorStyle::Hollow);
    }

    /// The placeholder's pane of a frame of `editor`: where it is, over which background, and
    /// its text and face.
    fn placeholder(editor: &Editor, window: WindowId, hint: [&str; 3]) -> Option<Shown> {
        placeholder_with(editor, window, (hint, None))
    }

    /// As [`placeholder`], with `suggested` the suggestion of the window's composer.
    fn placeholder_with(
        editor: &Editor,
        window: WindowId,
        (hint, suggested): ([&str; 3], Option<&str>),
    ) -> Option<Shown> {
        let (terminals, bands) = (Terminals::default(), SpaceBands::default());
        let its = editor.window_by_id(window)?.buffer();
        let takes = "   (C-c C-s takes it)";
        let natives = NativeSessions::saying(hint, takes, suggested.map(|text| (its, text)));
        let shown = (editor, &terminals, &bands, Some(&natives));
        let statuses = statuses(editor);
        with_frame(shown, None, &statuses, &mut Cells::default(), |panes| {
            let id = tiled::pane_id(window, tiled::Part::Placeholder);
            let pane = panes.iter().find(|pane| pane.id == id)?;
            assert!(pane.layer > Layer::TILED && pane.layer < Layer::FLOAT);
            let PaneContent::Lines(lines) = &pane.content else {
                panic!("a placeholder of lines");
            };
            let spans = lines[0].iter();
            let spans = spans.map(|span| (span.text.to_owned(), span.face));
            Some((pane.region, pane.background, spans.collect()))
        })
    }

    /// An empty composer says what it is for after its cursor, selected or not, over the band,
    /// and nothing of it is in the buffer (ADR 0170).
    #[test]
    fn an_empty_composer_shows_its_placeholder_until_anything_is_typed() {
        let type_in = |editor: &mut Editor, range, text| {
            let id = editor.window().buffer();
            let edit = Replacement { range, text };
            let _ = editor.apply_edits(id, &[edit]);
        };
        let (mut editor, docked) = composer("");
        let hint = ["Write to Claude"; 3];
        let text = editor.regions(docked).unwrap().text;
        let region = CellRect {
            column: text.column + 1,
            columns: text.columns - 1,
            ..text
        };
        let wanted = (region, None, vec![(hint[0].to_owned(), Face::Prompt)]);
        let shown = placeholder(&editor, docked, hint);
        assert_eq!(shown, Some(wanted.clone()), "not selected");
        let host = editor.window().id();
        assert_eq!(placeholder(&editor, host, hint), None, "the transcript");

        editor.select_window(docked);
        assert_eq!(placeholder(&editor, docked, hint), Some(wanted.clone()));
        assert!(editor.current().1.text().is_empty());
        // An input method's text, and the person's.
        editor.set_preedit(Some(oso_core::display_map::Preedit::new("か".into(), None)));
        assert_eq!(placeholder(&editor, docked, hint), None, "composing");
        editor.set_preedit(None);
        assert_eq!(placeholder(&editor, docked, hint), Some(wanted.clone()));
        type_in(&mut editor, 0..0, "x");
        assert_eq!(placeholder(&editor, docked, hint), None, "typed");
        type_in(&mut editor, 0..1, "");
        assert_eq!(placeholder(&editor, docked, hint), Some(wanted));
    }

    /// A narrow window leaves the placeholder's last parts out, whole; the first is cut.
    #[test]
    fn a_narrow_windows_placeholder_leaves_whole_parts_out() {
        let (mut editor, docked) = composer("");
        let forms = ["Write · a b · c", "Write · a b", "Write"];
        let mut at = |columns: usize| {
            // The mark's two cells and the cursor's come before it.
            let columns = columns + 3;
            editor.set_frame_size(FrameSize { rows: 24, columns });
            let (region, _, spans) = placeholder(&editor, docked, forms).unwrap();
            assert_eq!(region.columns, columns - 3);
            spans[0].0.clone()
        };
        assert_eq!(at(15), "Write · a b · c");
        assert_eq!(at(14), "Write · a b");
        assert_eq!(at(11), "Write · a b");
        assert_eq!(at(10), "Write");
        assert_eq!(at(4), "Write", "cut by its pane");
    }

    /// A session's suggestion shows over the hint in an empty composer, with the key that takes
    /// it whole, and goes as the hint does (ADR 0165, part 3f).
    #[test]
    fn a_suggestion_shows_in_the_placeholder_until_anything_is_typed() {
        let (mut editor, docked) = composer("");
        let hint = ["Write to Claude"; 3];
        let says = |editor: &Editor| {
            let shown = placeholder_with(editor, docked, (hint, Some("run the tests")));
            shown.map(|(_, _, spans)| spans.iter().map(|span| &span.0[..]).collect::<String>())
        };
        let wanted = Some("run the tests   (C-c C-s takes it)".to_owned());
        assert_eq!(says(&editor), wanted, "not selected");
        editor.select_window(docked);
        assert_eq!(says(&editor), wanted);
        assert!(editor.current().1.text().is_empty());
        editor.set_preedit(Some(oso_core::display_map::Preedit::new("か".into(), None)));
        assert_eq!(says(&editor), None, "composing");
        editor.set_preedit(None);
        let id = editor.window().buffer();
        let typed = |range, text| [Replacement { range, text }];
        editor.apply_edits(id, &typed(0..0, "x")).unwrap();
        assert_eq!(says(&editor), None, "typed");
        editor.apply_edits(id, &typed(0..1, "")).unwrap();
        assert_eq!(says(&editor), wanted, "back");
        let host = editor.window().docked().unwrap().0;
        let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
        secret.set_mode(Mode::Composer);
        let secret = editor.add_buffer(secret);
        editor.set_dock(host, oso_core::window::DockSlot::Composer, Some(secret));
        assert_eq!(says(&editor), None, "secret");
    }

    /// A suggestion is cut for the key after it, which is whole; with no room for both, oso's
    /// hint shows, so that Claude's text never shows without oso's words.
    #[test]
    fn a_narrow_window_cuts_the_suggestion_never_its_key() {
        let (mut editor, docked) = composer("");
        let hint = ["Write · a b", "Write", "W"];
        let mut at = |columns: usize, text: &str| {
            let columns = columns + 3;
            editor.set_frame_size(FrameSize { rows: 24, columns });
            let shown = placeholder_with(&editor, docked, (hint, Some(text))).unwrap();
            let spans = shown.2.iter().map(|span| &span.0[..]);
            spans.collect::<String>()
        };
        // The key and its brackets take 21 cells.
        let text = "Write to Claude";
        assert_eq!(at(36, text), "Write to Claude   (C-c C-s takes it)");
        assert_eq!(at(35, text), "Write to Clau…   (C-c C-s takes it)");
        assert_eq!(at(29, text), "Write t…   (C-c C-s takes it)");
        assert_eq!(at(28, text), "Write · a b", "fewer than eight cells");
        assert_eq!(at(10, text), "Write");
        assert_eq!(at(23, "go"), "go   (C-c C-s takes it)", "short and whole");
        assert_eq!(at(22, "go"), "Write · a b");
        // Whole characters, two cells each.
        assert_eq!(at(29, "テストを実行"), "テスト…   (C-c C-s takes it)");
    }

    /// Nothing is said over an empty composer that turned secret, as it sends nothing, nor over a
    /// buffer docked in its place that is no composer.
    #[test]
    fn no_placeholder_in_a_secret_composer_or_another_buffer() {
        let (mut editor, docked) = composer("");
        assert!(placeholder(&editor, docked, ["hint"; 3]).is_some());
        let host = editor.window().buffer();
        let plain = editor.add_buffer(Buffer::from_text("plain", ""));
        editor.set_dock(host, oso_core::window::DockSlot::Composer, Some(plain));
        assert_eq!(
            placeholder(&editor, docked, ["hint"; 3]),
            None,
            "no composer"
        );
        // As `suggesting`'s test makes one.
        let mut secret = Buffer::visiting(std::path::PathBuf::from("/tmp/.env"));
        secret.set_mode(Mode::Composer);
        assert!(secret.is_secret() && secret.text().is_empty());
        let secret = editor.add_buffer(secret);
        editor.set_dock(host, oso_core::window::DockSlot::Composer, Some(secret));
        assert_eq!(placeholder(&editor, docked, ["hint"; 3]), None, "secret");
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
                        PaneContent::Rule { .. } => "rule".to_owned(),
                        PaneContent::Masked(_) => String::new(),
                        PaneContent::Grid(_) => "grid".to_owned(),
                        PaneContent::Signs(signs) => format!("signs {signs:?}"),
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
        // the echo area. Each window has ten ids, the others for what is drawn in some alone.
        assert_eq!(ids, [26, 16, 17, 27, 18, 28, 19, ECHO.0]);
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
        assert_eq!(ids, [26, 27, 28, ECHO.0]);
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
        assert_eq!((tiled[0].0.0, tiled[1].0.0), (26, 16));
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
        // Then dictation's mark, none here (ADR 0175).
        assert_eq!(modeline[1], (String::new(), Face::Prompt));
        // Then the sessions' marks, none here (ADR 0121).
        assert_eq!(modeline[2], (String::new(), Face::SessionAlert));
        assert_eq!(modeline[3], (String::new(), Face::SessionDone));
        assert_eq!(modeline[4], ("[bidi controls] ".to_owned(), Face::Warning));
        assert_eq!(modeline[5], ("-- a.txt  ".to_owned(), Face::Modeline));
        assert_eq!(modeline[6].1, Face::Modeline);
        let echo = &panes[2].2;
        assert_eq!(echo[0], (String::new(), Face::Warning), "not said twice");
        assert_eq!(echo[1], ("Saved".to_owned(), Face::Text));
    }

    /// The spans of the selected window's modeline in `editor`, with their faces.
    fn modeline_of(editor: &Editor) -> Vec<(String, Face)> {
        with_panes(editor, &statuses(editor), &mut Cells::default(), |panes| {
            let modeline = panes
                .iter()
                .find(|pane| pane.background == Some(Face::Modeline));
            match modeline.map(|pane| &pane.content) {
                Some(PaneContent::Lines(lines)) => lines[0]
                    .iter()
                    .map(|span| (span.text.to_owned(), span.face))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            }
        })
    }

    #[test]
    fn a_dictation_that_records_is_red_and_one_that_waits_is_dim() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", ""));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        editor.set_dictation_mark(oso_core::editor::DictationMark::Recording);
        assert_eq!(
            modeline_of(&editor)[1],
            ("● REC  ".to_owned(), Face::Dictation)
        );
        editor.set_dictation_mark(oso_core::editor::DictationMark::Waiting);
        assert_eq!(modeline_of(&editor)[1], ("…  ".to_owned(), Face::Prompt));
    }

    #[test]
    fn the_sessions_marks_draw_in_their_states_faces() {
        let mut editor = Editor::with_buffer(Buffer::from_text("a.txt", ""));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        editor.set_session_marks(oso_core::editor::SessionMarks {
            needs_input: 2,
            done: 3,
            dark: false,
        });
        let modeline = with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                let modeline = panes
                    .iter()
                    .find(|pane| pane.background == Some(Face::Modeline));
                match modeline.map(|pane| &pane.content) {
                    Some(PaneContent::Lines(lines)) => lines[0]
                        .iter()
                        .map(|span| (span.text.to_owned(), span.face))
                        .collect::<Vec<_>>(),
                    _ => Vec::new(),
                }
            },
        );
        // As `*Spaces*` draws their marks (ADR 0131).
        assert_eq!(modeline[2], ("⚠ 2  ".to_owned(), Face::SessionAlert));
        assert_eq!(modeline[3], ("● 3  ".to_owned(), Face::SessionDone));
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

    /// A review comment's draft over several rows (ADR 0122): its answer's pane takes them
    /// all, with the gap above its first and the padding below its last, its field is one
    /// over the same rows, and its text's rows are drawn in it.
    #[test]
    fn a_drafts_answer_of_several_rows_is_one_pane_and_one_field() {
        use oso_core::buffer::Replacement;
        use oso_core::editor::DraftFor;
        use oso_core::review::{Anchor, Side};

        let (mut editor, project) = crate::commands::review::tests::in_project(true);
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        let (review, _) = editor.add_review(project, "*review*".into());
        let anchor = Anchor::Line {
            side: Side::New,
            number: 1,
        };
        let draft = DraftFor::Comment {
            path: b"a".as_slice().into(),
            anchor,
        };
        editor.open_draft(review, draft, "").unwrap();
        let buffer = editor.prompt().unwrap().buffer();
        let text = "one\ntwo\nthree\nfour";
        let edit = Replacement { range: 0..0, text };
        editor.apply_edits(buffer, &[edit]).unwrap();
        let layout = editor.prompt_layout().unwrap().clone();
        assert_eq!(layout.input.rows, 4);
        assert_eq!(layout.field.rows, 4);
        let regions = layout.regions;
        let mut cells = Cells::default();
        with_panes(&editor, &statuses(&editor), &mut cells, |panes| {
            let float = panes.iter().find(|pane| pane.layer == Layer::FLOAT);
            let panel = float.and_then(|pane| pane.panel.as_ref()).unwrap();
            assert_eq!(panel.field, Some(layout.field));
            let answer = panes
                .iter()
                .find(|pane| pane.region == layout.input)
                .unwrap();
            assert_eq!(answer.bands.above, regions.gap);
            assert_eq!(answer.bands.below, regions.padded_below());
            assert!(regions.padded_below(), "a float of 24 rows has room");
            let PaneContent::Text(view) = &answer.content else {
                panic!("the answer is the prompt's window");
            };
            let lines: Vec<usize> = view.screen.rows().iter().map(|row| row.line).collect();
            assert_eq!(lines, [0, 1, 2, 3], "a row for each line");
            // Nothing else of the float keeps a band into the padding below.
            for pane in panes
                .iter()
                .filter(|pane| pane.layer == Layer::FLOAT_CONTENT && pane.region != layout.input)
            {
                assert!(!pane.bands.below, "{:?}", pane.region);
            }
        });
    }

    /// An editor on two lines of Rust that a server follows, with an error on `x` and a warning
    /// on `fn`.
    fn with_diagnostics() -> Editor {
        use oso_core::buffer::ServerMark;
        use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
        let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "let x = 1;\nfn f() {}\n"));
        editor.set_frame_size(FrameSize {
            rows: 6,
            columns: 40,
        });
        let id = editor.window().buffer();
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.set_followed(true);
        buffer.set_server(Some(ServerMark::Ready));
        let new = |range, severity| New {
            range,
            severity,
            tags: Tags::default(),
            source: "",
            code: "",
            message: "m",
        };
        let set = Diagnostics::new(
            "fake",
            [new(4..5, Severity::Error), new(11..13, Severity::Warning)],
            buffer.text().len(),
        );
        let revision = buffer.revision();
        assert!(buffer.take_diagnostics(revision, set));
        editor.fit_gutter();
        editor
    }

    #[test]
    fn a_buffer_a_server_follows_draws_its_signs_lines_and_counts_in_their_faces() {
        let editor = with_diagnostics();
        let regions = editor.tiled_regions();
        assert_eq!(regions.gutter.columns, 1, "the sign column alone");
        let window = editor.tiled().0.id();
        with_panes(
            &editor,
            &statuses(&editor),
            &mut Cells::default(),
            |panes| {
                let signs = panes
                    .iter()
                    .find(|pane| pane.id == tiled::pane_id(window, tiled::Part::Signs))
                    .expect("a sign column");
                assert_eq!(signs.region.columns, 1);
                assert_eq!(signs.region.column, regions.gutter.column);
                let PaneContent::Signs(shown) = &signs.content else {
                    panic!("signs");
                };
                let (e, w) = (Some(Face::DiagnosticError), Some(Face::DiagnosticWarning));
                assert_eq!(&shown[..2], [e, w]);
                let text = panes
                    .iter()
                    .find(|pane| pane.id == tiled::pane_id(window, tiled::Part::Text))
                    .unwrap();
                let PaneContent::Text(view) = &text.content else {
                    panic!("text");
                };
                let lines: Vec<_> = view
                    .marks
                    .underlines
                    .iter()
                    .map(|(face, cells)| (*face, cells.len()))
                    .collect();
                assert_eq!(
                    lines,
                    [
                        (Face::DiagnosticError, 1),
                        (Face::DiagnosticWarning, 1),
                        (Face::DiagnosticInfo, 0)
                    ]
                );
                let modeline = panes
                    .iter()
                    .find(|pane| pane.id == tiled::pane_id(window, tiled::Part::Modeline))
                    .unwrap();
                let PaneContent::Lines(rows) = &modeline.content else {
                    panic!("modeline");
                };
                let counts: Vec<_> = rows[0]
                    .iter()
                    .filter(|span| span.text.contains(['E', 'W']) && span.text.starts_with(' '))
                    .map(|span| (span.text, span.face))
                    .collect();
                assert_eq!(
                    counts,
                    [
                        (" E1", Face::DiagnosticError),
                        (" W1", Face::DiagnosticWarning)
                    ]
                );
            },
        );
    }
}
