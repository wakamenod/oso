//! The native macOS window layer (ADR 0070, stage 2): `NSApplication`'s run loop, one
//! `NSWindow` and a view of oso's own that takes keys and the input method, reported as
//! `window`'s events to a [`Handler`].
//!
//! Must not depend on: `text`, `control`, `process` or any workspace crate.
//!
//! - Everything runs on the main thread, and the state lives for the rest of the process,
//!   as AppKit calls back until it ends.
//! - The handler is never entered twice. What it asks of the window is queued and done after
//!   it returns, and an event that comes while it runs waits for it.
//! - A panic in the handler stops the loop, and no handler method runs again; `run` then
//!   resumes it. The panic hook has written what is at risk by then (ADR 0062).

mod delegates;
mod keys;
mod runloop;
mod view;

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::c_void;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEvent,
    NSEventModifierFlags, NSEventType, NSRequestUserAttentionType, NSScreen, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString, NSUserDefaults,
};
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};

use super::{CursorArea, Event, Handler};
use delegates::{AppDelegate, WindowDelegate};
use runloop::{Hooks, Wake};
use view::OsoView;

/// The window's first size, in points, as winit's default.
const FIRST_SIZE: NSSize = NSSize::new(800.0, 600.0);

/// Runs the event loop until it ends. `start` makes the handler, given the waker other threads
/// reach it through, before the app finishes launching. Returns when the handler asked to exit;
/// a quit from the system ends the process inside AppKit, after [`Handler::exiting`].
pub fn run<T: 'static, H: Handler<T> + 'static>(start: impl FnOnce(Waker<T>) -> H) {
    let mtm = MainThreadMarker::new().expect("the window layer runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    // No windows in tabs, and held letters repeat (ADR 0070).
    NSWindow::setAllowsAutomaticWindowTabbing(false, mtm);
    hold_letters_to_repeat();

    let state: &'static State = Box::leak(Box::new(State::new(mtm)));
    let hooks = state.hooks.get_or_init(|| Hooks::install(state));
    let (sender, receiver) = mpsc::channel();
    let wake = Arc::clone(&hooks.wake);
    let handler = start(Waker { sender, wake });
    *state.dispatch.borrow_mut() = Some(Box::new(Adapter { handler, receiver }));

    let delegate = AppDelegate::new(mtm, state);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();

    // The loop stopped: the handler asked to exit, or panicked.
    if let Some(payload) = state.panic.take() {
        panic::resume_unwind(payload);
    }
    state.exiting();
    drop(state.dispatch.borrow_mut().take());
}

/// Registers in memory only that held letters repeat in oso, where the system would offer
/// accented forms. No preference is written, and other apps keep the person's setting.
fn hold_letters_to_repeat() {
    let key = NSString::from_str("ApplePressAndHoldEnabled");
    let value = NSNumber::numberWithBool(false);
    let value: &AnyObject = &value;
    let defaults = NSDictionary::from_slices(&[&*key], &[value]);
    // SAFETY: the dictionary's keys are strings and its values property list objects, as
    // `registerDefaults:` requires.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

/// What a handler can be given. Erases the user event's type, so that the AppKit classes,
/// which cannot be generic, hold one of these.
enum Delivery<'a> {
    Resume,
    Window(Event<'a>),
    /// Other threads sent something; the adapter takes all of it.
    Users,
    BeforeWaiting,
    Exiting,
}

trait Dispatch {
    fn deliver(&mut self, control: &Control, delivery: Delivery<'_>);
}

/// The handler with the end of the channel its user events come through.
struct Adapter<T, H> {
    handler: H,
    receiver: Receiver<T>,
}

impl<T, H: Handler<T>> Dispatch for Adapter<T, H> {
    fn deliver(&mut self, control: &Control, delivery: Delivery<'_>) {
        match delivery {
            Delivery::Resume => self.handler.resume(control),
            Delivery::Window(event) => self.handler.window_event(control, event),
            Delivery::Users => {
                while let Ok(event) = self.receiver.try_recv() {
                    self.handler.user_event(control, event);
                }
            }
            Delivery::BeforeWaiting => self.handler.before_waiting(control),
            Delivery::Exiting => self.handler.exiting(),
        }
    }
}

/// What the handler asked of the window, done once it has returned, since AppKit calls back
/// from inside most of these.
enum Request {
    Visible(bool),
    OuterPosition(i32, i32),
    InnerSize(u32, u32),
    ImeAllowed(bool),
}

/// An event that came while the handler ran. Only those that keep what the app knows of the
/// window true can come then; they wait for it.
enum Pending {
    Resized,
    ScaleFactorChanged,
    Occluded(bool),
    ModifiersCleared,
    CloseRequested,
}

/// The window layer's state, on the main thread, for the rest of the process.
struct State {
    mtm: MainThreadMarker,
    dispatch: RefCell<Option<Box<dyn Dispatch>>>,
    hooks: OnceCell<Hooks>,
    window: OnceCell<(
        Retained<NSWindow>,
        Retained<OsoView>,
        Retained<WindowDelegate>,
    )>,
    requests: RefCell<Vec<Request>>,
    pending: RefCell<Vec<Pending>>,
    redraw: Cell<bool>,
    exit: Cell<bool>,
    exited: Cell<bool>,
    deadline: Cell<Option<Instant>>,
    ime_allowed: Cell<bool>,
    /// The cursor's cell the app last gave, for the input method's candidate window.
    ime_area: Cell<Option<CursorArea>>,
    panic: RefCell<Option<Box<dyn Any + Send>>>,
}

impl State {
    fn new(mtm: MainThreadMarker) -> Self {
        Self {
            mtm,
            dispatch: RefCell::new(None),
            hooks: OnceCell::new(),
            window: OnceCell::new(),
            requests: RefCell::new(Vec::new()),
            pending: RefCell::new(Vec::new()),
            redraw: Cell::new(false),
            exit: Cell::new(false),
            exited: Cell::new(false),
            deadline: Cell::new(None),
            ime_allowed: Cell::new(false),
            ime_area: Cell::new(None),
            panic: RefCell::new(None),
        }
    }

    /// Gives the handler `delivery`, then does what it asked. Nothing reaches a handler that
    /// panicked or has been told the loop is ending.
    fn deliver(&'static self, delivery: Delivery<'_>) {
        if self.panic.borrow().is_some() || self.exited.get() {
            return;
        }
        let Ok(mut dispatch) = self.dispatch.try_borrow_mut() else {
            self.hold(delivery);
            return;
        };
        let Some(handler) = dispatch.as_mut() else {
            return;
        };
        let control = Control { state: self };
        let result = panic::catch_unwind(AssertUnwindSafe(|| handler.deliver(&control, delivery)));
        drop(dispatch);
        if let Err(payload) = result {
            *self.panic.borrow_mut() = Some(payload);
            self.stop();
            return;
        }
        self.after_handler();
    }

    /// Keeps an event that came while the handler ran, if it is one that must not be lost.
    fn hold(&self, delivery: Delivery<'_>) {
        let pending = match delivery {
            Delivery::Window(Event::Resized { .. }) => Pending::Resized,
            Delivery::Window(Event::ScaleFactorChanged(_)) => Pending::ScaleFactorChanged,
            Delivery::Window(Event::Occluded(occluded)) => Pending::Occluded(occluded),
            Delivery::Window(Event::ModifiersChanged(_)) => Pending::ModifiersCleared,
            Delivery::Window(Event::CloseRequested) => Pending::CloseRequested,
            Delivery::Window(Event::RedrawRequested) => {
                self.redraw.set(true);
                return;
            }
            _ => {
                log::debug!("an event came while the handler ran, and was dropped");
                return;
            }
        };
        self.pending.borrow_mut().push(pending);
    }

    /// Does what the handler asked, gives it what waited, and stops the loop if it asked to
    /// exit.
    fn after_handler(&'static self) {
        let requests = self.requests.take();
        for request in requests {
            self.apply(request);
        }
        let pending = self.pending.take();
        for event in pending {
            let event = match event {
                Pending::Resized => self.resized_event(),
                Pending::ScaleFactorChanged => self.scale_event(),
                Pending::Occluded(occluded) => Event::Occluded(occluded),
                Pending::ModifiersCleared => Event::ModifiersChanged(super::Held::NONE),
                Pending::CloseRequested => Event::CloseRequested,
            };
            self.deliver(Delivery::Window(event));
        }
        if self.exit.get() {
            self.stop();
        }
    }

    fn apply(&'static self, request: Request) {
        let Some((window, view, _)) = self.window.get() else {
            return;
        };
        match request {
            Request::Visible(true) => window.makeKeyAndOrderFront(None),
            Request::Visible(false) => window.orderOut(None),
            Request::OuterPosition(x, y) => {
                let scale = window.backingScaleFactor();
                let top = primary_screen_height(self.mtm) - f64::from(y) / scale;
                window.setFrameTopLeftPoint(NSPoint::new(f64::from(x) / scale, top));
            }
            Request::InnerSize(width, height) => {
                let scale = window.backingScaleFactor();
                window.setContentSize(NSSize::new(
                    f64::from(width) / scale,
                    f64::from(height) / scale,
                ));
            }
            Request::ImeAllowed(allowed) => {
                if !allowed {
                    // As winit does: the composition is dropped. Committing it instead is
                    // the next step of ADR 0070, with the app's side of it.
                    view.discard_marked();
                }
                self.ime_allowed.set(allowed);
            }
        }
    }

    /// Stops the loop at the end of the current event, and posts one so that it comes at
    /// once.
    fn stop(&self) {
        let app = NSApplication::sharedApplication(self.mtm);
        app.stop(None);
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(event) = event {
            app.postEvent_atStart(&event, true);
        }
    }

    /// The loop is ending: the handler hears it once.
    fn exiting(&'static self) {
        if self.panic.borrow().is_some() || self.exited.get() {
            return;
        }
        self.deliver(Delivery::Exiting);
        self.exited.set(true);
    }

    /// Other threads sent something.
    fn users_sent(&'static self) {
        self.deliver(Delivery::Users);
    }

    /// The loop is about to wait: the handler hears it, then a frame is drawn if one was asked
    /// for, and the timer is set for the deadline.
    fn before_waiting(&'static self) {
        self.deliver(Delivery::BeforeWaiting);
        if self.redraw.take() && self.window.get().is_some() {
            self.deliver(Delivery::Window(Event::RedrawRequested));
        }
        let Some(hooks) = self.hooks.get() else {
            return;
        };
        if self.redraw.get() {
            // The frame asked for another: go round once more instead of waiting.
            hooks.wake.signal();
        }
        hooks.wake_at(self.deadline.get());
    }

    fn resized_event(&self) -> Event<'static> {
        let (width, height) = self.inner_size();
        Event::Resized { width, height }
    }

    fn scale_event(&self) -> Event<'static> {
        Event::ScaleFactorChanged(self.scale_factor())
    }

    fn inner_size(&self) -> (u32, u32) {
        let Some((window, view, _)) = self.window.get() else {
            return (0, 0);
        };
        let scale = window.backingScaleFactor();
        let size = view.bounds().size;
        (pixels(size.width * scale), pixels(size.height * scale))
    }

    fn scale_factor(&self) -> f64 {
        self.window
            .get()
            .map_or(1.0, |(window, _, _)| window.backingScaleFactor())
    }
}

/// A length in points times the scale, as whole pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a window's size is far inside u32, and never negative"
)]
fn pixels(length: f64) -> u32 {
    length.round().max(0.0) as u32
}

