//! Dev-only tooling behind the `dev` feature: the control socket and
//! `oso ctl` (ADR 0016, ADR 0023, ADR 0032). Release builds never contain
//! it, and CI checks this (`just release-safety`).

pub mod ctl;
mod png;
mod protocol;
mod server;

use std::sync::mpsc;
use std::thread;

use oso_core::cursor::Cursor;
use oso_platform::control::{CleanupGuard, ControlDir};
use oso_render::Capture;
use serde_json::{Value, json};

pub use server::{Reply, UiOp, UiRequest};

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

/// The result of `get-state`: the cursor and the surface size, if there is
/// a window. M1-7b extends it to buffers and windows.
pub fn state(cursor: &Cursor, surface_size: Option<(u32, u32)>) -> Value {
    json!({
        "cursor": { "position": cursor.position },
        "surface": surface_size.map(|(width, height)| json!({ "width": width, "height": height })),
    })
}

#[cfg(test)]
mod tests {
    use super::state;
    use oso_core::cursor::Cursor;
    use serde_json::json;

    #[test]
    fn state_reports_the_cursor_and_surface() {
        let cursor = Cursor { position: 2 };
        assert_eq!(
            state(&cursor, Some((800, 600))),
            json!({ "cursor": { "position": 2 }, "surface": { "width": 800, "height": 600 } })
        );
        assert_eq!(
            state(&cursor, None),
            json!({ "cursor": { "position": 2 }, "surface": null })
        );
    }
}
