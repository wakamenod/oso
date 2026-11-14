//! The wheel over a terminal, for its program (ADR 0120, step 4b-2): reports where the program
//! asked for them, in any window under the pointer, or `↑` and `↓` on the alternate screen
//! (mode 1007) of the selected window; never while a password is read. With Shift held, while
//! oso holds input, or otherwise, the wheel scrolls the scrollback.
//!
//! Must not depend on: how a frame is drawn; `mouse` finds the cell.

use oso_core::buffer::BufferId;
use oso_core::key::{Key, KeyPress, Modifiers, NamedKey};
use oso_core::window::WindowId;
use oso_platform::window::{Held, Position};
use oso_term::{Button, Modes, PointerAction};

use super::super::App;
use super::mouse::Reported;

/// The most wheel notches one wheel event sends a program, whatever the trackpad's speed.
const NOTCHES_MAX: usize = 32;

/// Where the wheel over a terminal goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WheelGoes {
    /// It scrolls the scrollback.
    Scrollback,
    /// Its program asked for reports, and gets one per notch.
    Reports,
    /// `↑` or `↓` per notch (mode 1007).
    Arrows,
}

/// Where the wheel over a terminal whose program has `modes` goes: never to the program while
/// oso is `holding` input, with `shift` held, or while it `reads_a_password` (the author,
/// 2026-11-14). Reports go to any window under the pointer, as the program asked for them;
/// the arrows of mode 1007 only to the `selected` window, as they are keys the person did not
/// type.
fn wheel_goes(
    holding: bool,
    shift: bool,
    modes: Modes,
    selected: bool,
    reads_a_password: bool,
) -> WheelGoes {
    if holding || shift || reads_a_password {
        WheelGoes::Scrollback
    } else if modes.mouse != oso_term::Mouse::Off {
        WheelGoes::Reports
    } else if modes.alternate_screen && modes.alternate_scroll && selected {
        WheelGoes::Arrows
    } else {
        WheelGoes::Scrollback
    }
}

impl App {
    /// The wheel over the terminal shown in `window`: `rows` and `columns` of notches, sent to
    /// its program at the pointer `at` where [`wheel_goes`] says. Returns whether they went;
    /// if not, the wheel scrolls the scrollback.
    pub(in crate::app) fn wheel_to_program(
        &mut self,
        window: WindowId,
        buffer: BufferId,
        rows: isize,
        columns: isize,
        at: Option<Position>,
    ) -> bool {
        let Some(hosted) = self.terminals.map.get(buffer) else {
            return false;
        };
        let goes = wheel_goes(
            self.oso_holds_input(),
            self.modifiers.contains(Held::SHIFT),
            hosted.grid.modes(),
            window == self.editor.tiled().0.id(),
            self.reads_password(buffer),
        );
        match goes {
            WheelGoes::Scrollback => return false,
            WheelGoes::Reports => self.wheel_reports(window, buffer, rows, columns, at),
            WheelGoes::Arrows => self.wheel_arrows(buffer, rows),
        }
        true
    }

    /// The wheel's notches as reports, stopped at the first a full queue drops, said once.
    fn wheel_reports(
        &mut self,
        window: WindowId,
        buffer: BufferId,
        rows: isize,
        columns: isize,
        at: Option<Position>,
    ) {
        let cell = self.cell_in(window, buffer, at.unwrap_or(Position { x: 0.0, y: 0.0 }));
        let held = self.modifiers;
        let notches = [
            (rows, Button::WheelUp, Button::WheelDown),
            (columns, Button::WheelLeft, Button::WheelRight),
        ];
        'notches: for (count, back, on) in notches {
            let button = if count < 0 { back } else { on };
            for _ in 0..count.unsigned_abs().min(NOTCHES_MAX) {
                let press = PointerAction::Press(button);
                if self.report(buffer, cell, held, press) == Reported::Full {
                    break 'notches;
                }
            }
        }
    }

    /// The wheel's `rows` of notches as `↑` or `↓`, which the terminal's thread encodes against
    /// the program's cursor-key mode, as any key; sideways notches send nothing. Like any key
    /// sent to the program, they clear the selection.
    fn wheel_arrows(&mut self, buffer: BufferId, rows: isize) {
        self.clear_terminal_selection(buffer);
        let Some(terminal) = (self.terminals.map.get(buffer)).and_then(|h| h.terminal.as_ref())
        else {
            return;
        };
        let arrow = if rows < 0 {
            NamedKey::Up
        } else {
            NamedKey::Down
        };
        let key = KeyPress {
            modifiers: Modifiers::NONE,
            key: Key::Named(arrow),
        };
        for _ in 0..rows.unsigned_abs().min(NOTCHES_MAX) {
            if terminal.send_key(key).is_err() {
                self.editor
                    .message("The terminal's input is full; mouse dropped");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
