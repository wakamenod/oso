//! A merge, rebase, cherry-pick, revert or `git am` that stopped, or a bisect under way (ADR
//! 0127, step 3a): what the files git keeps in the repository's own directory say of it, what
//! the head shows, the questions before aborting or skipping, what a step asks for and when it
//! is refused, and the git commands that go on. The code calls these a sequence, as magit does,
//! so that a stopped merge is never taken for a run the person stopped with `C-g`.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; [`judge`] is a pure function
//! of what the caller read, which is untrusted bytes: a file that does not parse is as missing,
//! and nothing here panics or grows past its input.
//!
//! - The order is git's own (`wt-status.c`): a rebase's directory first, then `MERGE_HEAD`,
//!   `CHERRY_PICK_HEAD`, `REVERT_HEAD`, the sequencer's directory, and `BISECT_LOG`.
//! - A bisect is shown only: it has no continue, and `git bisect reset` in a shell ends it.
//! - `C-c C-c` continues each but a bisect, `C-c C-s` skips a rebase's, a pick's, a revert's
//!   or a patch's commit, and `C-c C-k` aborts; skipping and aborting ask first.
//! - A skip or an abort is refused unless what is stopped is still the very one its question
//!   was asked over ([`refusal`]), so that a `y` never runs on what it did not name.
//! - What git or its files gave is escaped on one line in a question, as in the head.

use oso_core::editor::row_text;

use super::history::Commit;

/// The most bytes of one file read: an id, a ref's name or a count, or the start of a todo list.
pub(crate) const MAX_FILE: usize = 4096;

/// What is under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Doing {
    Rebase,
    /// `git am`, applying a mailbox's patches.
    Am,
    Merge,
    CherryPick,
    Revert,
    Bisect,
}

impl Doing {
    /// git's own command for it.
    pub(crate) fn command(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Am => "am",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Bisect => "bisect",
        }
    }

    /// What the echo area and the questions call it.
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Am => "git am",
            other => other.command(),
        }
    }

    /// The head's label for it, as wide as `Upstream: ` where it fits.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Rebase => "Rebasing: ",
            Self::Am => "Applying: ",
            Self::Merge => "Merging:  ",
            Self::CherryPick => "Picking:  ",
            Self::Revert => "Reverting: ",
            Self::Bisect => "Bisecting: ",
        }
    }

    /// Why `step` may not be asked of it, if it may not: a bisect takes none, and a merge
    /// cannot skip.
    pub(crate) fn refuses(self, step: Step) -> Option<String> {
        match (self, step) {
            (Self::Bisect, _) => {
                Some("A bisect takes no step here: git bisect reset in a shell ends it".into())
            }
            (Self::Merge, Step::Skip) => Some(format!(
                "A merge has no skip: {} continues it, {} aborts it",
                Step::Continue.key(),
                Step::Abort.key()
            )),
            _ => None,
        }
    }

    /// The keys the head names for it.
    pub(crate) fn keys(self) -> Option<String> {
        let steps = [Step::Continue, Step::Skip, Step::Abort];
        let named: Vec<String> = (steps.into_iter())
            .filter(|step| self.refuses(*step).is_none())
            .map(|step| format!("{} {}", step.key(), step.verb()))
            .collect();
        (!named.is_empty()).then(|| named.join(", "))
    }
}

/// What a key asks of a stopped run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Continue,
    Skip,
    Abort,
}

impl Step {
    /// The key of a git status that asks it.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Continue => "C-c C-c",
            Self::Skip => "C-c C-s",
            Self::Abort => "C-c C-k",
        }
    }

    /// What its key does, as the head says it.
    fn verb(self) -> &'static str {
        match self {
            Self::Continue => "continues",
            Self::Skip => "skips",
            Self::Abort => "aborts",
        }
    }

    fn flag(self) -> &'static str {
        match self {
            Self::Continue => "--continue",
            Self::Skip => "--skip",
            Self::Abort => "--abort",
        }
    }
}

