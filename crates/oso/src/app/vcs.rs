//! Git status buffers in the app (ADR 0127): what their commands ask, carried out on the UI
//! thread, and the texts their threads lay out.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git, reads a
//! file or lays out a text: each status buffer's thread, `oso-vcs`, does, and its text goes in
//! at O(1), with what it replaces freed off the UI thread. What a line names comes from the
//! table built with the text, never from the text.
//!
//! - A status is read when its buffer becomes shown, on `g`, after each staging, and when a
//!   file of its repository is saved while it is shown; nothing runs while it is not shown.
//! - What an agent does runs no git (ADR 0085, ADR 0118): a save it asked for, or a status its
//!   command left shown, only marks the status stale in its modeline, and the person's next `g`
//!   or key in it reads it again.
//! - Restricted projects get no status buffer, and one whose project is no longer trusted runs
//!   nothing more.
//! - A new text keeps each window's point on what it was on (the same hunk's line, hunk, file
//!   or section), and waits while the editor's text must not change.

use std::collections::BTreeSet;
use std::sync::Arc;

use oso_core::buffer::{Acted, BufferId};
use oso_core::editor::{GitRequest, GitShown};
use oso_core::project::ProjectId;

use super::{App, UserEvent};
use crate::commands::review::{MAX_NAME, one_line};
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::layout::{Laid, Row};
use crate::vcs::stage::Target;
use crate::vcs::{Built, Report, Request, View, Worker};

mod agent;
mod commit;
mod discard;
mod draft;
mod remote;
mod requests;
mod runs;
mod sequence;
mod stash;
mod status;

pub(crate) use runs::RunEvent;

/// The most characters of a failure the echo area shows.
const MAX_MESSAGE: usize = 200;

/// What a status buffer's thread sent the UI thread, as its `serial`th start.
pub(crate) struct GitEvent {
    buffer: BufferId,
    serial: u64,
    report: Report,
}

impl std::fmt::Debug for GitEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitEvent")
            .field("serial", &self.serial)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// The status buffers the app keeps.
#[derive(Default)]
pub(super) struct GitStatuses {
    open: Vec<Open>,
    serial: u64,
    generation: u64,
    /// The event being handled is the agent channel's, so what changes is the agent's doing.
    pub(super) by_agent: bool,
    /// The buffer the selected window showed at the last look, which a key typed since was
    /// typed in.
    selected: Option<BufferId>,
    /// The repositories git runs in, and the commits' messages (step 1b).
    commits: commit::Commits,
}

/// A status buffer as the app knows it.
struct Open {
    buffer: BufferId,
    project: ProjectId,
    root: TrustedRoot,
    /// Which start of the thread its reports must come from.
    serial: u64,
    worker: Option<Worker>,
    view: View,
    /// What the text shown is about, line by line.
    built: Arc<Built>,
    /// The newest request's generation, and the one shown.
    asked: u64,
    shown_generation: u64,
    /// Whether a window showed it at the last look.
    shown: bool,
    /// A read was asked while no window showed it.
    stale: bool,
    /// What an agent did made it stale: the person's next key in it reads it.
    agent_stale: bool,
    /// The newest text, held while the editor's text must not change.
    waiting: Option<Box<Laid>>,
    revoked: bool,
}

