//! The agent sessions benchmarks (ADR 0121, step 3), with a fake `claude`, a shell script the
//! configuration names. `sessions_printing`: 20 hosted sessions printing at once, one shown,
//! and the UI thread's time on each frame meanwhile. `session_state_to_photon`: from the fake
//! printing a permission dialog to the frame whose modeline first counts it. Recorded, with no
//! threshold yet.
//!
//! Must not depend on: anything but the harness's own pacing, from the module above, and its
//! temporary directories, which are removed however the phase ends.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{log_step, us_to_ms};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// How many sessions print at once.
const SESSIONS: usize = 20;
/// How long they print while frames are counted.
const PRINTING: Duration = Duration::from_secs(5);
/// How long a session may take to start, or a state to be found.
const WITHIN: Duration = Duration::from_secs(30);
/// How many dialogs are timed in each run.
const DIALOGS: usize = 5;

/// The fake: it prints a line every 10 ms with `print` in its directory, and the screen a flow
/// writes to `screen` otherwise, each time it is written. `DIR` is the scratch directory.
const FAKE: &str = r#"#!/bin/sh
dir='DIR'
i=0
while :; do
  if [ -f "$dir/print" ]; then
    i=$((i+1)); printf 'line %s of a session that prints a lot of output\n' "$i"; sleep 0.01
  elif [ -f "$dir/screen" ]; then
    printf '\033[2J\033[H'; cat "$dir/screen"; rm -f "$dir/screen"
  else
    sleep 0.05
  fi
done
"#;

const PERMISSION: &str = include_str!("../../../src/sessions/screens/permission-bash.txt");
const WORKING: &str = include_str!("../../../src/sessions/screens/working.txt");

/// Both flows, in new processes, as `sessions_printing_frame_p50_ms`,
/// `sessions_printing_frame_p99_ms` and `session_state_to_photon_ms`.
pub(super) fn sessions(exe: &Path, runs: usize, report: &mut Report) -> Result<(), String> {
    log_step("agent sessions");
    let dir = TempDir::new("sessions")
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let scratch = dir.file("scratch");
    let project = dir.file("proj");
    for made in [&scratch, &project] {
        std::fs::create_dir(made).map_err(|e| format!("{}: {e}", made.display()))?;
    }
    let config = fake(&dir, &scratch)?;
    let mut metrics: [Vec<f64>; 3] = Default::default();
    for _ in 0..runs {
        let notes = project.join("notes.txt");
        std::fs::write(&notes, "notes\n").map_err(|e| format!("{}: {e}", notes.display()))?;
        let mut app = App::start_with_config(exe, Some(&notes), Some(&config))?;
        let frames = printing(&mut app, &scratch)?;
        app.stop();
        metrics[0].push(percentile(&frames, 50.0));
        metrics[1].push(percentile(&frames, 99.0));
        // One session alone, so that the dialog is the one the fake printed.
        let mut app = App::start_with_config(exe, Some(&notes), Some(&config))?;
        app.call("run-command", Some(serde_json::json!({ "name": "claude" })))?;
        wait_for_sessions(&mut app, 1)?;
        metrics[2].push(photon(&mut app, &scratch)?);
        app.stop();
    }
    let [p50, p99, photon] = metrics;
    report.add("sessions_printing_frame_p50_ms", p50);
    report.add("sessions_printing_frame_p99_ms", p99);
    report.add("session_state_to_photon_ms", photon);
    Ok(())
}