/// What is under way, as the head shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stop {
    pub(crate) doing: Doing,
    /// The branch a rebase rewrites, from `refs/heads/`, or `None` if it rebases a detached
    /// `HEAD`.
    pub(crate) branch: Option<Vec<u8>>,
    /// The commit a rebase goes onto.
    pub(crate) onto: Option<String>,
    /// The commit it stopped at: the one a rebase or a pick could not apply, the one merged.
    pub(crate) at: Option<String>,
    /// The step it is on, of how many, for a rebase or a mailbox.
    pub(crate) progress: Option<(u64, u64)>,
    /// Where aborting takes `HEAD` back to, when git keeps it.
    pub(crate) orig: Option<String>,
    /// The commits named, as `git log` gave them, which the caller reads for the ids above.
    pub(crate) commits: Vec<Commit>,
}

/// What a step asks for: the step, and what was stopped as it was asked, without the commits
/// read for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Want {
    pub(crate) step: Step,
    pub(crate) stop: Stop,
}

impl Want {
    /// `step` of `stop`, as it is now.
    pub(crate) fn new(step: Step, stop: &Stop) -> Want {
        Want {
            step,
            stop: Stop {
                commits: Vec::new(),
                ..stop.clone()
            },
        }
    }
}

/// Why `want` may not run over `now`, what is stopped just before git would run, if it may
/// not: nothing or something else is stopped, the step does not apply, or, for a skip or an
/// abort, the run moved on since its question.
pub(crate) fn refusal(now: Option<&Stop>, want: &Want) -> Option<String> {
    let noun = want.stop.doing.noun();
    let Some(now) = now else {
        return Some(format!(
            "No {noun} is stopped now: g reads the status again"
        ));
    };
    if now.doing != want.stop.doing {
        return Some(format!(
            "A {} is stopped now, not a {noun}: g reads the status again",
            now.doing.noun()
        ));
    }
    if let Some(why) = want.stop.doing.refuses(want.step) {
        return Some(why);
    }
    (want.step != Step::Continue && !now.same(&want.stop)).then(|| {
        format!(
            "The {noun} moved on since the question: {} asks again",
            want.step.key()
        )
    })
}

/// Why what moves `HEAD` (the commit menu, a pull, a checkout, a pick, a revert) is refused
/// over `stop`, what is stopped, if it is: anything but a bisect (Q6).
pub(crate) fn stopped_refusal(stop: Option<&Stop>) -> Option<String> {
    let stop = stop.filter(|stop| stop.doing != Doing::Bisect)?;
    Some(format!(
        "A {} is stopped here: {} in the status continues it, {} aborts it",
        stop.doing.noun(),
        Step::Continue.key(),
        Step::Abort.key()
    ))
}

impl Stop {
    /// Whether `other` is the same stop, the commits read for either aside.
    pub(crate) fn same(&self, other: &Stop) -> bool {
        (
            self.doing,
            &self.branch,
            &self.onto,
            &self.at,
            self.progress,
            &self.orig,
        ) == (
            other.doing,
            &other.branch,
            &other.onto,
            &other.at,
            other.progress,
            &other.orig,
        )
    }