/// The height of the screen with the menu bar, in points, which places are measured from.
fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |screen| screen.frame().size.height)
}

/// The running loop, lent to the handler with each call.
pub struct Control {
    state: &'static State,
}

impl Control {
    /// Ends the loop once the handler returns.
    pub fn exit(&self) {
        self.state.exit.set(true);
    }

    /// Whether [`Control::exit`] was called.
    pub fn exiting(&self) -> bool {
        self.state.exit.get()
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
            window.makeKeyAndOrderFront(None);
            let delegate = WindowDelegate::new(mtm, state);
            window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            (window, view, delegate)
        });
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
    /// and a [`Event::Resized`] says what it came to, so this gives no size.
    pub fn request_inner_size(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        self.state
            .requests
            .borrow_mut()
            .push(Request::InnerSize(width, height));
        None
    }

    pub fn set_ime_allowed(&self, allowed: bool) {
        self.state
            .requests
            .borrow_mut()
            .push(Request::ImeAllowed(allowed));
    }

    /// Where the cursor's cell is, for the input method's candidate window. Only kept: the
    /// input method asks for it when it needs it.
    pub fn set_ime_cursor_area(&self, area: CursorArea) {
        self.state.ime_area.set(Some(area));
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
    sender: Sender<T>,
    wake: Arc<Wake>,
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
    /// Sends `event` to the main thread. `false` once the event loop is gone.
    pub fn send(&self, event: T) -> bool {
        let sent = self.sender.send(event).is_ok();
        if sent {
            self.wake.signal();
        }
        sent
    }
}
