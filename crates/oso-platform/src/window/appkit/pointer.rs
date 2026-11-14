//! The mouse in oso's view, as oso's events: the wheel (ADR 0112), and the buttons, their
//! drags and, while the app asks for them, the pointer's moves (ADR 0120, step 4b-2). Places
//! are in the drawable's pixels from its top left, as the view is flipped.
//!
//! Must not depend on: the run loop's hooks or the delegates; it reports to the state.

use objc2::DefinedClass;
use objc2_app_kit::NSEvent;

use super::Delivery;
use super::keys;
use super::view::OsoView;
use crate::window::{Event, Mouse, MouseAction, Position, Wheel};

/// `otherMouse*`'s button number for the middle button; higher ones report nothing.
const MIDDLE: isize = 2;

impl OsoView {
    /// `scrollWheel:`.
    pub(super) fn wheel(&self, event: &NSEvent) {
        let (x, y) = (event.scrollingDeltaX(), event.scrollingDeltaY());
        let scale = self.scale();
        let wheel = if event.hasPreciseScrollingDeltas() {
            Wheel::Pixels {
                x: x * scale,
                y: y * scale,
            }
        } else {
            #[expect(clippy::cast_possible_truncation, reason = "a wheel's notches are few")]
            Wheel::Lines {
                x: x as f32,
                y: y as f32,
            }
        };
        self.ivars().state.deliver(Delivery::Window(Event::Wheel {
            delta: wheel,
            at: Some(self.place(event)),
        }));
    }

    /// A button pressed or released, or the pointer moved, with or without one held.
    pub(super) fn mouse(&self, event: &NSEvent, action: MouseAction) {
        let mouse = Mouse {
            action,
            at: self.place(event),
            held: keys::held(event.modifierFlags()),
        };
        self.ivars()
            .state
            .deliver(Delivery::Window(Event::Mouse(mouse)));
    }

    /// `otherMouse*`: the middle button's press or release, or its drag; the others report
    /// nothing.
    pub(super) fn other_mouse(&self, event: &NSEvent, action: MouseAction) {
        if event.buttonNumber() == MIDDLE {
            self.mouse(event, action);
        }
    }

    /// Where `event` happened: in the view's own points, from its top left as it is flipped,
    /// then in pixels as the drawable's size is (ADR 0112).
    fn place(&self, event: &NSEvent) -> Position {
        let scale = self.scale();
        let point = self.convertPoint_fromView(event.locationInWindow(), None);
        Position {
            x: point.x * scale,
            y: point.y * scale,
        }
    }
}
