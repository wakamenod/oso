//! What the handler and other threads hold of the window layer: the running loop lent with
//! each call, the window, the target the renderer draws into, and the waker.
//!
//! Must not depend on: the delegates or the run loop's hooks; what these ask of AppKit
//! goes through the state, or waits there until the handler returns.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Instant;

use objc2::MainThreadOnly;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSRequestUserAttentionType, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSString};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use super::delegates::WindowDelegate;
use super::runloop::Wake;
use super::view::OsoView;
use super::{FIRST_SIZE, Request, State, primary_screen_height};
use crate::window::CursorArea;

/// The running loop, lent to the handler with each call.
pub struct Control {
    pub(super) state: &'static State,
}

impl Control {
    /// Ends the loop once the handler returns.
    pub fn exit(&self) {
        self.state.exit.set(true);
    }

    /// Whether [`Control::exit`] was called.
    pub fn exit_requested(&self) -> bool {
        self.state.exit.get()
    }

    /// The app's answer to an end it held with `QuitAnswer::Later`: yes ends the process, as
    /// for `QuitAnswer::Now`, once the handler returns. With no end held, nothing happens.
    pub fn answer_quit(&self, yes: bool) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::AnswerQuit(yes));
    }

    /// From now on, the loop sleeps while no event comes.
    pub fn sleep_when_idle(&self) {
        self.state.deadline.set(None);
    }

    /// From now on, the loop also wakes when `deadline` passes, with no event.
    pub fn wake_at(&self, deadline: Instant) {
        self.state.deadline.set(Some(deadline));
    }

    /// Opens the window, titled `title`. There is one; a second call gives the same.
    pub fn open_window(&self, title: &str) -> Window {
        let state = self.state;
        state.window.get_or_init(|| {
            let mtm = state.mtm;
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), FIRST_SIZE);
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable;
            // SAFETY: the window is not released when closed (below), as one made outside a
            // window controller must not be.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            // SAFETY: the state keeps the window for the rest of the process.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(title));
            window.setRestorable(false);
            let view = OsoView::new(mtm, state, frame);
            window.setContentView(Some(&view));
            window.makeFirstResponder(Some(&view));
            window.center();
            // The delegate first, so that it hears the window come up, occluded or not.
            let delegate = WindowDelegate::new(mtm, state);
            window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            window.makeKeyAndOrderFront(None);
            (window, view, delegate)
        });
        // As AppKit has them now, whatever notices came before.
        state.read_focus_for_secure_input();
        // The app reads both when it opens the window, so only later changes are reported.
        state.reported_scale.set(state.scale_factor());
        state.reported_size.set(state.inner_size());
        Window { state }
    }
}

/// The OS window, on the main thread. Sizes and places are physical pixels.
pub struct Window {
    state: &'static State,
}

impl Window {
    /// What the renderer draws into, which may go to another thread.
    pub fn render_target(&self) -> RenderTarget {
        let view = self.state.window.get().map(|(_, view, _)| view);
        RenderTarget {
            ns_view: view.map(|view| NonNull::from(&**view).cast()),
        }
    }

    /// The drawable area's size.
    pub fn inner_size(&self) -> (u32, u32) {
        self.state.inner_size()
    }

    pub fn scale_factor(&self) -> f64 {
        self.state.scale_factor()
    }

    /// Asks for a frame, drawn before the loop next waits.
    pub fn request_redraw(&self) {
        self.state.redraw.set(true);
    }

    /// The renderer skipped a frame because the window was hidden by others: it is drawn once
    /// the window is visible, which is looked at again soon, notice or not.
    pub fn redraw_when_visible(&self) {
        let mut frame = self.state.hidden_frame.get();
        frame.skipped();
        self.state.hidden_frame.set(frame);
    }