impl App {
    /// After commands and after each event: takes back the trust of projects no longer
    /// trusted, forgets killed buffers, carries out what git commands asked, reads a status
    /// that became shown, and puts in a text that waited. `key` after the commands of a key, and
    /// `agent` when what changed since the last look is the agent channel's doing.
    pub(super) fn sync_git(&mut self, key: bool, agent: bool) {
        self.revoke_untrusted_git();
        let gone: Vec<usize> = (self.gits.open.iter().enumerate())
            .filter(|(_, open)| self.editor.git_status_of(open.buffer).is_none())
            .map(|(index, _)| index)
            .collect();
        for index in gone.into_iter().rev() {
            let open = self.gits.open.remove(index);
            self.freer.free((open.built, open.waiting));
        }
        for request in self.editor.take_git_requests() {
            self.git_request(request);
        }
        self.forget_closed_pushes();
        self.forget_closed_sequence_questions();
        self.forget_closed_stash_prompts();
        self.sync_commits();
        let agent = agent || self.gits.by_agent;
        // A key is the person's in a status when the status was selected as it was typed.
        let typed_in = self.gits.selected.replace(self.editor.window().buffer());
        for index in 0..self.gits.open.len() {
            let buffer = self.gits.open[index].buffer;
            let shown = self
                .editor
                .windows()
                .any(|window| window.buffer() == buffer);
            let open = &mut self.gits.open[index];
            let seen = Seen {
                was_shown: open.shown,
                shown,
                stale: open.stale,
                agent_stale: open.agent_stale,
                keyed: key && typed_in == Some(buffer),
                agent,
            };
            open.shown = shown;
            match seen.decide() {
                Decision::Read => self.git_ask(index, Ask::Read),
                Decision::Stale => self.mark_git_stale(index),
                Decision::Nothing => {}
            }
        }
        self.put_in_waiting_git();
    }

    fn revoke_untrusted_git(&mut self) {
        for open in &mut self.gits.open {
            if !open.revoked && !self.editor.project_trusted(open.project) {
                open.root.revoke();
                open.worker = None;
                open.revoked = true;
            }
        }
    }

    fn git_request(&mut self, request: GitRequest) {
        match requests::concern(&request) {
            requests::Concern::Commit => self.git_commit_request(request),
            requests::Concern::Remote => self.git_remote_request(request),
            requests::Concern::Sequence => self.git_sequence_request(request),
            requests::Concern::Stash => self.git_stash_request(request),
            requests::Concern::Status => self.git_status_request(request),
        }
    }

    /// Shows the status of `project` in the selected window, read again, making its buffer if
    /// it has none.
    fn open_git_status(&mut self, project: ProjectId) {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        if let Some(index) = self
            .gits
            .open
            .iter()
            .position(|open| open.project == project)
        {
            let buffer = self.gits.open[index].buffer;
            self.editor.show(buffer);
            if self.gits.open[index].revoked {
                self.gits.open[index].root = root;
                self.gits.open[index].revoked = false;
                self.start_git_worker(index);
            }
            self.gits.open[index].shown = true;
            self.git_ask(index, Ask::Read);
            return self.request_redraw();
        }
        let name = self
            .editor
            .project(project)
            .map_or_else(String::new, |project| one_line(project.name(), MAX_NAME));
        let buffer = self
            .editor
            .add_git_status(project, format!("*git: {name}*"));
        self.editor
            .set_internal_text(buffer, "Reading the status…\n");
        self.editor.show(buffer);
        self.gits.open.push(Open {
            buffer,
            project,
            root,
            serial: 0,
            worker: None,
            view: View::default(),
            built: Arc::default(),
            asked: 0,
            shown_generation: 0,
            shown: true,
            stale: false,
            agent_stale: false,
            waiting: None,
            revoked: false,
        });
        let index = self.gits.open.len() - 1;
        self.start_git_worker(index);
        self.git_ask(index, Ask::Read);
        self.request_redraw();
    }

    fn start_git_worker(&mut self, index: usize) {
        self.gits.serial += 1;
        let serial = self.gits.serial;
        let open = &mut self.gits.open[index];
        open.serial = serial;
        open.worker = None;
        let buffer = open.buffer;
        let waker = self.waker.clone();
        let started = Worker::start(open.root.clone(), super::reviews::state(), move |report| {
            waker.send(UserEvent::Git(GitEvent {
                buffer,
                serial,
                report,
            }))
        });
        match started {
            Ok(worker) => open.worker = Some(worker),
            Err(e) => self
                .editor
                .message(format!("Cannot start the status's thread: {e}")),
        }
    }

