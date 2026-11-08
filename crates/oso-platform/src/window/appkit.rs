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
mod gate;
mod handles;
mod keys;
mod runloop;
mod secure;
mod view;

use std::any::Any;
use std::cell::{Cell, OnceCell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use objc2::exception;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSScreen, NSWindow,
};
use objc2_foundation::{
    MainThreadMarker, NSDictionary, NSNumber, NSPoint, NSSize, NSString, NSUserDefaults,
};

use super::{Event, Handler};
use delegates::{AppDelegate, WindowDelegate};
pub use handles::{Control, RenderTarget, Waker, Window};
use runloop::Hooks;
use view::OsoView;

/// The window's first size, in points, as winit's default.
const FIRST_SIZE: NSSize = NSSize::new(800.0, 600.0);

/// Runs the event loop until it ends. `start` makes the handler, given the waker other threads
/// reach it through, before the app finishes launching. Returns when the handler asked to exit;
/// a quit from the system ends the process inside AppKit, after [`Handler::exiting`].
pub fn run<T: 'static, H: Handler<T> + 'static>(start: impl FnOnce(Waker<T>) -> H) {
    let mtm = MainThreadMarker::new().expect("the window layer runs on the main thread");
    let app = NSApplication::sharedApplication(mtm);
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
    // A panic in `exiting` itself: nothing of the handler runs again, its drops included.
    if let Some(payload) = state.panic.take() {
        std::mem::forget(state.dispatch.borrow_mut().take());
        panic::resume_unwind(payload);
    }
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
    UserEvents,
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
            Delivery::UserEvents => {
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
    DiscardComposition,
}

/// An event that came while the handler ran. Only those that keep what the app knows of the
/// window true can come then; they wait for it.
enum Pending {
    Resized,
    ScaleFactorChanged,
    Occluded(bool),
    ModifiersChanged(super::Held),
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
    /// Other threads sent something while the handler ran.
    users_waiting: Cell<bool>,
    exit: Cell<bool>,
    exited: Cell<bool>,
    deadline: Cell<Option<Instant>>,
    /// Whether oso has the IME on, and the cursor's cell the app last gave.
    ime: Cell<gate::ImeGate>,
    /// The loop ended while the handler ran: it hears so once it returns.
    exit_waiting: Cell<bool>,
    /// Whether secure input is wanted, oso is active, its window has the keyboard, and it is
    /// held (ADR 0073).
    secure: Cell<gate::SecureInput>,
    /// The size and the scale the handler was last told, so that it hears only changes.
    reported_size: Cell<(u32, u32)>,
    reported_scale: Cell<f64>,
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
            users_waiting: Cell::new(false),
            exit: Cell::new(false),
            exited: Cell::new(false),
            deadline: Cell::new(None),
            ime: Cell::new(gate::ImeGate::default()),
            exit_waiting: Cell::new(false),
            secure: Cell::new(gate::SecureInput::default()),
            reported_size: Cell::new((0, 0)),
            reported_scale: Cell::new(0.0),
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
        // A pool of its own, so that what a frame or an event autoreleases goes at once.
        let result = autoreleasepool(|_| {
            panic::catch_unwind(AssertUnwindSafe(|| {
                // An Objective-C exception from a call the handler made becomes a panic here,
                // so the panic hook runs as for any other (ADR 0062). Its reason may quote
                // anything, so the message does not carry it.
                if exception::catch(AssertUnwindSafe(|| handler.deliver(&control, delivery)))
                    .is_err()
                {
                    panic!("an Objective-C exception was raised while the app handled an event");
                }
            }))
        });
        drop(dispatch);
        if let Err(payload) = result {
            self.update_secure(gate::SecureInput::release);
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
            Delivery::Window(Event::ModifiersChanged(held)) => Pending::ModifiersChanged(held),
            Delivery::Window(Event::CloseRequested) => Pending::CloseRequested,
            Delivery::Window(Event::RedrawRequested) => {
                self.redraw.set(true);
                return;
            }
            // They wait in the channel; the source is signalled again once the handler returns.
            Delivery::UserEvents => {
                self.users_waiting.set(true);
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
            // One that throws must not drop the rest, as an IME turned off among them.
            if exception::catch(AssertUnwindSafe(|| self.apply(request))).is_err() {
                log::error!("an Objective-C exception was raised by a window request");
            }
        }
        let pending = self.pending.take();
        for event in pending {
            let event = match event {
                Pending::Resized => self.resized_event(),
                Pending::ScaleFactorChanged => Event::ScaleFactorChanged(self.scale_factor()),
                Pending::Occluded(occluded) => Event::Occluded(occluded),
                Pending::ModifiersChanged(held) => Event::ModifiersChanged(held),
                Pending::CloseRequested => Event::CloseRequested,
            };
            self.deliver(Delivery::Window(event));
        }
        if self.users_waiting.take()
            && let Some(hooks) = self.hooks.get()
        {
            hooks.wake.signal();
        }
        if self.exit_waiting.take() {
            self.exiting();
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
                // The state first, so that it holds whatever the input method does next.
                self.update_ime(|ime| ime.set_allowed(allowed));
                if !allowed {
                    // As winit does: the composition is dropped. Committing it instead is
                    // the next step of ADR 0070, with the app's side of it.
                    view.discard_marked();
                }
            }
            Request::DiscardComposition => view.discard_marked(),
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
        // Secure input goes first, whatever else happens, as the process may end next.
        self.update_secure(gate::SecureInput::release);
        if self.panic.borrow().is_some() || self.exited.get() {
            return;
        }
        // Not while the handler runs: it hears it once it returns. The quit guard answers a
        // quit "later" meanwhile, so AppKit does not end the process before (ADR 0070).
        if self.dispatch.try_borrow_mut().is_err() {
            self.exit_waiting.set(true);
            return;
        }
        self.deliver(Delivery::Exiting);
        self.exited.set(true);
    }

    fn ime_allowed(&self) -> bool {
        self.ime.get().allowed()
    }

    /// Changes what secure input depends on, then makes the calls that asks for. The state is
    /// stored before each call, so that a notice coming during one reads it as it will be.
    fn update_secure(&self, change: impl FnOnce(&mut gate::SecureInput)) {
        let mut secure = self.secure.get();
        change(&mut secure);
        self.secure.set(secure);
        // At most an owed release, then the call the change asks for.
        for _ in 0..2 {
            let mut secure = self.secure.get();
            let Some(step) = secure.next() else {
                return;
            };
            secure.made(step, true);
            self.secure.set(secure);
            let accepted = match step {
                gate::SecureStep::Enable => secure::enable(),
                gate::SecureStep::Disable => secure::disable(),
            };
            if !accepted {
                log::warn!("the system refused a change of secure input");
                let mut secure = self.secure.get();
                secure.made(step, false);
                self.secure.set(secure);
                return;
            }
        }
    }

    fn update_ime(&self, change: impl FnOnce(&mut gate::ImeGate)) {
        let mut ime = self.ime.get();
        change(&mut ime);
        self.ime.set(ime);
    }

    /// Other threads sent something.
    fn users_sent(&'static self) {
        self.deliver(Delivery::UserEvents);
    }

    /// The loop is about to wait: the handler hears it, then a frame is drawn if one was asked
    /// for, and the timer is set for the deadline.
    fn before_waiting(&'static self) {
        // A run loop nested in the handler: the wait after the handler returns comes later.
        if self.dispatch.try_borrow().is_err() {
            return;
        }
        self.deliver(Delivery::BeforeWaiting);
        if self.redraw.take() && self.window.get().is_some() {
            self.deliver(Delivery::Window(Event::RedrawRequested));
        }
        let Some(hooks) = self.hooks.get() else {
            return;
        };
        if self.redraw.get() {
            // The frame asked for another: go round once more instead of waiting. A wake-up
            // alone, as no other thread sent anything.
            hooks.wake.wake_up();
        }
        hooks.wake_at(self.deadline.get());
    }

    fn resized_event(&self) -> Event<'static> {
        let (width, height) = self.inner_size();
        Event::Resized { width, height }
    }

    /// Tells the handler the drawable area's size, if it differs from the last it was told.
    fn report_size(&'static self) {
        // Nothing to report while the window is being made: the handler reads it once made.
        if self.window.get().is_none() {
            return;
        }
        let size = self.inner_size();
        if self.reported_size.replace(size) != size {
            self.deliver(Delivery::Window(self.resized_event()));
        }
    }

    /// Tells the handler the scale, and the size it gives, if the scale differs from the last
    /// it was told. AppKit says the backing changed also when a window is set up.
    fn report_scale(&'static self) {
        if self.window.get().is_none() {
            return;
        }
        let scale = self.scale_factor();
        if self.reported_scale.replace(scale) != scale {
            self.deliver(Delivery::Window(Event::ScaleFactorChanged(scale)));
        }
        self.report_size();
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
pub(super) fn pixels(length: f64) -> u32 {
    length.round().max(0.0) as u32
}

/// The height of the screen with the menu bar, in points, which places are measured from.
pub(super) fn primary_screen_height(mtm: MainThreadMarker) -> f64 {
    NSScreen::screens(mtm)
        .firstObject()
        .map_or(0.0, |screen| screen.frame().size.height)
}