/// Writes the fake and a configuration that names it, and gives the configuration's path.
fn fake(dir: &TempDir, scratch: &Path) -> Result<std::path::PathBuf, String> {
    let fake = dir.file("fake-claude");
    let script = FAKE.replace("DIR", &scratch.display().to_string());
    std::fs::write(&fake, script).map_err(|e| format!("{}: {e}", fake.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: {e}", fake.display()))?;
    }
    let config = dir.file("config.toml");
    let text = format!(
        "[terminal]\nshell = \"/bin/sh\"\nlogin = false\n\n[sessions]\nclaude = \"{}\"\n",
        fake.display()
    );
    std::fs::write(&config, text).map_err(|e| format!("{}: {e}", config.display()))?;
    Ok(config)
}

/// Starts [`SESSIONS`] sessions, the last one shown, has them all print for [`PRINTING`], and
/// gives the UI thread's time on each frame meanwhile, in ms.
fn printing(app: &mut App, scratch: &Path) -> Result<Vec<f64>, String> {
    for n in 0..SESSIONS {
        app.call("run-command", Some(serde_json::json!({ "name": "claude" })))?;
        if n > 0 {
            // Another session runs here, so starting one more is asked first, and `y` starts it
            // (ADR 0121 step 4).
            app.send_keys("y")?;
        }
    }
    wait_for_sessions(app, SESSIONS)?;
    let print = scratch.join("print");
    std::fs::write(&print, "").map_err(|e| format!("{}: {e}", print.display()))?;
    // What came before the printing is not counted.
    app.timings()?;
    thread::sleep(PRINTING);
    let timings = app.timings()?;
    std::fs::remove_file(&print).map_err(|e| format!("{}: {e}", print.display()))?;
    let frames: Vec<f64> = timings["ui_frame_us"]
        .as_array()
        .ok_or("no frame times: is this a dev build?")?
        .iter()
        .filter_map(us_to_ms)
        .collect();
    if frames.is_empty() {
        return Err("no frame was drawn while the sessions printed".into());
    }
    Ok(frames)
}

/// Waits until the dev state lists `count` hosted sessions.
fn wait_for_sessions(app: &mut App, count: usize) -> Result<(), String> {
    let deadline = Instant::now() + WITHIN;
    loop {
        let state = app.call("get-state", None)?;
        let hosted = state["sessions"]["sessions"]
            .as_array()
            .map_or(0, |sessions| sessions.len());
        if hosted >= count {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{hosted} of {count} sessions started"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The median, over [`DIALOGS`] dialogs, of the time from the fake printing one to the frame
/// that counts it on the modeline, in ms.
fn photon(app: &mut App, scratch: &Path) -> Result<f64, String> {
    let mut samples = Vec::new();
    let screen = scratch.join("screen");
    let staged = scratch.join("screen.new");
    for _ in 0..DIALOGS {
        // Back to work: no session needs input.
        write_screen(&staged, &screen, WORKING)?;
        wait_for_needs(app, false)?;
        app.timings()?;
        let wrote = unix_us();
        write_screen(&staged, &screen, PERMISSION)?;
        wait_for_needs(app, true)?;
        let timings = app.timings()?;
        let shown = timings["needs_input_frames"]
            .as_array()
            .and_then(|frames| frames.iter().filter_map(serde_json::Value::as_u64).min())
            .ok_or("no frame counted the dialog")?;
        samples.push(shown.saturating_sub(wrote) as f64 / 1000.0);
    }
    Ok(crate::report::median(&samples))
}

/// Has every fake print `text`: the one file each looks at.
fn write_screen(staged: &Path, screen: &Path, text: &str) -> Result<(), String> {
    std::fs::write(staged, text).map_err(|e| format!("{}: {e}", staged.display()))?;
    std::fs::rename(staged, screen).map_err(|e| format!("{}: {e}", screen.display()))
}

/// Waits until a session needs input, or none does.
fn wait_for_needs(app: &mut App, needs: bool) -> Result<(), String> {
    let deadline = Instant::now() + WITHIN;
    loop {
        let state = app.call("get-state", None)?;
        let any = state["sessions"]["sessions"].as_array().is_some_and(|all| {
            all.iter().any(|session| {
                session["state"]
                    .as_str()
                    .is_some_and(|state| state.starts_with("needs input"))
            })
        });
        if any == needs {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("no session became needs input = {needs}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn unix_us() -> u64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
}