    /// Asks the status's thread to read, lay out, or run `ops`; while no window shows it, a
    /// read waits until one does.
    fn git_ask(&mut self, index: usize, ask: Ask) {
        // A write to the index while a commit runs would race it for the index's lock (Q8).
        if matches!(ask, Ask::Stage(..) | Ask::Discard(..))
            && self.repo_busy(self.gits.open[index].project)
        {
            return self.editor.message(runs::RUNNING);
        }
        let open = &mut self.gits.open[index];
        if open.revoked || !self.editor.project_trusted(open.project) {
            return self.editor.message(RESTRICTED);
        }
        if matches!(ask, Ask::Read) && !open.shown {
            open.stale = true;
            return;
        }
        if open.agent_stale && matches!(ask, Ask::Read | Ask::Stage(..) | Ask::Discard(..)) {
            open.agent_stale = false;
            self.editor.set_mode_note(open.buffer, String::new());
        }
        self.gits.generation += 1;
        let generation = self.gits.generation;
        open.asked = generation;
        open.stale = false;
        open.view.withheld = withheld(&self.editor, open.root.path());
        let view = open.view.clone();
        let request = match ask {
            Ask::Read => Request::Read { generation, view },
            Ask::Layout => Request::Layout { generation, view },
            Ask::Stage(targets, stage) => Request::Stage {
                generation,
                built: Arc::clone(&open.built),
                targets,
                stage,
                view,
            },
            Ask::Discard(targets) => Request::Discard {
                generation,
                built: Arc::clone(&open.built),
                targets,
                view,
            },
        };
        let asked = open
            .worker
            .as_ref()
            .is_some_and(|worker| worker.ask(request));
        if !asked {
            self.editor
                .message("The status's thread has ended: C-x k, then C-x g again");
        }
    }

    fn git_index(&self, buffer: BufferId) -> Option<usize> {
        self.gits.open.iter().position(|open| open.buffer == buffer)
    }

    /// A status's thread reported.
    pub(super) fn git_reported(&mut self, event: GitEvent) {
        let GitEvent {
            buffer,
            serial,
            report,
        } = event;
        let current = self
            .git_index(buffer)
            .filter(|index| self.gits.open[*index].serial == serial);
        let Some(index) = current else {
            return self.freer.free(report);
        };
        match report {
            Report::Laid { generation, laid } => {
                let open = &mut self.gits.open[index];
                if generation < open.shown_generation {
                    return self.freer.free(laid);
                }
                open.shown_generation = generation;
                let replaced = open.waiting.replace(laid);
                self.freer.free(replaced);
                self.put_in_waiting_git();
            }
            Report::Ran { said } => self.editor.message(one_line(&said, MAX_MESSAGE)),
            Report::Refused { why } => self.editor.message(one_line(&why, MAX_MESSAGE)),
            Report::Failed { why } => {
                let why = one_line(&why, MAX_MESSAGE);
                self.editor.message(format!("git: {why}"));
            }
        }
    }

    /// Puts in each status text that waited, once nothing makes it wait.
    fn put_in_waiting_git(&mut self) {
        if self.editor.reload_waits() {
            return;
        }
        let mut put = false;
        for index in 0..self.gits.open.len() {
            let Some(laid) = self.gits.open[index].waiting.take() else {
                continue;
            };
            self.put_in_git_text(index, *laid);
            put = true;
        }
        if put {
            self.buffers_changed();
            self.request_redraw();
        }
    }

