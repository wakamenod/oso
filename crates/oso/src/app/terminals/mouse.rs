//! The mouse in the app (ADR 0120, step 4b-2; ADR 0112). A press selects the window under it,
//! and the first press on a window not selected does only that; point never moves in a text
//! buffer. In the selected window's terminal, a program that asked for mouse reports gets them,
//! but never while it reads a password; otherwise, or with Shift held, a drag selects cells,
//! and the release copies them to the kill ring and the OS clipboard, as `M-w` does. The wheel
//! is `wheel`'s. While oso holds input (a prompt, a menu, a key sequence and the rest that
//! hold keys from a terminal), presses are dropped. A drag ends early, copying nothing, when the
//! window loses the focus, another press comes, or its window is no longer selected or oso
//! holds input.
//!
//! Must not depend on: how a frame is drawn, beyond the size of a cell and where a window's
//! text is.

use oso_core::buffer::BufferId;
use oso_core::window::WindowId;
use oso_platform::window::{Held, Mouse, MouseAction, MouseButton, Position};
use oso_term::{Button, Pointer, PointerAction};

use super::super::App;
use super::reads_a_password;

/// A cell of a terminal's screen, and the pointer in pixels from the screen's top left:
/// `(row, column, x, y)`.
pub(super) type At = (u16, u16, u32, u32);

/// What became of a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reported {
    /// Nothing went: no cell, no program, or a motion in the cell of the last.
    Skipped,
    Sent,
    /// The terminal's input queue was full, and the person was told.
    Full,
}

/// What the button pressed does until its release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// Nothing: the press selected a window, was dropped, or fell on no terminal's text.
    Nothing,
    /// oso selects cells of the terminal `buffer`, shown in `window`.
    Select { window: WindowId, buffer: BufferId },
    /// The program in `buffer`, shown in `window`, gets the reports; `last` is where the
    /// last one went, for a release when the drag ends early.
    Report {
        window: WindowId,
        buffer: BufferId,
        last: Option<At>,
    },
}

/// The mouse as the app follows it.
#[derive(Default)]
pub(in crate::app) struct Pointing {
    /// The button held, and what its press began.
    pressed: Option<(MouseButton, Drag)>,
    /// The cell the last motion report went for, so that one goes per cell.
    reported: Option<(BufferId, u16, u16)>,
    /// Whether the window was asked to report moves with no button held.
    motion: bool,
}

impl Pointing {
    /// Ends the drag going on, if any, and returns it.
    fn take_drag(&mut self) -> Option<(MouseButton, Drag)> {
        self.reported = None;
        self.pressed.take()
    }

    /// Begins `drag` for `button`, once [`Pointing::take_drag`] ended any before it.
    fn begin(&mut self, button: MouseButton, drag: Drag) {
        self.pressed = Some((button, drag));
    }

    /// The drag `button`'s release ends, if one goes on for it.
    fn released(&self, button: MouseButton) -> Option<Drag> {
        let (pressed, drag) = self.pressed?;
        (pressed == button).then_some(drag)
    }
}

/// Where a press goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PressGoes {
    /// oso holds input: nothing happens.
    Dropped,
    /// It only selects the window under it.
    SelectWindow,
    /// It begins a selection of cells.
    Selection,
    /// The program gets it.
    Report,
    /// Nothing more: a text window, or no terminal's text, under a selected window.
    Nothing,
}

/// What a press does: oso `holding` input, on a window `selected` or not, of a `terminal` or
/// not, over its text or not (`in_text`), for a program that wants `reports`, with `shift`
/// held, of the `left` button or another.
fn route_press(
    holding: bool,
    selected: bool,
    terminal: bool,
    in_text: bool,
    reports: bool,
    shift: bool,
    left: bool,
) -> PressGoes {
    if holding {
        PressGoes::Dropped
    } else if !selected {
        PressGoes::SelectWindow
    } else if !terminal || !in_text {
        PressGoes::Nothing
    } else if reports && !shift {
        PressGoes::Report
    } else if left {
        PressGoes::Selection
    } else {
        PressGoes::Nothing
    }
}

