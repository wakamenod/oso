//! The panic hook: where it happened and the stack, and never the message, which may quote
//! text (ADR 0028, ADR 0051). Outside a command on the UI thread it writes the copies, before
//! unwinding begins, as a `Drop` that panics during unwinding aborts before anything
//! could catch it (ADR 0062).
//!
//! Must not depend on: `app`, `dispatch` or winit; a hook runs in whatever state the panic
//! left, and gets what it needs from what was published.
//!
//! Copies come first and printing second. A terminal nobody reads can block a write to
//! stderr for good (ADR 0055), so nothing else may wait on one. Printing happens on a thread of
//! its own that this waits for only so long, and a failure to print is ignored, as a panic in a
//! hook aborts. A hook that hangs is worse than one that says nothing.

use std::backtrace::Backtrace;
use std::io::{self, Write};
use std::panic;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::{Shared, Why};

/// How long a hook waits for its words to get out, when the process may be going. Resolving
/// a stack in a debug build can take a second or two.
pub(super) const SAY_LIMIT: Duration = Duration::from_secs(5);

/// The name of the thread that prints, whose own panic prints nothing.
const SAY_THREAD: &str = "oso-panic-note";

/// Where a panic happened, which decides what the hook does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// Under the catch at the command boundary: the editor runs on, so nothing is saved
    /// (ADR 0014).
    InCommand,
    /// On the UI thread and nothing will catch it: the process is going.
    OutsideCommand,
    /// Another thread. The loader and the saver catch their own.
    OtherThread,
}

pub(super) fn install(shared: Arc<Shared>) {
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "an unknown place".to_owned(), ToString::to_string);
        on_panic(&shared, location, io::stderr());
    }));
}

pub(super) fn place_of(shared: &Shared) -> Place {
    if thread::current().id() != shared.ui_thread {
        Place::OtherThread
    } else if shared.in_command.load(Ordering::Relaxed) {
        Place::InCommand
    } else {
        Place::OutsideCommand
    }
}

/// What the hook does, printing to `out`. It never sees the panic's message.
pub(super) fn on_panic(shared: &Shared, location: String, out: impl Write + Send + 'static) {
    let place = match place_of(shared) {
        // A second panic under one command comes from a `Drop` during its unwinding. The
        // process aborts then, whatever the boundary would have caught.
        Place::InCommand if shared.panicked_in_command.swap(true, Ordering::Relaxed) => {
            Place::OutsideCommand
        }
        place => place,
    };
    match place {
        Place::InCommand => {
            let text = move || format!("oso: a command panicked at {location}. The editor runs on");
            // The editor runs on, and must not wait for a terminal even a moment.
            say_to(out, text, Duration::ZERO);
        }
        Place::OutsideCommand => {
            let stack = Backtrace::force_capture();
            let report = shared.write_copies(Why::Panic);
            let text = move || format!("oso panicked at {location}\n{stack}\n{report}");
            say_to(out, text, SAY_LIMIT);
        }
        Place::OtherThread => {
            let current = thread::current();
            let name = current.name().unwrap_or("a thread").to_owned();
            if name != SAY_THREAD {
                let stack = Backtrace::force_capture();
                let text = move || format!("oso: {name} panicked at {location}\n{stack}");
                say_to(out, text, SAY_LIMIT);
            }
        }
    }
}

/// Writes what `text` makes to `out` on a thread of its own, as a blocked `out` must hold
/// nothing up and resolving a stack takes a while, and waits for at most `limit`. If no
/// thread can start, nothing is printed: writing here could hang the hook.
pub(super) fn say_to(
    mut out: impl Write + Send + 'static,
    text: impl FnOnce() -> String + Send + 'static,
    limit: Duration,
) {
    let (finished, wait) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(SAY_THREAD.into())
        .spawn(move || {
            let _ = writeln!(out, "{}", text());
            let _ = finished.send(());
        });
    if spawned.is_ok() {
        let _ = wait.recv_timeout(limit);
    }
}