    /// The ids the caller reads the short ids and subjects of.
    pub(crate) fn ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = [&self.at, &self.onto, &self.orig]
            .into_iter()
            .flatten()
            .map(String::as_str)
            .collect();
        ids.dedup();
        ids
    }

    /// The commit `id` names, as read; the id alone, shortened, if it was not.
    pub(crate) fn named(&self, id: &str) -> (String, Option<&[u8]>) {
        match self.commits.iter().find(|commit| commit.id.starts_with(id)) {
            Some(commit) => (commit.short.clone(), Some(commit.subject.as_slice())),
            None => (id.chars().take(7).collect(), None),
        }
    }

    /// The question before aborting it, on one line.
    pub(crate) fn abort_question(&self) -> String {
        let back = |what: &str| match &self.orig {
            Some(orig) => format!("{what} goes back to {}", self.shown(orig)),
            None => format!("{what} stays where it is"),
        };
        match self.doing {
            Doing::Rebase => {
                let branch = (self.branch.as_deref()).map_or_else(
                    || "HEAD".to_owned(),
                    |b| row_text(&String::from_utf8_lossy(b)),
                );
                let done = match self.progress {
                    Some((at, of)) => format!(" ({at} of {of} so far)"),
                    None => String::new(),
                };
                format!(
                    "Abort the rebase of {branch}? The commits it replayed{done} and what you \
                     resolved go; {}",
                    back(&branch)
                )
            }
            Doing::Merge => {
                let of = (self.at.as_deref())
                    .map_or_else(String::new, |at| format!(" of {}", self.shown(at)));
                format!(
                    "Abort the merge{of}? What you resolved goes, and any change to the files it \
                     touched; HEAD stays where it is"
                )
            }
            Doing::CherryPick | Doing::Revert => format!(
                "Abort the {}? What it made and what you resolved go; {}",
                self.doing.noun(),
                back("HEAD")
            ),
            Doing::Am => {
                "Abort git am? The patches it applied and what you resolved go; HEAD goes back \
                 to where it began"
                    .to_owned()
            }
            Doing::Bisect => String::new(),
        }
    }

    /// The question before skipping the commit it stopped at, on one line.
    pub(crate) fn skip_question(&self) -> String {
        let what = (self.at.as_deref()).map_or_else(
            || "the commit it stopped at".to_owned(),
            |at| self.shown(at),
        );
        match self.doing {
            Doing::Am => "Skip this patch? Its changes and what you resolved in it go".to_owned(),
            _ => format!(
                "Skip {what}? Its changes and what you resolved in it go, and the {} goes on \
                 with the next",
                self.doing.noun()
            ),
        }
    }

    /// `id`'s short id and subject, lossily and escaped on one line, as a question names it.
    fn shown(&self, id: &str) -> String {
        match self.named(id) {
            (short, Some(subject)) => {
                format!("{short} {}", row_text(&String::from_utf8_lossy(subject)))
            }
            (short, None) => short,
        }
    }
}

/// The arguments of `step` for `doing`.
pub(crate) fn args(doing: Doing, step: Step) -> Vec<String> {
    vec![doing.command().to_owned(), step.flag().to_owned()]
}

/// What the log and the echo area call `step` for `doing`.
pub(crate) fn title(doing: Doing, step: Step) -> String {
    format!("git {} {}", doing.command(), step.flag())
}

/// What the echo area says once `step` went well.
pub(crate) fn done(doing: Doing, step: Step) -> String {
    let noun = doing.noun();
    match step {
        Step::Continue => format!("Continued the {noun}"),
        Step::Skip => format!("Skipped a commit of the {noun}"),
        Step::Abort => format!("Aborted the {noun}"),
    }
}

/// What the echo area says when `title` failed with `output`, `stopped` if something is still
/// stopped after it: a conflict says what to do next. Only git's own lines are looked at, not a
/// hook's: those that start as git's conflicts and refusals do.
pub(crate) fn failed(title: &str, output: &str, stopped: bool) -> String {
    let lines = || output.lines().map(str::trim_start);
    let conflict = lines()
        .any(|line| line.starts_with("CONFLICT (") || line.starts_with("error: could not apply"));
    if stopped && conflict {
        return format!(
            "{title} stopped on a conflict: resolve each file, s stages it, {} continues, {} \
             aborts",
            Step::Continue.key(),
            Step::Abort.key()
        );
    }
    let skip = lines().any(|line| {
        (line.starts_with("git ") && line.ends_with(" --skip"))
            || line.ends_with("you might want to skip this patch.")
    });
    if stopped && skip {
        return format!(
            "{title}: nothing is left to commit here, {} skips this commit",
            Step::Skip.key()
        );
    }
    super::remote::failed(title, output)
}