impl App {
    /// A mouse button pressed or released, or the pointer moved. Returns false for a move
    /// that did nothing, after which the event needs no more work.
    pub(in crate::app) fn handle_mouse(&mut self, mouse: Mouse) -> bool {
        if !matches!(mouse.action, MouseAction::Moved) {
            self.person_typed("mouse");
        }
        match mouse.action {
            MouseAction::Press(button) => self.mouse_pressed(button, mouse.at, mouse.held),
            MouseAction::Release(button) => self.mouse_released(button, mouse.at, mouse.held),
            MouseAction::Moved => return self.mouse_moved(mouse.at, mouse.held),
        }
        true
    }

    fn mouse_pressed(&mut self, button: MouseButton, at: Position, held: Held) {
        // One button at a time: a press during a drag ends it and begins its own, so a
        // release that never came cannot hold a drag on.
        self.end_drag();
        // A press clears any selection.
        let mut cleared = false;
        for (_, hosted) in self.terminals.map.iter_mut() {
            cleared |= hosted.grid.clear_selection();
        }
        if cleared {
            self.request_redraw();
        }
        let Some((window, in_text)) = self.window_under(at) else {
            return;
        };
        let buffer = self.editor.window_by_id(window).map(|w| w.buffer());
        let hosted = buffer.and_then(|buffer| self.terminals.map.get(buffer));
        // While its program reads a password, no report reaches it (the author, 2026-11-14).
        let reports = hosted.is_some_and(|h| h.grid.modes().mouse != oso_term::Mouse::Off)
            && buffer.is_some_and(|buffer| !self.reads_password(buffer));
        let goes = route_press(
            self.oso_holds_input(),
            window == self.editor.tiled().0.id(),
            hosted.is_some(),
            in_text,
            reports,
            held.contains(Held::SHIFT),
            button == MouseButton::Left,
        );
        let drag = match (goes, buffer) {
            (PressGoes::SelectWindow, _) => {
                self.editor.select_window(window);
                self.request_redraw();
                Drag::Nothing
            }
            (PressGoes::Selection, Some(buffer)) => {
                if let Some((row, column, ..)) = self.cell_in(window, buffer, at)
                    && let Some(hosted) = self.terminals.map.get_mut(buffer)
                {
                    hosted.grid.select_from(row, column);
                }
                Drag::Select { window, buffer }
            }
            (PressGoes::Report, Some(buffer)) => {
                let cell = self.cell_in(window, buffer, at);
                self.report(
                    buffer,
                    cell,
                    held,
                    PointerAction::Press(term_button(button)),
                );
                Drag::Report {
                    window,
                    buffer,
                    last: cell,
                }
            }
            _ => Drag::Nothing,
        };
        self.terminals.pointer.begin(button, drag);
    }

    fn mouse_released(&mut self, button: MouseButton, at: Position, held: Held) {
        let Some(drag) = self.terminals.pointer.released(button) else {
            return;
        };
        if !self.drag_goes_on(drag) {
            self.end_drag();
            return;
        }
        self.terminals.pointer.take_drag();
        match drag {
            Drag::Select { window, buffer } => {
                let _ = self.select_to(window, buffer, at);
                self.copy_selection(buffer);
            }
            Drag::Report { window, buffer, .. } => {
                let cell = self.cell_in(window, buffer, at);
                self.report(
                    buffer,
                    cell,
                    held,
                    PointerAction::Release(term_button(button)),
                );
            }
            Drag::Nothing => {}
        }
    }

    /// Returns whether the move did anything.
    fn mouse_moved(&mut self, at: Position, held: Held) -> bool {
        if let Some((_, drag)) = self.terminals.pointer.pressed
            && !self.drag_goes_on(drag)
        {
            self.end_drag();
            return true;
        }
        match self.terminals.pointer.pressed {
            Some((_, Drag::Select { window, buffer })) => self.select_to(window, buffer, at),
            Some((button, Drag::Report { window, buffer, .. })) => {
                let cell = self.cell_in(window, buffer, at);
                let went =
                    self.report(buffer, cell, held, PointerAction::Motion) != Reported::Skipped;
                if went {
                    let last = cell;
                    self.terminals.pointer.pressed = Some((
                        button,
                        Drag::Report {
                            window,
                            buffer,
                            last,
                        },
                    ));
                }
                went
            }
            Some((_, Drag::Nothing)) => false,
            // With no button held, only for a program that asked for every motion (1003), in
            // the selected window, under the pointer.
            None => {
                let window = self.editor.tiled().0.id();
                let buffer = self.editor.tiled().0.buffer();
                let wants = self.wants_every_motion(buffer)
                    && !held.contains(Held::SHIFT)
                    && !self.oso_holds_input()
                    && (self.window_under(at)).is_some_and(|(w, inside)| w == window && inside);
                let cell = if wants {
                    self.cell_in(window, buffer, at)
                } else {
                    None
                };
                self.report(buffer, cell, held, PointerAction::Motion) != Reported::Skipped
            }
        }
    }

