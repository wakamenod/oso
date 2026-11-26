//! The benchmark of a pull request reviewed a commit at a time (ADR 0167): a pull request of
//! 200 commits, against a stand-in `gh` a dev build takes from `OSO_DEV_GH`, reviewed with the
//! review menu's `p`: how soon its commits are listed after the review is asked for, and how
//! soon a step with `M-n` shows the next commit's review. Recorded, with no threshold.
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the module
//! above, its temporary directories, which are removed however the phase ends, and git, which
//! it starts through the harness's one place for processes (ADR 0046). A stand-in `gh` is a
//! shell script, so the scenario runs on Unix alone.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::review::{git, hooks_of, trust};
use super::{LOAD_TIMEOUT, log_step, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::report::Report;

/// How many commits the pull request has.
const COMMITS: usize = 200;
/// How many steps each run times.
const STEPS: usize = 5;
/// How long a listing or a step may take.
const WITHIN: Duration = Duration::from_secs(60);
/// How often to look.
const POLL: Duration = Duration::from_millis(5);

/// The metrics the scenario records.
const METRICS: [&str; 2] = ["review_commits_list_ms", "review_commit_step_ms"];

/// Lists and steps through a pull request's commits, in new processes, as
/// `review_commits_list_ms` (from `RET` on the pull request's number to its modeline naming
/// its commits) and `review_commit_step_ms` (from `M-n` to the next commit's review shown read
/// whole, the median of [`STEPS`] a run).
pub(super) fn review_commits(exe: &Path, runs: usize, report: &mut Report) -> Result<(), String> {
    if !cfg!(unix) {
        return Ok(());
    }
    log_step("reviewing a pull request of 200 commits a commit at a time");
    let dir = TempDir::new("review-commits")
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let root = dir.file("repo");
    std::fs::create_dir(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    std::fs::create_dir(hooks_of(&root)).map_err(|e| format!("cannot make hooks: {e}"))?;
    let (base, head) = repository(&root)?;
    let gh = stand_in_gh(&dir.file("bin"), &base, &head)?;
    let config = dir.file("config.toml");
    std::fs::write(&config, "[lsp]\nstart = \"manual\"\n")
        .map_err(|e| format!("{}: {e}", config.display()))?;
    let mut metrics: [Vec<f64>; METRICS.len()] = Default::default();
    for run in 0..runs {
        let state = dir.file(&format!("state-{run}"));
        let env = [
            ("OSO_STATE_DIR", state.as_path()),
            ("OSO_DEV_GH", gh.as_path()),
        ];
        let mut app = App::start_with_env(exe, Some(&root.join("a.txt")), Some(&config), &env)?;
        app.wait_until("loaded file", LOAD_TIMEOUT, |t| {
            t["load"]["done_us"].is_u64()
        })?;
        trust(&mut app)?;
        app.send_keys("C-c a V")?;
        wait_for(&mut app, "the review menu", |state| {
            state["menu"] == "review"
        })?;
        app.send_keys("p 7")?;
        settle(&mut app)?;
        let asked = Instant::now();
        app.send_keys("RET")?;
        let listed = format!("#7 all {COMMITS} commits");
        wait_for(&mut app, "the pull request's commits", |state| {
            state["modeline"]
                .as_str()
                .is_some_and(|modeline| modeline.contains(&listed))
        })?;
        metrics[0].push(asked.elapsed().as_secs_f64() * 1000.0);
        settle(&mut app)?;
        let mut steps = Vec::with_capacity(STEPS);
        for step in 1..=STEPS {
            let asked_us = unix_us();
            let asked = Instant::now();
            app.send_keys("M-n")?;
            let place = format!("#7 {step}/{COMMITS}");
            wait_for(&mut app, "the commit's review", |state| {
                state["modeline"]
                    .as_str()
                    .is_some_and(|modeline| modeline.contains(&place))
            })?;
            until_read(&mut app, asked_us)?;
            steps.push(asked.elapsed().as_secs_f64() * 1000.0);
            settle(&mut app)?;
        }
        steps.sort_by(f64::total_cmp);
        metrics[1].push(steps[STEPS / 2]);
        app.stop();
    }
    for (name, runs) in METRICS.into_iter().zip(metrics) {
        report.add(name, runs);
    }
    Ok(())
}

/// Makes the repository at `root`: `main` at one commit, and `feature` with [`COMMITS`]
/// commits on it, each changing `b.txt`; its base and head commits.
fn repository(root: &Path) -> Result<(String, String), String> {
    let id = [
        "-c",
        "user.name=perf",
        "-c",
        "user.email=perf@example.invalid",
    ];
    let commit = |message: &str| -> Result<(), String> {
        let mut args = id.to_vec();
        args.extend(["commit", "-q", "--no-gpg-sign", "-a", "-m", message]);
        git(root, &args)
    };
    git(root, &["init", "-q", "-b", "main"])?;
    git(
        root,
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    )?;
    write(root, "a.txt", "a\n")?;
    write(root, "b.txt", "0\n")?;
    git(root, &["add", "-A"])?;
    commit("base")?;
    let base = rev(root, "HEAD")?;
    git(root, &["switch", "-q", "-c", "feature"])?;
    let mut text = String::from("0\n");
    for number in 1..=COMMITS {
        text.push_str(&format!("{number}\n"));
        write(root, "b.txt", &text)?;
        commit(&format!("Change {number}"))?;
    }
    let head = rev(root, "HEAD")?;
    // The review compares commits: the work tree is `main`'s, so that none of it shows.
    git(root, &["switch", "-q", "main"])?;
    Ok((base, head))
}

/// The commit `revision` names in the repository at `root`, as git's loose ref of a branch, or
/// `HEAD`'s.
fn rev(root: &Path, revision: &str) -> Result<String, String> {
    let git_dir = root.join(".git");
    let head = std::fs::read_to_string(git_dir.join(revision))
        .map_err(|e| format!("cannot read {revision}: {e}"))?;
    let head = head.trim();
    let id = match head.strip_prefix("ref: ") {
        Some(name) => std::fs::read_to_string(git_dir.join(name))
            .map_err(|e| format!("cannot read {name}: {e}"))?,
        None => head.to_owned(),
    };
    Ok(id.trim().to_owned())
}

fn write(root: &Path, name: &str, text: &str) -> Result<(), String> {
    let path = root.join(name);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// A `gh` in `bin` that answers `pr view` with the pull request #7 from `base` to `head`.
fn stand_in_gh(bin: &Path, base: &str, head: &str) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let sides = format!(
        r#"{{"number":7,"title":"Many","baseRefName":"main","baseRefOid":"{base}","headRefName":"feature","headRefOid":"{head}","isCrossRepository":false}}"#
    );
    std::fs::write(bin.join("sides.json"), sides).map_err(|e| format!("cannot write: {e}"))?;
    let script = format!(
        "#!/bin/sh\ncase \"$1 $2\" in\n'pr view') cat '{}' ;;\n*) exit 1 ;;\nesac\n",
        bin.join("sides.json").display()
    );
    let gh = bin.join("gh");
    std::fs::write(&gh, script).map_err(|e| format!("{}: {e}", gh.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}", gh.display()))?;
    }
    Ok(gh)
}

fn wait_for(app: &mut App, what: &str, done: impl Fn(&Value) -> bool) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let state = app.call("get-state", None)?;
        if done(&state) {
            return Ok(());
        }
        if started.elapsed() > WITHIN {
            return Err(format!("no {what} within {WITHIN:?}: {}", state["echo"]));
        }
        thread::sleep(POLL);
    }
}

/// Waits for a review frame since `asked` that shows a review read whole.
fn until_read(app: &mut App, asked: u64) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let timings = app.timings()?;
        let done = (timings["review_frames"].as_array().into_iter().flatten())
            .any(|frame| frame["unix_us"].as_u64() >= Some(asked) && frame["done"] == true);
        if done {
            return Ok(());
        }
        if started.elapsed() > WITHIN {
            return Err(format!(
                "the commit's review was not read within {WITHIN:?}"
            ));
        }
        thread::sleep(POLL);
    }
}

/// Microseconds since the Unix epoch, as the review's frames are stamped.
fn unix_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_micros()).unwrap_or(u64::MAX)
        })
}
