//! The startup benchmark: launches with no file, from just before the process starts to its
//! first presented frame (ADR 0025).
//!
//! Must not depend on: anything but the harness's own launching and reading of timings, from
//! the module above.

use std::path::Path;
use std::thread;
use std::time::Duration;

use super::{COLD_FRAME_TIMEOUT, FRAME_TIMEOUT, Scale, gpu_of, has_frame, us_to_ms};
use crate::app::App;
use crate::report::{Gpu, Report};

/// Launches the app with no file. Startup is the time from just before the
/// process started to its first presented frame. A frame skipped while the
/// window is hidden does not count.
pub(super) fn startup(
    exe: &Path,
    scale: &Scale,
    report: &mut Report,
) -> Result<Option<Gpu>, String> {
    let mut gpu = None;
    let mut from_spawn = Vec::new();
    let mut from_main = Vec::new();
    // The first launch of a new binary compiles shaders. It is reported but
    // not what later launches see.
    for run in 0..=scale.startups {
        let mut app = App::start(exe, None)?;
        // Nothing asks the UI thread anything while it starts up.
        thread::sleep(Duration::from_millis(500));
        let timings = if run == 0 {
            app.wait_patiently_until("first frame", COLD_FRAME_TIMEOUT, has_frame)?
        } else {
            app.wait_until("first frame", FRAME_TIMEOUT, has_frame)?
        };
        let spawned = timings["first_frame_unix_us"]
            .as_u64()
            .map(|us| app.spawn_to(us));
        if run == 0 {
            report.add("startup_first_launch_ms", spawned.into_iter().collect());
            gpu = gpu_of(&timings);
        } else {
            from_spawn.extend(spawned);
            from_main.extend(us_to_ms(&timings["startup_us"]));
        }
        app.stop();
    }
    report.add("startup_ms", from_spawn);
    report.add("startup_from_main_ms", from_main);
    Ok(gpu)
}
