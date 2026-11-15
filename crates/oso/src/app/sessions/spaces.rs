//! The issue space commands in the app (ADR 0136, step 1b): what `space-new`, `space-add-repo`
//! and the others ask, carried out on the UI thread, with the disk work (a space's directory,
//! whether a path is a repository, the instructions' directory) on the sessions worker, and
//! the lists their prompts complete over.
//!
//! Must not depend on: key presses or how a frame is drawn.

mod members;
mod orchestrator;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::complete::Candidate;
use oso_core::editor::{ListingRequest, SpaceRequest};

use crate::app::App;
use crate::commands::agent_sessions::{ARCHIVED_LIST, MEMBERS_LIST, REPOSITORIES_LIST};
use crate::sessions::spaces::edit::{NotAdded, sessions_to_stop};
use crate::sessions::spaces::probe::Made;
use crate::sessions::spaces::probe::RepoCheck;
use crate::sessions::spaces::stored::MAX_MEMBERS;
use crate::sessions::spaces::stored::{is_plain, now_ms};
use crate::sessions::spaces::{Member, SpaceId};
use crate::sessions::worker::{Job, SpaceJob};

/// What the echo area says while a prompt keeps the frame's tree.
const PROMPT_OPEN: &str = "Close the prompt first";

/// A repository looked at on the worker, and what it is for.
#[derive(Debug)]
enum Pending {
    /// The first repository of a new space for the issue with this title.
    New { title: String },
    /// One more repository of this space.
    Add { space: SpaceId },
}

/// What the space commands wait for.
#[derive(Debug, Default)]
pub(in crate::app) struct SpaceWork {
    /// The repositories being looked at.
    pending: HashMap<RepoCheck, Pending>,
    /// The checks asked for so far.
    checks: u64,
    /// A title `space-new` read with no repository to start from: the next `space-add-repo`
    /// answer makes its space.
    title: Option<String>,
    /// The spaces whose directory is shown once it is made.
    show_directory: HashSet<SpaceId>,
    /// The spaces whose orchestrator starts once their directory is made (step 3).
    pub(in crate::app) start_after: HashSet<SpaceId>,
    /// The sessions being got ready as an orchestrator, by key: their space.
    pub(in crate::app) starting: HashMap<crate::sessions::worker::SessionKey, SpaceId>,
    /// What the question the host asked last offers, which `y` does (`SpaceRequest::Confirmed`):
    /// dropped when another is asked or it is answered otherwise.
    pub(in crate::app) offer: Option<orchestrator::Offer>,
    /// The open spaces' trusted repositories as last seen, to tell one trusted since; none
    /// while the trust list is unread.
    pub(in crate::app) trusted_seen: Option<Vec<(SpaceId, PathBuf)>>,
    /// The repositories each space's orchestrator was given, at its start or pasted since.
    pub(in crate::app) given: HashMap<SpaceId, Vec<PathBuf>>,
    /// The repository `space-use-repo` chose for project commands from each space's
    /// orchestrator's buffer (ADR 0136, step 4).
    pub(in crate::app) used: HashMap<SpaceId, PathBuf>,
    /// The directories each session being started also reads, until it is in the table.
    pub(in crate::app) reads:
        HashMap<crate::sessions::worker::SessionKey, Vec<crate::sessions::spaces::Member>>,
}

impl SpaceWork {
    /// The next look at a repository.
    fn check(&mut self) -> RepoCheck {
        self.checks += 1;
        RepoCheck(self.checks)
    }
}

impl App {
    /// Carries out a space command's request.
    pub(in crate::app) fn space_request(&mut self, request: SpaceRequest) {
        match request {
            SpaceRequest::New(title) => self.new_space(title),
            SpaceRequest::AddRepo(text) => self.add_repository(&text),
            SpaceRequest::RemoveRepo(name) => self.remove_repository(&name),
            SpaceRequest::UseRepo(name) => self.use_repository(&name),
            SpaceRequest::Close => self.close_space(),
            SpaceRequest::Reopen(name) => self.reopen_space(&name),
            SpaceRequest::Rename(title) => self.rename_space(&title),
            SpaceRequest::Instructions => self.space_instructions(),
            SpaceRequest::Directory => self.space_directory(),
            SpaceRequest::StartOrchestrator { remake_directory } => {
                self.start_orchestrator(remake_directory);
            }
            SpaceRequest::Confirmed => self.space_confirmed(),
        }
        self.sessions_touched();
        self.request_redraw();
    }

