//! The native macOS window layer in place of winit (ADR 0070, stage 2), behind
//! `native-window`: the same loop, window and waker as `shell` gives, over
//! `oso_platform::window`'s, so the rest of the app does not know which one runs.
//!
//! Must not depend on: what an event does. Each one goes to the app's handler for it, and
//! nothing here reads the editor.

use std::sync::Arc;
use std::time::Instant;

use oso_core::frame::PixelRect;
use oso_platform::text::TextSystem;
use oso_platform::window::{self as platform, Control, CursorArea, Event, Handler};
use oso_render::RenderState;

use super::{App, UserEvent};

/// The window's title.
const TITLE: &str = "oso Editor";

/// This layer commits a composition when oso turns the IME off (ADR 0077).
pub(super) const COMMITS_WHEN_TURNED_OFF: bool = true;

/// This layer answers the input method's questions from what the app gives it: where the
/// composition is drawn, and the excerpt of the cursor's line (ADR 0077).
pub(super) const ANSWERS_INPUT_METHOD: bool = true;

/// Runs the event loop until it ends. `start` makes the app, given the waker other threads
/// reach it through, before the window opens. `started` is when the process started.
pub fn run(started: Instant, start: impl FnOnce(Waker) -> App) {
    platform::run(|waker| {
        log::debug!("event loop built {:?} after start", started.elapsed());
        start(Waker(waker))
    });
}

/// Wakes the event loop from another thread with a [`UserEvent`].
#[derive(Clone)]
pub struct Waker(platform::Waker<UserEvent>);

impl Waker {
    /// Sends `event` to the UI thread. `false` once the event loop is gone.
    pub fn send(&self, event: UserEvent) -> bool {
        self.0.send(event)
    }
}

/// The running event loop, lent to the app while it handles an event.
pub(super) struct Loop<'a>(&'a Control);

impl Loop<'_> {
    /// Ends the loop once the app has handled this event.
    pub(super) fn exit(&self) {
        self.0.exit();
    }

    /// Whether [`Loop::exit`] was called.
    pub(super) fn exiting(&self) -> bool {
        self.0.exit_requested()
    }

    /// From now on, the loop sleeps while no event comes. This only says how it waits; it
    /// never blocks.
    pub(super) fn sleep_when_idle(&self) {
        self.0.sleep_when_idle();
    }

    /// From now on, the loop also wakes when `deadline` passes, with no event.
    pub(super) fn wake_at(&self, deadline: Instant) {
        self.0.wake_at(deadline);
    }

    /// Opens the window.
    pub(super) fn open_window(&self) -> Window {
        Window(self.0.open_window(TITLE))
    }
}

/// The OS window. Sizes and places are physical pixels.
pub(super) struct Window(platform::Window);

impl Window {
    /// Sets up the renderer for this window, `width` x `height` pixels, drawing text
    /// with `text_system`.
    pub(super) fn start_renderer(
        &self,
        width: u32,
        height: u32,
        text_system: Box<dyn TextSystem>,
    ) -> RenderState {
        pollster::block_on(RenderState::new(
            Arc::new(self.0.render_target()),
            width,
            height,
            text_system,
        ))
    }

    /// The drawable area's size.
    pub(super) fn inner_size(&self) -> (u32, u32) {
        self.0.inner_size()
    }

    pub(super) fn scale_factor(&self) -> f64 {
        self.0.scale_factor()
    }

    pub(super) fn request_redraw(&self) {
        self.0.request_redraw();
    }

    /// A frame skipped because the window was hidden: drawn once it is visible again.
    pub(super) fn redraw_when_visible(&self) {
        self.0.redraw_when_visible();
    }

    pub(super) fn set_visible(&self, visible: bool) {
        self.0.set_visible(visible);
    }

    /// Asks for the person's attention, without taking the keyboard from another app.
    pub(super) fn request_attention(&self) {
        self.0.request_attention();
    }

    /// Where the window's frame is on the screen. For the restart handoff.
    #[cfg(feature = "dev")]
    pub(super) fn outer_position(&self) -> Option<(i32, i32)> {
        self.0.outer_position()
    }

    #[cfg(feature = "dev")]
    pub(super) fn set_outer_position(&self, x: i32, y: i32) {
        self.0.set_outer_position(x, y);
    }

    /// Asks for a drawable area of `width` x `height`. It is applied once the app has handled
    /// this event, and a `Resized` says what it came to, so this gives no size.
    #[cfg(feature = "dev")]
    pub(super) fn request_inner_size(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.0.request_inner_size(width, height);
        None
    }

    pub(super) fn set_ime_allowed(&self, allowed: bool) {
        self.0.set_ime_allowed(allowed);
    }

    /// Whether the app wants secure input (ADR 0073).
    pub(super) fn set_secure_input(&self, wanted: bool) {
        self.0.set_secure_input(wanted);
    }

    /// Whether secure input is wanted, held by the window, and on in the system, for the
    /// dev socket.
    #[cfg(feature = "dev")]
    pub(super) fn secure_input(&self) -> Option<(bool, bool, bool)> {
        Some(self.0.secure_input())
    }

    /// Whether the view gives the input method a context, for the dev socket.
    #[cfg(feature = "dev")]
    pub(super) fn gives_input_context(&self) -> Option<bool> {
        Some(self.0.gives_input_context())
    }

    /// Drops the composition the input method holds, with no commit.
    pub(super) fn discard_composition(&self) {
        self.0.discard_composition();
    }

    /// Tells the input method where the cursor's cell is, for its candidate window. The
    /// native layer only keeps it, and answers the input method with it when asked.
    pub(super) fn set_ime_cursor_area(&self, area: PixelRect) {
        self.0.set_ime_cursor_area(cursor_area(area));
    }

    /// Tells the input method where the composition `text` is drawn: its first cell, and
    /// where each of its characters is from that cell's left (ADR 0077). Only kept, as the
    /// cursor area is.
    pub(super) fn set_ime_preedit(&self, laid_out: Option<(&str, PixelRect, &[f32])>) {
        let laid_out =
            laid_out.map(|(text, first, positions)| (text, cursor_area(first), positions));
        self.0.set_ime_preedit(laid_out);
    }

    /// Gives the input method the excerpt of the cursor's line, taken from `excerpt`, and
    /// where the cursor is in it in bytes, or none (ADR 0077). Only kept, and read while the
    /// IME is on.
    pub(super) fn set_ime_excerpt(&self, excerpt: &mut String, cursor: Option<usize>) {
        self.0.set_ime_excerpt(excerpt, cursor);
    }

    /// How long the excerpt the window holds is, and where the cursor is in it, for the dev
    /// socket. No text.
    #[cfg(feature = "dev")]
    pub(super) fn ime_excerpt_extent(&self) -> Option<(usize, usize)> {
        self.0.ime_excerpt_extent()
    }
}

impl Handler<UserEvent> for App {
    fn resume(&mut self, control: &Control) {
        self.open_window(&Loop(control));
    }

    fn user_event(&mut self, control: &Control, event: UserEvent) {
        self.handle_user_event(&Loop(control), event);
    }

    fn window_event(&mut self, control: &Control, event: Event<'_>) {
        self.handle_window_event(&Loop(control), event);
    }

    fn before_waiting(&mut self, control: &Control) {
        self.before_waiting(&Loop(control));
    }

    /// The event loop is ending, on every exit. On macOS `applicationWillTerminate` leads
    /// here for a Dock quit or a logout.
    fn exiting(&mut self) {
        self.ending();
    }
}

/// `rect` as the window layer takes a place for the input method.
fn cursor_area(rect: PixelRect) -> CursorArea {
    CursorArea {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}
