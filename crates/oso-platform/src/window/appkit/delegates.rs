//! The application's and the window's delegates: the app has launched or will end, and the
//! window was asked to close, resized, hidden or lost the keyboard.
//!
//! Must not depend on: the run loop's hooks; each method reports to the state.

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSWindow, NSWindowDelegate, NSWindowOcclusionState,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol};

use super::{Delivery, State, quit};
use crate::window::Event;

pub(super) struct Ivars {
    state: &'static State,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `AppDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OsoAppDelegate"]
    #[ivars = Ivars]
    pub(super) struct AppDelegate;

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: the methods have the signatures AppKit declares.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let app = NSApplication::sharedApplication(self.mtm());
            // Only once launched, as winit does: set before, the window comes up later.
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            // Launched from a terminal, an unbundled app is not in front unless it asks.
            #[expect(
                deprecated,
                reason = "`activate` does not bring an unbundled app forward"
            )]
            app.activateIgnoringOtherApps(true);
            self.ivars().state.deliver(Delivery::Resume);
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _notification: &NSNotification) {
            self.ivars().state.read_focus_for_secure_input();
        }

        #[unsafe(method(applicationDidResignActive:))]
        fn did_resign_active(&self, _notification: &NSNotification) {
            self.ivars().state.read_focus_for_secure_input();
        }

        /// The OS asks to end: a Dock quit, another app's request, or the end of the session.
        /// The app answers, or holds the end for its answer (ADR 0070, ADR 0084).
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().state.quit_requested(quit::quit_reason_now())
        }

        /// The OS's request to end was answered yes: AppKit ends the process after this returns.
        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _notification: &NSNotification) {
            self.ivars().state.exiting();
        }
    }
);

impl AppDelegate {
    pub(super) fn new(mtm: MainThreadMarker, state: &'static State) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { state });
        // SAFETY: NSObject's `init`, with this signature.
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `WindowDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OsoWindowDelegate"]
    #[ivars = Ivars]
    pub(super) struct WindowDelegate;

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for WindowDelegate {}

    // SAFETY: the methods have the signatures AppKit declares.
    unsafe impl NSWindowDelegate for WindowDelegate {
        /// The close button: the app decides, so the window stays (ADR 0059).
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            self.ivars()
                .state
                .deliver(Delivery::Window(Event::CloseRequested));
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, _notification: &NSNotification) {
            self.ivars().state.report_size();
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn did_change_occlusion(&self, _notification: &NSNotification) {
            let state = self.ivars().state;
            let Some((window, _, _)) = state.window.get() else {
                return;
            };
            let visible = window
                .occlusionState()
                .contains(NSWindowOcclusionState::Visible);
            if !visible {
                // Its notice of being visible again brings the frame, as the app asks then.
                let mut frame = state.hidden_frame.get();
                frame.told_hidden();
                state.hidden_frame.set(frame);
            }
            state.deliver(Delivery::Window(Event::Occluded(!visible)));
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _notification: &NSNotification) {
            self.ivars().state.read_focus_for_secure_input();
        }

        /// Keys released elsewhere never reach the window, so no modifier counts as held.
        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _notification: &NSNotification) {
            let state = self.ivars().state;
            state.read_focus_for_secure_input();
            if let Some((_, view, _)) = state.window.get() {
                view.forget_modifiers();
            }
            state.deliver(Delivery::Window(Event::ModifiersChanged(
                crate::window::Held::NONE,
            )));
        }
    }
);

impl WindowDelegate {
    pub(super) fn new(mtm: MainThreadMarker, state: &'static State) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { state });
        // SAFETY: NSObject's `init`, with this signature.
        unsafe { msg_send![super(this), init] }
    }
}