    /// `space-new`: the selected buffer's repository is looked at, then the space is made; with
    /// none, `space-add-repo`'s prompt reads one (P1).
    fn new_space(&mut self, title: String) {
        if self.watch.spaces().full() {
            return self.editor.message("No more spaces can be kept".to_owned());
        }
        match self.selected_repository() {
            Some(repo) => {
                let token = self.hosting.spaces.check();
                self.hosting
                    .spaces
                    .pending
                    .insert(token, Pending::New { title });
                self.check_repository(token, repo);
            }
            None => {
                self.hosting.spaces.title = Some(title);
                let effects = self
                    .dispatcher
                    .run_from_host(&mut self.editor, "space-add-repo");
                if effects.redraw {
                    self.request_redraw();
                }
            }
        }
    }

    /// `space-add-repo`: the path is looked at, then added to the space in front, or makes the
    /// space a `space-new` with no repository waits for.
    fn add_repository(&mut self, text: &str) {
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            return self
                .editor
                .message("A repository's path must be absolute".to_owned());
        }
        let pending = match self.hosting.spaces.title.take() {
            Some(title) => Pending::New { title },
            None => match self.front_space() {
                Some(space) => Pending::Add { space },
                None => return self.editor.message("No space in front".to_owned()),
            },
        };
        let token = self.hosting.spaces.check();
        self.hosting.spaces.pending.insert(token, pending);
        self.check_repository(token, path);
    }

    /// Asks the worker whether `path` is a repository's main checkout, for `token`.
    fn check_repository(&mut self, token: RepoCheck, path: PathBuf) {
        match &self.sessions {
            Some(worker) => {
                worker.send(Job::Space(SpaceJob::CheckRepository { check: token, path }))
            }
            None => {
                self.hosting.spaces.pending.remove(&token);
                self.editor
                    .message("Spaces need the sessions worker, which did not start".to_owned());
            }
        }
    }

    /// The worker looked at `path` for `token`.
    pub(in crate::app) fn repository_checked(
        &mut self,
        token: RepoCheck,
        path: &Path,
        member: Result<Member, String>,
    ) {
        let Some(pending) = self.hosting.spaces.pending.remove(&token) else {
            return;
        };
        let member = match member {
            Ok(member) => member,
            Err(why) => return self.editor.message(printable(&why)),
        };
        let now = now_ms();
        match pending {
            Pending::New { title } => {
                let shown = self.editor.window().buffer();
                let spaces = self.watch.spaces_mut();
                let Some(at) = spaces.new_space(&title, member, now) else {
                    return self
                        .editor
                        .message("No id for a new space yet; try again".to_owned());
                };
                // Entered for the first time, it shows the buffer shown now, alone.
                spaces.showed(at, None, Some(shown));
                self.enter_space(at);
                self.make_space_directory(at, false);
                let name = self.space_name(at);
                self.editor.message(format!("Space {name} made"));
            }
            Pending::Add { space } => {
                let spaces = self.watch.spaces_mut();
                let Some(at) = spaces.find(&space) else {
                    return;
                };
                let shown = printable(&path.display().to_string());
                match spaces.add_member(at, member, now) {
                    // A running orchestrator not given it is offered it once trusted, after
                    // this event (ADR 0136, Q5).
                    Ok(()) => self
                        .editor
                        .message(format!("Added {shown} to space {}", self.space_name(at))),
                    Err(NotAdded::Already) => self
                        .editor
                        .message(format!("The space has {shown} already")),
                    Err(NotAdded::Full) => self
                        .editor
                        .message(format!("A space holds at most {MAX_MEMBERS} repositories")),
                }
            }
        }
        self.sessions_touched();
        self.request_redraw();
    }

    /// `[spaces] root` as it is now, its `~/` expanded; `None` with no home directory for it.
    pub(in crate::app) fn spaces_root_now(&self) -> Option<PathBuf> {
        let home = oso_platform::privacy::home();
        crate::config::expand(self.configuration.spaces_root(), home.as_deref())
    }

    /// Asks the worker to make space `at`'s directory under `[spaces] root`, showing it after
    /// when `show`.
    pub(in crate::app) fn make_space_directory(&mut self, at: usize, show: bool) {
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let home = oso_platform::privacy::home();
        let root = crate::config::expand(self.configuration.spaces_root(), home.as_deref());
        let Some(root) = root else {
            return self.editor.message(
                "[spaces] root starts with ~/, and oso found no home directory".to_owned(),
            );
        };
        let id = stored.id.clone();
        let name = stored.name.clone();
        let members = stored.members.clone();
        let taken: Vec<String> = self
            .watch
            .spaces()
            .iter()
            .filter(|space| space.id() != &id)
            .map(|space| space.name().to_owned())
            .collect();
        if show {
            self.hosting.spaces.show_directory.insert(id.clone());
        }
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(SpaceJob::MakeDirectory {
                id,
                root,
                name,
                members,
                taken,
                exact: false,
            }));
        }
    }

    /// The worker made space `id`'s directory, or said why not, and looked at its
    /// repositories.
    pub(in crate::app) fn space_directory_made(
        &mut self,
        id: &SpaceId,
        made: Result<Made, String>,
        identities: Vec<(PathBuf, Option<(u64, u64)>)>,
    ) {
        let show = self.hosting.spaces.show_directory.remove(id);
        let start = self.hosting.spaces.start_after.remove(id);
        let spaces = self.watch.spaces_mut();
        let Some(at) = spaces.find(id) else {
            return;
        };
        for (root, identity) in identities {
            spaces.set_identity(at, &root, identity);
        }
        match made {
            Ok(Made {
                path,
                identity,
                name,
                asked,
            }) => {
                spaces.set_directory(at, path.clone(), identity, &name, &asked, now_ms());
                if show {
                    self.show_listing(path);
                }
                if start {
                    self.launch_orchestrator(at);
                }
            }
            Err(why) => self.editor.message(printable(&why)),
        }
        self.sessions_touched();
        self.request_redraw();
    }

    /// `space-remove-repo`: refused while a session of the space runs in the repository.
    fn remove_repository(&mut self, name: &str) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        let Some(root) = self.watch.spaces().member_named(name) else {
            return self
                .editor
                .message(format!("The space has no repository {}", printable(name)));
        };
        let busy = self.hosting.table().iter().find(|session| {
            session.runs()
                && self.space_of_session(session) == Some(at)
                && session.directory.starts_with(&root)
        });
        if let Some(session) = busy {
            let who = printable(&session.name);
            return self
                .editor
                .message(format!("Session {who} runs in it; stop it first"));
        }
        if self.watch.spaces_mut().remove_member(at, &root, now_ms()) {
            self.editor
                .message(format!("Took {} out of the space", printable(name)));
        }
    }

    /// `space-close`: refused while a session of the space works or needs input; its idle
    /// ones are stopped, and the space is archived.
    fn close_space(&mut self) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        let sessions: Vec<_> = self
            .hosting
            .table()
            .iter()
            .filter(|session| session.runs() && self.space_of_session(session) == Some(at))
            // The state the list shows: a session whose screen was not judged yet is idle.
            .map(|session| (session.key, session.name.clone(), self.shown_state(session)))
            .collect();
        let idle = match sessions_to_stop(sessions) {
            Ok(idle) => idle,
            Err(who) => {
                return self.editor.message(format!(
                    "Session {} is working or needs input; the space stays open",
                    printable(&who)
                ));
            }
        };
        let name = self.space_name(at);
        if self
            .watch
            .spaces_mut()
            .archive(&mut self.editor, at, now_ms())
            .is_err()
        {
            return self.editor.message(PROMPT_OPEN.to_owned());
        }
        for key in idle {
            self.hang_up_session(key);
        }
        self.editor
            .message(format!("Space {name} closed; space-reopen opens it again"));
    }

    /// `space-reopen`.
    fn reopen_space(&mut self, name: &str) {
        let Some(at) = self.watch.spaces().archived_named(name) else {
            return self
                .editor
                .message(format!("No closed space {}", printable(name)));
        };
        self.watch.spaces_mut().reopen(at, now_ms());
        self.enter_space(at);
        let name = self.space_name(at);
        self.editor.message(format!("Space {name} open again"));
    }

    /// `space-rename`.
    fn rename_space(&mut self, title: &str) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        self.watch.spaces_mut().rename(at, title, now_ms());
        let name = self.space_name(at);
        self.editor.message(format!("The space is now {name}"));
    }

    /// `space-instructions`: the file is found, its directory made, on the worker.
    fn space_instructions(&mut self) {
        let Some(id) = self.front_space() else {
            return self.editor.message("No space in front".to_owned());
        };
        match (&self.sessions, self.hosting.state_directory()) {
            (Some(worker), Some(state)) => {
                worker.send(Job::Space(SpaceJob::Instructions { id, state }));
            }
            _ => self
                .editor
                .message("Instructions need oso's state directory".to_owned()),
        }
    }

    /// The worker found space `id`'s instructions' file: it is visited.
    pub(in crate::app) fn instructions_ready(
        &mut self,
        id: &SpaceId,
        file: Result<PathBuf, String>,
    ) {
        if self.watch.spaces().find(id).is_none() {
            return;
        }
        match file {
            // Through the editor's visits, so that a buffer of it already open is shown.
            Ok(file) => {
                self.editor.request_visit(file, oso_core::load::UNASKED);
                self.serve_files();
            }
            Err(why) => self.editor.message(printable(&why)),
        }
        self.request_redraw();
    }

    /// `space-directory`: listed, made first when it has none.
    fn space_directory(&mut self) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        match self
            .watch
            .spaces()
            .stored(at)
            .and_then(|s| s.directory.clone())
        {
            Some(directory) => self.show_listing(directory),
            None => self.make_space_directory(at, true),
        }
    }

    /// Lists `directory` in the selected window, as `dired` does.
    fn show_listing(&mut self, directory: PathBuf) {
        self.editor.request_listing(ListingRequest::Open {
            directory,
            point: None,
        });
        self.serve_listings();
        self.request_redraw();
    }

    /// The repository of the selected buffer: its project's root, or for a worktree, the
    /// repository git listed it under, else the deepest repository of a space above it.
    fn selected_repository(&self) -> Option<PathBuf> {
        let side = self.editor.side_window();
        let window = self.editor.window();
        let buffer = if Some(window.id()) == side {
            self.editor
                .windows()
                .find(|w| Some(w.id()) != side)
                .map(oso_core::window::Window::buffer)?
        } else {
            window.buffer()
        };
        let project = self.editor.buffer(buffer)?.project()?;
        let root = self.editor.project(project)?.root().to_path_buf();
        if let Some(repo) = self.listed_repository_of(&root) {
            return Some(repo);
        }
        let above = self
            .watch
            .spaces()
            .members()
            .into_iter()
            .filter(|member| root.starts_with(member) && *member != root)
            .max_by_key(|member| member.components().count());
        Some(above.unwrap_or(root))
    }

    /// The id of the space in front.
    fn front_space(&self) -> Option<SpaceId> {
        let spaces = self.watch.spaces();
        spaces
            .current()
            .and_then(|at| spaces.get(at))
            .map(|space| space.id().clone())
    }

    /// Space `at`'s name.
    fn space_name(&self, at: usize) -> String {
        self.watch
            .spaces()
            .get(at)
            .map_or_else(String::new, |space| space.name().to_owned())
    }

    /// After the space requests of an event: a title `space-new` waited with is dropped once
    /// no prompt is open, as `space-add-repo`'s was closed without an answer.
    pub(in crate::app) fn forget_unanswered_space(&mut self) {
        if self.hosting.spaces.title.is_some() && self.editor.prompt().is_none() {
            self.hosting.spaces.title = None;
        }
    }

    /// Gives the space prompts their lists: the repositories oso knows, the space in front's,
    /// and the closed spaces.
    pub(in crate::app) fn space_lists(&mut self) {
        let spaces = self.watch.spaces();
        let members = spaces.member_candidates();
        let archived = spaces.archived_candidates();
        let known = self.known_repositories();
        self.editor.set_listed(REPOSITORIES_LIST, known);
        self.editor.set_listed(MEMBERS_LIST, members);
        self.editor.set_listed(ARCHIVED_LIST, archived);
    }

    /// The repositories oso knows, by path, with their names: the open spaces', the
    /// remembered sessions', and the projects of buffers that are no worktree git listed. A
    /// path with a control or invisible character is left out, as a candidate's text is the
    /// answer and is shown as it is. Each root is looked at once, and the listed worktrees
    /// through one index of them.
    fn known_repositories(&self) -> Arc<[Candidate]> {
        let listed = self.listed_worktrees();
        let worktree = |root: &Path| listed.contains(&spelled(root));
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in self.watch.spaces().members() {
            if seen.insert(spelled(&root)) {
                roots.push(root);
            }
        }
        let projects: HashSet<_> = self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .collect();
        let project_roots = projects
            .into_iter()
            .filter_map(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf());
        let session_roots = self.hosting.table().iter().filter_map(|s| s.root.clone());
        for root in session_roots.chain(project_roots) {
            if !worktree(&root) && seen.insert(spelled(&root)) {
                roots.push(root);
            }
        }
        roots
            .into_iter()
            .filter_map(|root| {
                let text = root.to_str()?.to_owned();
                if !text.chars().all(is_plain) {
                    return None;
                }
                let name = crate::sessions::spaces::stored::last_component(&root);
                Some(Candidate::new(text.as_str(), "", name))
            })
            .collect()
    }
}

/// `path` as its components spell it, for comparing paths through a set.
fn spelled(path: &Path) -> PathBuf {
    path.components().collect()
}

/// `text` as the echo area may show it, whole: no control or invisible character, as such a
/// one in a directory's name could make the line read otherwise than it is.
pub(in crate::app) fn printable(text: &str) -> String {
    text.chars().filter(|&c| is_plain(c)).collect()
}