/// What is under way, from what `file` read of the work tree's own directory, by path there
/// (`None` if missing or unread, at most [`MAX_FILE`] bytes), and whether `mark`, a directory
/// or a file, is there; `None` if nothing is.
pub(crate) fn judge(
    file: &dyn Fn(&str) -> Option<Vec<u8>>,
    mark: &dyn Fn(&str) -> bool,
) -> Option<Stop> {
    let id = |name: &str| file(name).as_deref().and_then(first_id);
    let blank = |doing| Stop {
        doing,
        branch: None,
        onto: None,
        at: None,
        progress: None,
        orig: None,
        commits: Vec::new(),
    };
    for (dir, step, of) in [
        ("rebase-merge", "msgnum", "end"),
        ("rebase-apply", "next", "last"),
    ] {
        if !mark(dir) {
            continue;
        }
        let path = |name: &str| format!("{dir}/{name}");
        let am = dir == "rebase-apply" && mark("rebase-apply/applying");
        let count = |name: &str| file(&path(name)).as_deref().and_then(number);
        let progress = count(step).zip(count(of)).filter(|(at, of)| at <= of);
        if am {
            return Some(Stop {
                progress,
                ..blank(Doing::Am)
            });
        }
        let at = id("REBASE_HEAD").or_else(|| id(&path("stopped-sha")));
        return Some(Stop {
            branch: file(&path("head-name")).as_deref().and_then(head_name),
            onto: id(&path("onto")),
            at,
            progress,
            orig: id(&path("orig-head")),
            ..blank(Doing::Rebase)
        });
    }
    // Each head is read once, so that what it says and that it is there agree.
    let picked = |doing, head: Option<&[u8]>| Stop {
        at: head.and_then(first_id),
        orig: id("sequencer/head"),
        ..blank(doing)
    };
    if let Some(head) = file("MERGE_HEAD") {
        return Some(Stop {
            at: first_id(&head),
            ..blank(Doing::Merge)
        });
    }
    if let Some(head) = file("CHERRY_PICK_HEAD") {
        return Some(picked(Doing::CherryPick, Some(&head)));
    }
    if let Some(head) = file("REVERT_HEAD") {
        return Some(picked(Doing::Revert, Some(&head)));
    }
    if mark("sequencer") {
        // Between two picks, with nothing stopped on: the todo list's next command says which.
        let doing = match file("sequencer/todo").as_deref().and_then(first_command) {
            Some(b"revert") => Doing::Revert,
            Some(b"pick" | b"p") => Doing::CherryPick,
            _ => return None,
        };
        return Some(picked(doing, None));
    }
    mark("BISECT_LOG").then(|| blank(Doing::Bisect))
}

/// The first line's id, a whole one of 40 or 64 hex digits, as git writes them there: a short
/// one could name a branch instead.
fn first_id(bytes: &[u8]) -> Option<String> {
    let line = bytes.split(|&b| b == b'\n').next()?.trim_ascii();
    let hex = matches!(line.len(), 40 | 64) && line.iter().all(u8::is_ascii_hexdigit);
    hex.then(|| String::from_utf8_lossy(line).to_ascii_lowercase())
}

/// A count, as git writes one.
fn number(bytes: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(bytes.trim_ascii()).ok()?;
    (text.len() <= 9).then(|| text.parse().ok()).flatten()
}

/// The branch `refs/heads/NAME` names, or `None` for a detached `HEAD`.
fn head_name(bytes: &[u8]) -> Option<Vec<u8>> {
    let line = bytes.split(|&b| b == b'\n').next()?.trim_ascii_end();
    line.strip_prefix(b"refs/heads/")
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
}

/// The first command of a todo list, past comments and blank lines.
fn first_command(bytes: &[u8]) -> Option<&[u8]> {
    (bytes.split(|&b| b == b'\n'))
        .map(<[u8]>::trim_ascii)
        .find(|line| !line.is_empty() && !line.starts_with(b"#"))?
        .split(|b| b.is_ascii_whitespace())
        .next()
}

#[cfg(test)]
mod tests;
