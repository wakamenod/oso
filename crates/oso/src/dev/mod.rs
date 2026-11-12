//! Dev-only tooling behind the `dev` feature: the control socket, `oso ctl`
//! `oso dev` and the timings benchmarks read (ADR 0016, ADR 0023, ADR 0032,
//! ADR 0046). Release builds never contain it, and CI checks this
//! (`just release-safety`).
//!
//! Must not depend on: `app`, `dispatch` or winit; the caller passes in the
//! `send` function that hands requests to the UI thread.

mod build_errors;
pub mod ctl;
pub mod handoff;
mod png;
mod protocol;
mod restart;
mod server;
mod state;
pub mod supervisor;
mod timing;
mod watch;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use oso_platform::clipboard::Source;
use oso_platform::control::{CleanupGuard, ControlDir, Socket};
use oso_render::Capture;
use serde_json::json;

pub use protocol::{ClipboardOp, ImeReport, Reply, UiOp, WheelAt};
pub use server::UiRequest;
pub use state::{
    ImeFacts, get_state, get_text, ime_state, screenshot_allowed, show_buffer, wheel_reply,
};
pub use timing::{FrameSpans, HandledKey, SecretWatch, Timings};

/// The option that sends emergency copies elsewhere, so that the end-to-end flows can
/// read them, and never touch the real ones. Only a dev build knows it.
const EMERGENCY_DIR_OPTION: &str = "--emergency-dir";

/// Takes `--emergency-dir DIR` from the front of `args`, if it is there, and returns the
/// rest with the directory. On its own the option stays, and is an unknown option.
pub fn take_emergency_dir(mut args: Vec<OsString>) -> (Vec<OsString>, Option<PathBuf>) {
    if args.len() < 2 || args[0] != EMERGENCY_DIR_OPTION {
        return (args, None);
    }
    let directory = PathBuf::from(args.remove(1));
    args.remove(0);
    (args, Some(directory))
}

/// The option that reads the user configuration from another file, so that the end-to-end
/// flows and benchmarks never read the person's (ADR 0108). Only a dev build knows it.
const CONFIG_OPTION: &str = "--config";

/// Takes `--config FILE` from the front of `args`, if it is there, and returns the rest with
/// the file, made absolute: the agent channel never saves into its directory. On its own
/// the option stays, and is an unknown option.
pub fn take_config_file(mut args: Vec<OsString>) -> (Vec<OsString>, Option<PathBuf>) {
    if args.len() < 2 || args[0] != CONFIG_OPTION {
        return (args, None);
    }
    let file = PathBuf::from(args.remove(1));
    args.remove(0);
    let file = std::path::absolute(&file).unwrap_or(file);
    (args, Some(file))
}

/// The option that gives the editor a clipboard in memory, so that the end-to-end flows
/// never touch the person's (ADR 0074). Only a dev build knows it.
const CLIPBOARD_OPTION: &str = "--clipboard";

/// Takes `--clipboard memory` from the front of `args`, if it is there, and returns the
/// rest with the clipboard to use: the system's without it. Any other value stays, and is
/// an unknown option.
pub fn take_clipboard_option(mut args: Vec<OsString>) -> (Vec<OsString>, Source) {
    if args.len() < 2 || args[0] != CLIPBOARD_OPTION || args[1] != "memory" {
        return (args, Source::System);
    }
    args.drain(..2);
    (args, Source::Memory)
}

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
        let listener = dir.listen(Socket::Dev)?;
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

#[cfg(test)]
mod tests {
    use super::{take_config_file, take_emergency_dir};
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn args(args: &[&str]) -> Vec<OsString> {
        args.iter().map(Into::into).collect()
    }

    #[test]
    fn the_emergency_directory_option_comes_first_or_means_nothing() {
        assert_eq!(
            take_emergency_dir(args(&["--emergency-dir", "/d", "--", "f"])),
            (args(&["--", "f"]), Some(PathBuf::from("/d")))
        );
        assert_eq!(take_emergency_dir(args(&["f"])), (args(&["f"]), None));
        assert_eq!(
            take_emergency_dir(args(&["--emergency-dir"])),
            (args(&["--emergency-dir"]), None)
        );
        assert_eq!(
            take_emergency_dir(args(&["f", "--emergency-dir", "/d"])),
            (args(&["f", "--emergency-dir", "/d"]), None)
        );
    }

    #[test]
    fn the_config_option_comes_first_and_its_file_is_made_absolute() {
        let absolute = if cfg!(windows) {
            r"C:\c\config.toml"
        } else {
            "/c/config.toml"
        };
        assert_eq!(
            take_config_file(args(&["--config", absolute, "--", "f"])),
            (args(&["--", "f"]), Some(PathBuf::from(absolute)))
        );
        let (rest, file) = take_config_file(args(&["--config", "c.toml"]));
        assert!(rest.is_empty());
        let file = file.unwrap();
        assert!(file.is_absolute() && file.ends_with("c.toml"), "{file:?}");
        assert_eq!(
            take_config_file(args(&["f", "--config", "c.toml"])),
            (args(&["f", "--config", "c.toml"]), None)
        );
    }
}