    /// Whether a drag begun as `drag` may go on: its window is still the one selected, oso
    /// holds no input, and a program getting reports reads no password. A key may have
    /// selected another window or opened a prompt while the button was held.
    fn drag_goes_on(&self, drag: Drag) -> bool {
        let (window, reporting) = match drag {
            Drag::Nothing => return true,
            Drag::Select { window, .. } => (window, None),
            Drag::Report { window, buffer, .. } => (window, Some(buffer)),
        };
        window == self.editor.tiled().0.id()
            && !self.oso_holds_input()
            && reporting.is_none_or(|buffer| !self.reads_password(buffer))
    }

    /// Ends the drag going on, if any, with no copy: the window lost the focus, another
    /// press came, or the drag may not go on. A program that got the press gets its release
    /// where its last report went, so neither it nor the terminal's thread holds the button,
    /// unless it now reads a password.
    pub(in crate::app) fn end_drag(&mut self) {
        let Some((button, drag)) = self.terminals.pointer.take_drag() else {
            return;
        };
        if let Drag::Report { buffer, last, .. } = drag
            && !self.reads_password(buffer)
        {
            let release = PointerAction::Release(term_button(button));
            self.report(buffer, last, self.modifiers, release);
        }
    }

    /// Whether the terminal `buffer` runs a program that asked for every motion (1003), and
    /// reads no password.
    fn wants_every_motion(&self, buffer: BufferId) -> bool {
        (self.terminals.map.get(buffer)).is_some_and(|h| {
            h.terminal.is_some() && h.grid.modes().mouse == oso_term::Mouse::Motion
        }) && !self.reads_password(buffer)
    }

    /// Whether oso holds input from terminals now, as it holds keys: see
    /// `Dispatcher::holds_the_mouse`, and a composition.
    pub(super) fn oso_holds_input(&self) -> bool {
        self.dispatcher.holds_the_mouse(&self.editor) || self.composing()
    }

    /// Whether the terminal `buffer`'s program reads a password, or the buffer is gone or
    /// secret: no mouse report or wheel arrow goes to it then (ADR 0120, Q17).
    pub(super) fn reads_password(&self, buffer: BufferId) -> bool {
        self.editor
            .buffer(buffer)
            .is_none_or(|b| b.is_secret() || reads_a_password(b))
    }

    /// The tiled window under the pointer `at`, and whether `at` is over its text; `None`
    /// before the window opens.
    fn window_under(&self, at: Position) -> Option<(WindowId, bool)> {
        let view = self.view.as_ref()?;
        let cell = view.render_state.pixel_grid().cell_at(at.x, at.y);
        let window = self.editor.wheel_window(cell);
        let text = self
            .editor
            .frame()
            .regions(window)
            .map(|regions| regions.text);
        let in_text = cell.zip(text).is_some_and(|((row, column), text)| {
            (text.row..text.row + text.rows).contains(&row)
                && (text.column..text.column + text.columns).contains(&column)
        });
        Some((window, in_text))
    }

    /// The cell of the terminal `buffer`'s screen under `at` in `window`, kept inside its
    /// text and its screen as a drag past them is, and `at` in pixels from the screen's top
    /// left, at least 0: `(row, column, x, y)`.
    pub(super) fn cell_in(&self, window: WindowId, buffer: BufferId, at: Position) -> Option<At> {
        let view = self.view.as_ref()?;
        let text = self.editor.frame().regions(window)?.text;
        let size = self.terminals.map.get(buffer)?.grid.size()?;
        let metrics = view.render_state.cell_metrics();
        let [left, top] = (view.render_state.pixel_grid()).cell(
            i64::try_from(text.row).ok()?,
            i64::try_from(text.column).ok()?,
        );
        let x = (at.x - f64::from(left)).max(0.0);
        let y = (at.y - f64::from(top)).max(0.0);
        let rows = text.rows.min(usize::from(size.rows));
        let columns = text.columns.min(usize::from(size.columns));
        let index = |pixels: f64, per: f32, cells: usize| {
            let cell = whole(pixels / f64::from(per).max(1.0));
            u16::try_from(cell.min(cells.saturating_sub(1))).unwrap_or(u16::MAX)
        };
        let row = index(y, metrics.height, rows);
        let column = index(x, metrics.width, columns);
        let pixel = |p: f64| u32::try_from(whole(p)).unwrap_or(u32::MAX);
        Some((row, column, pixel(x), pixel(y)))
    }

