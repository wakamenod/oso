//! The phase that lists a directory of 100,000 names in the directory editor (ADR 0134), and
//! types in another window while it is listed again. Report-only: none of its metrics has a
//! threshold yet.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, and its temporary directories, which are removed however the phase ends.

use std::fs::OpenOptions;
use std::thread;
use std::time::{Duration, Instant};

use super::{LOAD_TIMEOUT, Lcg, log_step, paced_keys, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// How many names the directory holds beside the file the app starts on.
const NAMES: usize = 99_999;

/// What is typed in the file's window while the listing is read again: letters, then as many
/// `DEL`s, so that the file ends as it began.
const KEYS: [&str; 8] = ["t", "y", "p", "e", "DEL", "DEL", "DEL", "DEL"];

/// How long a listing may take to come whole.
const LISTED_WITHIN: Duration = Duration::from_secs(60);

/// How often to look whether it has.
const POLL: Duration = Duration::from_millis(5);

/// The rows of the whole listing: its header, `.` and `..`, every name, the start file, and
/// the empty row after the last line break.
const LINES: u64 = 1 + 2 + NAMES as u64 + 1 + 1;

/// How long a line count must hold to be the listing's own, not a partial one's that has as
/// many rows: longer than the 50 ms between partial listings.
const HOLDS: Duration = Duration::from_millis(80);

/// Lists the directory with `C-x C-j`, as `dired_first_rows_ms` (until the first rows show)
/// and `dired_list_ms` (until every row does), then types in the file's window while the
/// watcher has it listed again, as `dired_relist_type_p99_ms`.
pub(super) fn dired(
    exe: &std::path::Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("listing 100,000 names in the directory editor");
    let dir =
        TempDir::new("dired").map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    for index in 0..NAMES {
        let path = dir.file(&format!("file-{index:05}.txt"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let start = dir.file("start.txt");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&start)
        .map_err(|e| format!("{}: {e}", start.display()))?;
    let count = count & !7;
    let (mut first, mut whole, mut typing) = (Vec::new(), Vec::new(), Vec::new());
    for run in 0..runs {
        let mut app = App::start(exe, Some(&start))?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        app.send_keys("C-x 3")?;
        settle(&mut app)?;
        let asked = Instant::now();
        app.send_keys("C-x C-j")?;
        first.push(until(&mut app, asked, Duration::ZERO, |lines| lines > 4)?);
        whole.push(until(&mut app, asked, HOLDS, |lines| lines == LINES)?);
        // Back to the file, while a new name has the watcher list the directory again.
        app.send_keys("C-x o")?;
        settle(&mut app)?;
        let touched = dir.file(&format!("touched-{run}.txt"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&touched)
            .map_err(|e| format!("{}: {e}", touched.display()))?;
        let mut rng = Lcg(run as u64 + 1);
        let samples = paced_keys(&mut app, &KEYS, count, &mut rng)?;
        typing.push(percentile(&samples, 99.0));
        app.stop();
        std::fs::remove_file(&touched).map_err(|e| format!("{}: {e}", touched.display()))?;
    }
    report.add("dired_first_rows_ms", first);
    report.add("dired_list_ms", whole);
    report.add("dired_relist_type_p99_ms", typing);
    Ok(())
}

/// Milliseconds from `asked` until the selected window shows a listing whose row count `done`
/// takes, and keeps taking for `holds`.
fn until(
    app: &mut App,
    asked: Instant,
    holds: Duration,
    done: impl Fn(u64) -> bool,
) -> Result<f64, String> {
    let mut since: Option<Instant> = None;
    loop {
        let state = app.call("get-state", None)?;
        let buffer = &state["buffer"];
        let listing = buffer["name"]
            .as_str()
            .is_some_and(|name| name.ends_with('/'));
        if listing && buffer["lines"].as_u64().is_some_and(&done) {
            let seen = *since.get_or_insert_with(Instant::now);
            if seen.elapsed() >= holds {
                return Ok(seen.duration_since(asked).as_secs_f64() * 1000.0);
            }
        } else {
            since = None;
        }
        if asked.elapsed() > LISTED_WITHIN {
            return Err(format!(
                "the directory was not listed within {LISTED_WITHIN:?}: {buffer}"
            ));
        }
        thread::sleep(POLL);
    }
}
