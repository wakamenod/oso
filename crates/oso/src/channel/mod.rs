//! The agent channel (ADR 0085): a control socket in every build, off until the person
//! turns it on, through which an agent runs registered commands by their reach and reads
//! the state oso may show it.
//!
//! Must not depend on: `app`, `dispatch` or winit; the caller passes in the function that
//! hands requests to the UI thread.

pub mod protocol;
pub mod question;
pub mod server;
pub mod state;
pub mod wire;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use oso_platform::control::{CleanupGuard, ControlDir, Socket};

pub use server::{Connection, Event, Reply, Request};

/// The running channel, owned by the app. Dropping it stops the channel: its threads end,
/// its connections close and its socket goes.
pub struct Channel {
    dir: ControlDir,
    shared: Arc<server::Shared>,
    /// Taken when the channel stops, by the thread that removes the socket.
    cleanup: Option<CleanupGuard>,
}

impl std::fmt::Debug for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Channel").finish_non_exhaustive()
    }
}

/// Starts the channel on `<pid>-agent.sock`. `send` hands a request to the UI thread and
/// returns false once the UI is gone. Only the person's command calls this, once per
/// channel: the app holds at most one. It checks and binds in the file system, so call it
/// off the UI thread.
pub fn start<S>(send: S) -> io::Result<Channel>
where
    S: Fn(Request) -> bool + Clone + Send + 'static,
{
    let dir = ControlDir::open()?;
    let listener = dir.listen(Socket::Agent)?;
    let cleanup = listener.cleanup_guard();
    let shared = Arc::new(server::Shared::default());
    server::spawn(listener, Arc::clone(&shared), send)?;
    // Probing other instances' sockets may block, so it runs on its own thread.
    let stale = dir.clone();
    thread::Builder::new()
        .name("oso-agent-cleanup".into())
        .spawn(move || stale.remove_stale())?;
    Ok(Channel {
        dir,
        shared,
        cleanup: Some(cleanup),
    })
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        // The accept loop waits for a connection; this one wakes it to see the stop. It sends
        // nothing, so nothing asks the person. Connecting can wait, so it happens on a thread
        // of its own, which then removes the socket.
        let dir = self.dir.clone();
        let cleanup = self.cleanup.take();
        let spawned = thread::Builder::new()
            .name("oso-agent-stop".into())
            .spawn(move || {
                let _ = dir.connect(Socket::Agent, Some(std::process::id()));
                drop(cleanup);
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start the thread that stops it: {e}");
        }
    }
}
