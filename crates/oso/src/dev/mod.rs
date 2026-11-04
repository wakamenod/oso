//! Dev-only tooling behind the `dev` feature: the control socket, `oso ctl`
//! and the timings benchmarks read (ADR 0016, ADR 0023, ADR 0032, ADR 0046).
//! Release builds never contain it, and CI checks this
//! (`just release-safety`).
//!
//! Must not depend on: `app`, `dispatch` or winit; the caller passes in the
//! `send` function that hands requests to the UI thread.

pub mod ctl;
mod png;
mod protocol;
mod server;
mod state;
mod timing;

use std::sync::mpsc;
use std::thread;

use oso_platform::control::{CleanupGuard, ControlDir};
use oso_render::Capture;
use serde_json::json;

pub use protocol::{Reply, UiOp};
pub use server::UiRequest;
pub use state::{get_state, get_text, screenshot_allowed, show_buffer};
pub use timing::Timings;

/// Appears in every binary built with the `dev` feature, and in no other.
/// `just release-safety` looks for it in release binaries.
pub const DEV_BUILD_MARKER: &str = "oso-dev-control-socket";

/// The running control socket, owned by the app.
pub struct DevState {
    dir: ControlDir,
    /// Removes the socket and screenshots when the app exits.
    _cleanup: CleanupGuard,
    next_screenshot: u32,
}

/// Starts the control socket. `send` hands a request to the UI thread and
/// returns false once the UI is gone. Failing to start is logged and leaves
/// the app running without a socket.
pub fn start<S>(send: S) -> Option<DevState>
where
    S: Fn(UiRequest) -> bool + Clone + Send + 'static,
{
    let since = std::time::Instant::now();
    let started = (|| {
        let dir = ControlDir::open()?;
        let listener = dir.bind()?;
        let socket = listener.socket_path();
        let cleanup = listener.cleanup_guard();
        server::spawn(listener, send)?;
        // Probing other instances' sockets may block, so it runs on its own
        // thread rather than delaying the first frame.
        let stale = dir.clone();
        thread::Builder::new()
            .name("oso-control-cleanup".into())
            .spawn(move || stale.remove_stale())?;
        Ok::<_, std::io::Error>((dir, cleanup, socket))
    })();
    match started {
        Ok((dir, cleanup, socket)) => {
            log::info!(
                "{DEV_BUILD_MARKER}: listening on {} (started in {:?})",
                socket.display(),
                since.elapsed()
            );
            Some(DevState {
                dir,
                _cleanup: cleanup,
                next_screenshot: 1,
            })
        }
        Err(e) => {
            log::error!("{DEV_BUILD_MARKER}: not started: {e}");
            None
        }
    }
}

impl DevState {
    /// Finishes a screenshot on a background thread: waits for the GPU,
    /// encodes the PNG and writes it into the control directory, then
    /// replies with its path.
    pub fn finish_screenshot(&mut self, capture: Capture, reply: mpsc::Sender<Reply>) {
        let name = format!("screenshot-{}.png", self.next_screenshot);
        self.next_screenshot += 1;
        let dir = self.dir.clone();
        let spawned = thread::Builder::new()
            .name("oso-screenshot".into())
            .spawn(move || {
                let _ = reply.send(write_screenshot(capture, &dir, &name));
            });
        // On failure the reply sender is dropped, which the client sees as
        // an error.
        if let Err(e) = spawned {
            log::warn!("cannot start the screenshot thread: {e}");
        }
    }
}

fn write_screenshot(capture: Capture, dir: &ControlDir, name: &str) -> Reply {
    let image = capture.wait().map_err(|e| e.to_string())?;
    let png = png::encode(&image);
    let path = dir
        .write_private_file(name, &png)
        .map_err(|e| e.to_string())?;
    let path = path
        .into_os_string()
        .into_string()
        .map_err(|path| format!("the screenshot path is not UTF-8: {path:?}"))?;
    Ok(json!({ "path": path, "width": image.width, "height": image.height }))
}
