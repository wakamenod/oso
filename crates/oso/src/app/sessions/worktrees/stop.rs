//! `worktree-remove-stop` (ADR 0121, "Stopping a removal, as built"): the person stops a
//! running worktree removal, the person's own or an agent's. Before git runs it stops at once
//! and nothing is removed; once git may be deleting, the person is asked first, as the
//! worktree is then left half removed.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads a file: a
//! removal is found by its job's token, and matched to a row by the paths the job holds.

use std::path::Path;

use super::super::super::App;
use crate::git::GRACE;
use crate::sessions::worktree::jobs::{Running, Stop};
use crate::sessions::worktree::porcelain::printable;

impl App {
    /// `worktree-remove-stop`: the removal at point in `*Worktrees*`, else the only one that
    /// runs.
    pub(super) fn stop_removal(&mut self) {
        // A question asked before names nothing now.
        self.hosting.worktrees.stopping = None;
        let Some(jobs) = self.hosting.worktrees.jobs.clone() else {
            return self.editor.message("No worktree removal runs");
        };
        let running = jobs.running_removals();
        let at_point = self.row_at_point().map(|(_, row)| row);
        let target = match pick(at_point.as_ref().map(Option::as_deref), &running) {
            Ok(target) => target.clone(),
            Err(why) => return self.editor.message(why),
        };
        let shown = shown_end(target.path());
        match jobs.stop_removal_before_git(target.id) {
            Stop::Ask => {
                let question = format!(
                    "Stop removing worktree {shown}? git may be deleting: it is left half \
                     removed, maybe without its .git. What git deleted stays deleted (tracked \
                     files are in its branch); remove the rest by hand."
                );
                match self.editor.ask(question, "worktree-remove-stop-confirmed") {
                    Ok(_) => {
                        let path = target.path().to_owned();
                        self.hosting.worktrees.stopping = Some((target.id, path));
                    }
                    Err(e) => self.editor.message(e.to_string()),
                }
            }
            stop => self.say_stop(stop, &shown),
        }
    }

    /// The person's `y` to the stop's warning: the removal it named, which may have ended.
    pub(super) fn stop_removal_confirmed(&mut self) {
        let Some((id, path)) = self.hosting.worktrees.stopping.take() else {
            return self.editor.message("No removal waits to be stopped");
        };
        let shown = shown_end(&path);
        let stop = (self.hosting.worktrees.jobs.as_ref())
            .map_or(Stop::Ended, |jobs| jobs.stop_removal_confirmed(id));
        self.say_stop(stop, &shown);
    }

    /// Whether the removal of the job `token` was stopped: the app then clears nothing.
    pub(in crate::app) fn removal_was_stopped(&self, token: u64) -> bool {
        (self.hosting.worktrees.jobs.as_ref()).is_some_and(|jobs| jobs.removal_stopped(token))
    }

    fn say_stop(&mut self, stop: Stop, shown: &str) {
        let said = match stop {
            Stop::Ended => format!("The removal of {shown} has ended"),
            Stop::BeforeGit => {
                format!("Stopping the removal of {shown}: git has not run, so nothing is removed")
            }
            Stop::InGit | Stop::Ask => format!(
                "Stopping the removal of {shown}: SIGTERM now, SIGKILL in {} s",
                GRACE.as_secs()
            ),
        };
        self.editor.message(said);
    }
}

/// `path` for a message, its end kept: two long worktree paths differ there.
fn shown_end(path: &Path) -> String {
    const KEEP: usize = 120;
    let whole = path.display().to_string();
    let chars: Vec<char> = whole.chars().collect();
    let kept = if chars.len() > KEEP {
        format!(
            "…{}",
            chars[chars.len() - KEEP..].iter().collect::<String>()
        )
    } else {
        whole
    };
    printable(&kept, 200)
}

/// The removal `worktree-remove-stop` takes of those `running`, with `at_point` the row at
/// point when `*Worktrees*` is selected (`Some(None)` off any row): that row's removal, else
/// the only one that runs.
fn pick<'a>(
    at_point: Option<Option<&Path>>,
    running: &'a [Running],
) -> Result<&'a Running, String> {
    if let Some(Some(row)) = at_point {
        return (running.iter())
            .find(|r| r.resolved.as_deref() == Some(row) || r.asked == row)
            .ok_or_else(|| "No removal runs for the worktree at point".to_owned());
    }
    match running {
        [] => Err("No worktree removal runs".to_owned()),
        [only] => Ok(only),
        many => Err(format!(
            "{} worktree removals run: put point on one in *Worktrees* (worktree-list)",
            many.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::pick;
    use crate::sessions::worktree::jobs::{Phase, Running};

    fn running(id: u64, asked: &str, resolved: &str) -> Running {
        Running {
            id,
            asked: PathBuf::from(asked),
            resolved: Some(PathBuf::from(resolved)),
            phase: Phase::InGit,
        }
    }

    #[test]
    fn a_long_path_keeps_its_end() {
        let long = format!("/r/{}/end-of-it", "x".repeat(300));
        let shown = super::shown_end(Path::new(&long));
        assert!(
            shown.starts_with('…') && shown.ends_with("/end-of-it"),
            "{shown}"
        );
    }

    #[test]
    fn the_row_at_point_names_its_removal_by_either_path() {
        let all = [
            running(1, "/var/r/a", "/private/var/r/a"),
            running(2, "/r/b", "/r/b"),
        ];
        let id = |row: &str| pick(Some(Some(Path::new(row))), &all).map(|r| r.id);
        assert_eq!(id("/private/var/r/a"), Ok(1));
        assert_eq!(id("/var/r/a"), Ok(1));
        assert_eq!(id("/r/b"), Ok(2));
    }

    #[test]
    fn a_row_with_no_removal_stops_none_even_with_one_running() {
        let one = [running(1, "/r/a", "/r/a")];
        let picked = pick(Some(Some(Path::new("/r/other"))), &one);
        assert!(picked.unwrap_err().contains("at point"));
    }

    #[test]
    fn off_a_row_it_takes_the_only_one_and_refuses_to_guess_among_several() {
        let one = [running(1, "/r/a", "/r/a")];
        assert_eq!(pick(None, &one).map(|r| r.id), Ok(1));
        assert_eq!(pick(Some(None), &one).map(|r| r.id), Ok(1));
        assert!(
            pick(None, &[])
                .unwrap_err()
                .contains("No worktree removal runs")
        );
        let two = [running(1, "/r/a", "/r/a"), running(2, "/r/b", "/r/b")];
        assert!(
            pick(None, &two)
                .unwrap_err()
                .contains("2 worktree removals run")
        );
    }
}