    fn put_in_git_text(&mut self, index: usize, laid: Laid) {
        let Laid {
            text,
            highlights,
            outline,
            built,
        } = laid;
        let open = &self.gits.open[index];
        let buffer = open.buffer;
        let old = Arc::clone(&open.built);
        let lines: Vec<_> = (self.editor.git_points(buffer).into_iter())
            .map(|(window, line)| {
                if old.rows.is_empty() {
                    return (window, first_line(&built));
                }
                (window, built.line_of(&old.place(line)))
            })
            .collect();
        let line = lines
            .first()
            .map_or_else(|| first_line(&built), |(_, line)| *line);
        // Kept, at O(1), so that what the text replaces is freed off the UI thread.
        let old_text =
            (self.editor.buffer(buffer)).map(|shown| (shown.text().clone(), shown.display()));
        let refused = self.editor.set_git_text(
            buffer,
            text,
            GitShown {
                highlights,
                outline,
                line,
                lines,
            },
        );
        let open = &mut self.gits.open[index];
        open.view.forget_gone(&built);
        open.built = built;
        self.freer.free((old, old_text, refused));
    }

    /// A save of `buffer` ended: the status of its repository is read again if it is shown, or,
    /// for a save the person did not start (ADR 0118's record), marked stale.
    pub(super) fn git_saved(&mut self, buffer: BufferId, by: Acted) {
        let Some(path) = self.editor.buffer(buffer).and_then(|b| b.path()) else {
            return;
        };
        let path = path.to_path_buf();
        for index in 0..self.gits.open.len() {
            let open = &self.gits.open[index];
            if open.revoked || !path.starts_with(open.root.path()) {
                continue;
            }
            if by == Acted::Agent {
                self.mark_git_stale(index);
            } else {
                self.git_ask(index, Ask::Read);
            }
        }
    }
}

impl App {
    /// Marks the status at `index` stale, as what an agent did may have changed it, and says so
    /// in its modeline; nothing runs.
    fn mark_git_stale(&mut self, index: usize) {
        let open = &mut self.gits.open[index];
        if !open.agent_stale {
            open.agent_stale = true;
            let note = "stale: g reads it again".to_owned();
            self.editor.set_mode_note(open.buffer, note);
        }
    }
}

/// What a look at a status found, to decide whether it is read.
#[derive(Clone, Copy, Debug)]
struct Seen {
    was_shown: bool,
    shown: bool,
    /// A read was asked while it was not shown.
    stale: bool,
    /// What an agent did made it stale.
    agent_stale: bool,
    /// A key of the person's ran in it.
    keyed: bool,
    /// What changed is the agent's doing.
    agent: bool,
}

/// What a look at a status leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Read,
    Stale,
    Nothing,
}

impl Seen {
    /// A status the person made shown, or one that waited to be shown, is read; one the agent
    /// made shown is only marked stale; one an agent made stale is read on the person's key.
    fn decide(self) -> Decision {
        if !self.shown {
            return Decision::Nothing;
        }
        let became = !self.was_shown || self.stale;
        match (became, self.agent) {
            (true, false) => Decision::Read,
            (true, true) => Decision::Stale,
            (false, false) if self.agent_stale && self.keyed => Decision::Read,
            _ => Decision::Nothing,
        }
    }
}

/// What a status's thread is asked for.
enum Ask {
    Read,
    Layout,
    Stage(Vec<Target>, bool),
    /// Discard these targets, after the person's `y` (step 1c).
    Discard(Vec<Target>),
}

/// The paths, from `root`, of the open buffers under it that may hold secrets, as git names them.
fn withheld(editor: &oso_core::editor::Editor, root: &std::path::Path) -> BTreeSet<Vec<u8>> {
    (editor.buffers())
        .filter(|(_, buffer)| buffer.is_secret())
        .filter_map(|(_, buffer)| buffer.path()?.strip_prefix(root).ok())
        .filter_map(git_bytes)
        .collect()
}

/// A relative path as git writes it: its bytes, with `/` between names.
#[cfg(unix)]
fn git_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    Some(path.as_os_str().as_bytes().to_vec())
}

#[cfg(not(unix))]
fn git_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    let words: Vec<&str> = (path.components())
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    Some(words.join("/").into_bytes())
}

/// Where point goes in a first text: the first section's heading, else the top.
fn first_line(built: &Built) -> usize {
    (built.rows.iter())
        .position(|row| matches!(row, Row::Heading(_)))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
