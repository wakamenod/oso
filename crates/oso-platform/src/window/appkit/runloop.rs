//! The main run loop's hooks: a source other threads signal to wake it, an observer for the
//! moment before it waits, and a timer for a deadline. All three are added in the common
//! modes, so they also run during a live resize and while a quit waits in the modal panel
//! mode (ADR 0070).
//!
//! Must not depend on: the view or the window; each hook only calls back into the state.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Instant;

use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFIndex, CFRetained, CFRunLoop, CFRunLoopActivity, CFRunLoopObserver,
    CFRunLoopObserverContext, CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopTimer,
    CFRunLoopTimerContext, kCFRunLoopCommonModes,
};

use super::State;

/// What wakes the main run loop from another thread.
pub(super) struct Wake {
    source: CFRetained<CFRunLoopSource>,
    main: CFRetained<CFRunLoop>,
    /// The loop has stopped, or its handler panicked: nothing sent reaches it any more. A
    /// sender holds it shared while it queues, so that a close waits for the queueing.
    closed: RwLock<bool>,
}

// SAFETY: Core Foundation's run loop functions are thread-safe; `CFRunLoopSourceSignal` and
// `CFRunLoopWakeUp` are made to be called from other threads. Nothing else of the source or
// the run loop is touched through this type.
unsafe impl Send for Wake {}
// SAFETY: as for `Send`: the only calls through a shared `Wake` are those two.
unsafe impl Sync for Wake {}

impl Wake {
    /// Marks the source as ready and wakes the main run loop, which then calls the source's
    /// perform callback on the main thread.
    pub(super) fn signal(&self) {
        self.source.signal();
        self.main.wake_up();
    }

    /// Nothing sent from now on reaches the handler. Waits for a send under way.
    pub(super) fn close(&self) {
        *self.closed.write().unwrap_or_else(PoisonError::into_inner) = true;
    }

    /// Runs `queue` unless the loop has stopped, and the loop does not stop meanwhile. Returns
    /// whether it ran and queued.
    pub(super) fn while_open(&self, queue: impl FnOnce() -> bool) -> bool {
        let closed = self.closed.read().unwrap_or_else(PoisonError::into_inner);
        !*closed && queue()
    }

    /// Wakes the main run loop with nothing to deliver, so that it goes round once more.
    pub(super) fn wake_up(&self) {
        self.main.wake_up();
    }
}

/// The hooks of the main run loop, all calling back into `state`.
pub(super) struct Hooks {
    pub(super) wake: Arc<Wake>,
    timer: CFRetained<CFRunLoopTimer>,
    /// The deadline the timer was last set for, once it has been set.
    deadline: Cell<Option<Option<Instant>>>,
    /// Kept so that the observer lives as long as the hooks.
    _observer: CFRetained<CFRunLoopObserver>,
}

impl Hooks {
    /// Adds the hooks to the main run loop. `state` lives for the rest of the process, as
    /// the callbacks may come at any time until it ends.
    pub(super) fn install(state: &'static State) -> Self {
        let info = ptr::from_ref(state).cast_mut().cast::<c_void>();
        let main = CFRunLoop::main().expect("a process has a main run loop");
        // SAFETY: `kCFRunLoopCommonModes` is a constant Core Foundation defines.
        let common = unsafe { kCFRunLoopCommonModes };

        let mut source_context = CFRunLoopSourceContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(perform),
        };
        // SAFETY: the context is a valid version-0 context, copied by the call. Its `info`
        // is a `&'static State`, which outlives the source.
        let source = unsafe { CFRunLoopSource::new(None, 0, &mut source_context) }
            .expect("a run loop source can be made");
        main.add_source(Some(&source), common);

        let mut observer_context = CFRunLoopObserverContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
        };
        // SAFETY: as for the source; `before_waiting` has the callback's signature. The last
        // order runs it after AppKit's own observers.
        let observer = unsafe {
            CFRunLoopObserver::new(
                None,
                CFRunLoopActivity::BeforeWaiting.0,
                true,
                CFIndex::MAX,
                Some(before_waiting),
                &mut observer_context,
            )
        }
        .expect("a run loop observer can be made");
        main.add_observer(Some(&observer), common);

        let mut timer_context = CFRunLoopTimerContext {
            version: 0,
            info,
            retain: None,
            release: None,
            copyDescription: None,
        };
        // SAFETY: as for the source; `fired` has the callback's signature. The timer repeats
        // never on its own: its next date is set before each wait.
        let timer = unsafe {
            CFRunLoopTimer::new(
                None,
                far_future(),
                f64::MAX,
                0,
                0,
                Some(fired),
                &mut timer_context,
            )
        }
        .expect("a run loop timer can be made");
        main.add_timer(Some(&timer), common);

        Self {
            wake: Arc::new(Wake {
                source,
                main,
                closed: RwLock::new(false),
            }),
            timer,
            deadline: Cell::new(None),
            _observer: observer,
        }
    }

    /// Sets when the loop wakes with no event: at `deadline`, or never.
    pub(super) fn wake_at(&self, deadline: Option<Instant>) {
        // Before most waits it has not changed, and moving the timer is not free.
        if self.deadline.replace(Some(deadline)) == Some(deadline) {
            return;
        }
        let date = match deadline {
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(Instant::now());
                CFAbsoluteTimeGetCurrent() + wait.as_secs_f64()
            }
            None => far_future(),
        };
        self.timer.set_next_fire_date(date);
    }
}

/// A date the timer never reaches: about 300 years from now.
fn far_future() -> f64 {
    CFAbsoluteTimeGetCurrent() + 1e10
}

/// The state behind a callback's `info`.
///
/// # Safety
///
/// `info` must be the `&'static State` the hooks were installed with, and the call must come
/// on the main thread, as `State` is not `Sync`: the hooks are added to the main run loop
/// only, which calls them there.
unsafe fn state(info: *mut c_void) -> &'static State {
    // SAFETY: the caller says it is a `&'static State`.
    unsafe { &*info.cast::<State>() }
}

/// The wake source fired: another thread sent something.
unsafe extern "C-unwind" fn perform(info: *mut c_void) {
    // SAFETY: `info` is the state the source was made with.
    unsafe { state(info) }.users_sent();
}

/// The loop is about to wait.
unsafe extern "C-unwind" fn before_waiting(
    _observer: *mut CFRunLoopObserver,
    _activity: CFRunLoopActivity,
    info: *mut c_void,
) {
    // SAFETY: `info` is the state the observer was made with.
    unsafe { state(info) }.before_waiting();
}

/// The deadline came: the loop is awake, and the observer runs before it waits again.
unsafe extern "C-unwind" fn fired(_timer: *mut CFRunLoopTimer, _info: *mut c_void) {}