    pub fn set_visible(&self, visible: bool) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::Visible(visible));
    }

    /// Asks for the person's attention, without taking the keyboard from another app.
    pub fn request_attention(&self) {
        NSApplication::sharedApplication(self.state.mtm)
            .requestUserAttention(NSRequestUserAttentionType::InformationalRequest);
    }

    /// Where the window's frame is, from the top left of the screen with the menu bar.
    pub fn outer_position(&self) -> Option<(i32, i32)> {
        let (window, _, _) = self.state.window.get()?;
        let scale = window.backingScaleFactor();
        let frame = window.frame();
        let top = primary_screen_height(self.state.mtm) - (frame.origin.y + frame.size.height);
        Some((whole(frame.origin.x * scale), whole(top * scale)))
    }

    pub fn set_outer_position(&self, x: i32, y: i32) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::OuterPosition(x, y));
    }

    /// Asks for a drawable area of `width` x `height`. It is applied once the handler returns,
    /// and a resize event says what it came to.
    pub fn request_inner_size(&self, width: u32, height: u32) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::InnerSize(width, height));
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::ImeAllowed(allowed));
    }

    /// Drops the text being composed, with no commit, and tells the input method to drop its
    /// own, as when a buffer that may hold secrets is selected during a composition.
    pub fn discard_composition(&self) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::DiscardComposition);
    }

    /// Whether the app wants secure input, as while a buffer that may hold secrets is
    /// selected (ADR 0073). It is held only while oso is active and the window has the
    /// keyboard as well.
    pub fn set_secure_input(&self, wanted: bool) {
        self.state.update_secure(|secure| secure.set_wanted(wanted));
    }

    /// Whether secure input is wanted, whether this layer holds it, and whether any process
    /// has it on, for the dev socket's check.
    #[cfg(feature = "dev")]
    pub fn secure_input(&self) -> (bool, bool, bool) {
        let secure = self.state.secure.get();
        (
            secure.wanted(),
            secure.held(),
            super::secure::any_enabled(self.state.mtm),
        )
    }

    /// Where the cursor's cell is, for the input method's candidate window. Only kept: the
    /// input method asks for it when it needs it.
    pub fn set_ime_cursor_area(&self, area: CursorArea) {
        self.state.update_ime(|ime| ime.set_area(area));
    }
}

/// A place in pixels, as a whole number.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a place on the screen is far inside i32"
)]
fn whole(length: f64) -> i32 {
    length.round() as i32
}

/// The window's view as a handle for wgpu, which takes it as `Send` and `Sync`. It is a
/// pointer only, used on the main thread.
pub struct RenderTarget {
    ns_view: Option<NonNull<c_void>>,
}

// SAFETY: the target holds a pointer and nothing else. raw-window-handle's AppKit handle may
// be used only on the main thread, and `window_handle` refuses to give it elsewhere. The view
// lives for the rest of the process, in the state.
unsafe impl Send for RenderTarget {}
// SAFETY: as for `Send`.
unsafe impl Sync for RenderTarget {}

impl HasWindowHandle for RenderTarget {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        if MainThreadMarker::new().is_none() {
            return Err(HandleError::Unavailable);
        }
        let ns_view = self.ns_view.ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(ns_view));
        // SAFETY: the view lives for the rest of the process, so as long as the handle.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for RenderTarget {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::AppKit(AppKitDisplayHandle::new());
        // SAFETY: AppKit's display handle holds nothing.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

/// Wakes the event loop from another thread with a user event.
pub struct Waker<T> {
    pub(super) sender: Sender<T>,
    pub(super) wake: Arc<Wake>,
}

impl<T> Clone for Waker<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
        }
    }
}

impl<T> Waker<T> {
    /// Sends `event` to the main thread. `false` once the event loop has stopped or its
    /// handler panicked. `true` means it was queued before then; the loop may still stop
    /// before it takes it, as on an exit asked for meanwhile.
    pub fn send(&self, event: T) -> bool {
        // After a panic or the loop's end, nothing will take it, and a stop waits for this.
        self.wake.while_open(|| {
            let sent = self.sender.send(event).is_ok();
            if sent {
                self.wake.signal();
            }
            sent
        })
    }
}
