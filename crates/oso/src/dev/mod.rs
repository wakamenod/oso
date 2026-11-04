//! Dev-only tooling behind the `dev` feature: the control socket and
//! `oso ctl` (ADR 0016, ADR 0023, ADR 0032). Release builds never contain
//! it, and CI checks this (`just release-safety`).
//!
//! Must not depend on: `app`, `dispatch` or winit; the caller passes in the
//! `send` function that hands requests to the UI thread.

pub mod ctl;
mod png;
mod protocol;
mod server;

use std::sync::mpsc;
use std::thread;

use oso_core::buffer::ReadOnly;
use oso_core::editor::Editor;
use oso_core::key::KeyPress;
use oso_core::keymap::display_keys;
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

/// The result of `get-state`: the selected window's buffer, cursor and
/// scroll position, the keys of a pending prefix, and the surface size if
/// there is a window. It never includes buffer text. M1-7b extends it.
pub fn state(editor: &Editor, pending: &[KeyPress], surface_size: Option<(u32, u32)>) -> Value {
    let (window, buffer) = editor.current();
    let display = buffer.display();
    let cursor = window.selections.primary().cursor;
    let point = display.offset_to_display_point(cursor);
    let info = buffer.file_info();
    let size = window.size();
    json!({
        "buffer": {
            "name": buffer.name(),
            "path": buffer.path().map(|path| path.to_string_lossy()),
            "read_only": buffer.read_only().map(ReadOnly::as_str),
            "line_ending": info.line_ending.as_str(),
            "bom": info.bom,
            "invalid_bytes": buffer.invalid_bytes().len(),
            "bytes": buffer.text().len(),
            "lines": display.row_count(),
            "loading": buffer.loading().map(|progress| json!({
                "read": progress.read,
                "total": progress.total,
            })),
        },
        "cursor": {
            "offset": cursor,
            "row": point.row,
            "column": point.column,
            "count": window.selections.len(),
        },
        "scroll": { "top": window.scroll.top, "left": window.scroll.left },
        "window": { "rows": size.rows, "columns": size.columns },
        "keys": { "pending": display_keys(pending).to_string() },
        "surface": surface_size.map(|(width, height)| json!({ "width": width, "height": height })),
    })
}

#[cfg(test)]
mod tests {
    use super::state;
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::key::KeySequence;
    use oso_core::selection::SelectionSet;
    use oso_core::window::ViewSize;
    use serde_json::json;

    #[test]
    fn state_reports_the_buffer_cursor_scroll_and_surface() {
        let mut editor = Editor::with_buffer(Buffer::from_bytes("bad.txt", b"a\r\n\xffSECRET"));
        editor.set_view_size(ViewSize::new(5, 2));
        editor.current_mut().0.selections = SelectionSet::new(6);
        editor.follow_cursor();
        let state = state(&editor, &[], Some((800, 600)));
        assert_eq!(
            state,
            json!({
                "buffer": {
                    "name": "bad.txt",
                    "path": null,
                    "read_only": "invalid-utf8",
                    "line_ending": "crlf",
                    "bom": false,
                    "invalid_bytes": 1,
                    "bytes": 12,
                    "lines": 2,
                    "loading": null,
                },
                "cursor": { "offset": 6, "row": 1, "column": 4, "count": 1 },
                "scroll": { "top": 0, "left": 3 },
                "window": { "rows": 5, "columns": 2 },
                "keys": { "pending": "" },
                "surface": { "width": 800, "height": 600 },
            })
        );
        assert!(!state.to_string().contains("SECRET"), "no buffer text");
        let pending: KeySequence = "C-x 4".parse().unwrap();
        let state = super::state(&Editor::default(), &pending.0, None);
        assert_eq!(state["keys"]["pending"], "C-x 4");
        assert_eq!(state["buffer"]["name"], "*scratch*");
        assert_eq!(state["surface"], json!(null));
    }
}