    /// Sends the program in `buffer` the pointer at `cell` (`cell_in`'s) doing `action`. A
    /// motion goes once per cell.
    pub(super) fn report(
        &mut self,
        buffer: BufferId,
        cell: Option<At>,
        held: Held,
        action: PointerAction,
    ) -> Reported {
        let Some((row, column, x, y)) = cell else {
            return Reported::Skipped;
        };
        let pointer = &mut self.terminals.pointer;
        if action == PointerAction::Motion {
            if pointer.reported == Some((buffer, row, column)) {
                return Reported::Skipped;
            }
            pointer.reported = Some((buffer, row, column));
        }
        let Some(terminal) = (self.terminals.map.get(buffer)).and_then(|h| h.terminal.as_ref())
        else {
            return Reported::Skipped;
        };
        let modifiers = crate::keys::modifiers(held);
        let sent = terminal.send_pointer(Pointer {
            action,
            column,
            row,
            x,
            y,
            modifiers,
        });
        if sent.is_err() {
            self.editor
                .message("The terminal's input is full; mouse dropped");
            return Reported::Full;
        }
        Reported::Sent
    }

    /// Moves the selection's end in the terminal `buffer` to the cell under `at`. Returns
    /// whether that changed what is drawn.
    fn select_to(&mut self, window: WindowId, buffer: BufferId, at: Position) -> bool {
        let Some((row, column, ..)) = self.cell_in(window, buffer, at) else {
            return false;
        };
        let moved = (self.terminals.map.get_mut(buffer))
            .is_some_and(|hosted| hosted.grid.select_to(row, column));
        if moved {
            self.request_redraw();
        }
        moved
    }

    /// Copies the terminal `buffer`'s selection to the kill ring and the OS clipboard, as
    /// `M-w` does: a secret entry while its program reads a password (ADR 0074, ADR 0120 Q17).
    /// A press with no drag selected nothing, and copies nothing.
    fn copy_selection(&mut self, buffer: BufferId) {
        let Some(text) = (self.terminals.map.get(buffer)).and_then(|h| h.grid.selection_text())
        else {
            return;
        };
        let secret = self.reads_password(buffer);
        self.editor.kill_new(&text, secret);
        self.write_clipboard();
    }

    /// Clears the selection of the terminal `buffer`, as a key sent to it does.
    pub(in crate::app) fn clear_terminal_selection(&mut self, buffer: BufferId) {
        if let Some(hosted) = self.terminals.map.get_mut(buffer)
            && hosted.grid.clear_selection()
        {
            self.request_redraw();
        }
    }

    /// Asks the window for the pointer's moves with no button held only while the selected
    /// window's program wants every motion (mode 1003), so that it otherwise costs nothing.
    pub(in crate::app) fn want_pointer_motion(&mut self) {
        let wanted = self.wants_every_motion(self.editor.tiled().0.buffer());
        if wanted != self.terminals.pointer.motion
            && let Some(view) = &self.view
        {
            view.window.set_pointer_motion(wanted);
            self.terminals.pointer.motion = wanted;
        }
    }
}

impl super::Host {
    /// Whether a mouse button is held over oso: a drag or a click goes on, and the agent
    /// channel waits for it as for a key sequence (ADR 0085).
    pub(in crate::app) fn button_held(&self) -> bool {
        self.pointer.pressed.is_some()
    }
}

/// oso-term's button for the platform's.
fn term_button(button: MouseButton) -> Button {
    match button {
        MouseButton::Left => Button::Left,
        MouseButton::Middle => Button::Middle,
        MouseButton::Right => Button::Right,
    }
}

/// `value` rounded down to a whole number, 0 for one below 0 or not finite.
fn whole(value: f64) -> usize {
    if value.is_finite() && value >= 0.0 {
        // In range by the check, and whole by `floor`: the cast saturates past `usize`.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked above"
        )]
        let whole = value.floor() as usize;
        whole
    } else {
        0
    }
}

#[cfg(test)]
mod tests;
