//! The window's size and scale as the handler is told them, and a live resize, whose steps
//! are drawn at once (ADR 0070, ADR 0086).
//!
//! Must not depend on: the run loop's hooks or the delegates, which call in here.

use super::{Delivery, State, pixels};
use crate::window::Event;

impl State {
    pub(super) fn resized_event(&self) -> Event<'static> {
        let (width, height) = self.inner_size();
        Event::Resized { width, height }
    }

    /// Tells the handler the drawable area's size, if it differs from the last it was told.
    pub(super) fn report_size(&'static self) {
        // Nothing to report while the window is being made: the handler reads it once made.
        if self.window.get().is_none() {
            return;
        }
        let size = self.inner_size();
        if self.reported_size.replace(size) == size {
            return;
        }
        self.deliver(Delivery::Window(self.resized_event()));
        // A step of a live resize is drawn now, so that its frame lands in the same Core
        // Animation transaction as the size, and nothing is stretched (ADR 0086). Otherwise
        // the frame comes when the loop waits.
        let mut resize = self.live_resize.get();
        if resize.step() && self.redraw.take() {
            self.deliver(Delivery::Window(Event::RedrawRequested));
            resize.drew();
        }
        self.live_resize.set(resize);
    }

    /// A live resize starts or ends. While one runs, the layer presents each frame inside the
    /// Core Animation transaction that resizes it, which makes the main thread wait for the
    /// frame to be scheduled, so only then.
    pub(super) fn set_live_resize(&self, live: bool) {
        let mut resize = self.live_resize.get();
        if live {
            resize.start();
        } else {
            resize.end();
        }
        self.live_resize.set(resize);
        if let Some((_, view, _)) = self.window.get() {
            view.present_with_transaction(live);
        }
    }

    /// Tells the handler the scale, and the size it gives, if the scale differs from the last
    /// it was told. AppKit says the backing changed also when a window is set up.
    pub(super) fn report_scale(&'static self) {
        if self.window.get().is_none() {
            return;
        }
        let scale = self.scale_factor();
        if self.reported_scale.replace(scale) != scale {
            self.deliver(Delivery::Window(Event::ScaleFactorChanged(scale)));
        }
        self.report_size();
    }

    pub(super) fn inner_size(&self) -> (u32, u32) {
        let Some((window, view, _)) = self.window.get() else {
            return (0, 0);
        };
        let scale = window.backingScaleFactor();
        let size = view.bounds().size;
        (pixels(size.width * scale), pixels(size.height * scale))
    }

    pub(super) fn scale_factor(&self) -> f64 {
        self.window
            .get()
            .map_or(1.0, |(window, _, _)| window.backingScaleFactor())
    }
}
