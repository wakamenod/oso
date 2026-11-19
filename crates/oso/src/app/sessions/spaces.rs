//! The issue space commands in the app (ADR 0136, step 1b): what `space-new`, `space-add-repo`
//! and the others ask, carried out on the UI thread, with the disk work (a space's directory,
//! whether a path is a repository, the instructions' directory) on the sessions worker, and
//! the lists their prompts complete over.
//!
//! Must not depend on: key presses or how a frame is drawn.

mod handover;
mod lists;
mod members;
mod naming;
mod new_space;
mod orchestrator;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use oso_core::editor::{ListingRequest, SpaceRequest};

use crate::app::App;
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
    /// The first repository of a new space for what this description says.
    New { description: String },
    /// One more repository of this space.
    Add { space: SpaceId },
    /// The repository an agent's `space_add_repo` names (ADR 0161).
    Agent(Box<super::tools::AddLook>),
}

/// What the space commands wait for.
#[derive(Debug, Default)]
pub(in crate::app) struct SpaceWork {
    /// The repositories being looked at.
    pending: HashMap<RepoCheck, Pending>,
    /// The checks asked for so far.
    checks: u64,
    /// A description `space-new` read with no repository to start from: the next
    /// `space-add-repo` answer makes its space.
    description: Option<String>,
    /// The prompt `space-rename` opened, and the space it renames (ADR 0159): dropped once
    /// it closed with no answer.
    pub(in crate::app) renaming: Option<(oso_core::minibuffer::PromptId, SpaceId)>,
    /// The `*scratch*` each new space shows until its orchestrator: shown in its place only if
    /// the selected window still shows it (ADR 0159).
    pub(in crate::app) placeholders: HashMap<SpaceId, oso_core::buffer::BufferId>,
    /// The spaces the person's `claude` is naming, with what each was called when it was asked,
    /// so that a name the person gave meanwhile stays (ADR 0159, step 2).
    pub(in crate::app) asked_names: HashMap<SpaceId, String>,
    /// The run of the person's `claude` that names a space, while one is out: at most one,
    /// which an exit or a restart stops.
    pub(in crate::app) summary_run: Option<crate::sessions::spaces::summary::Running>,
    /// The spaces whose directory is being made: one job a space at a time.
    pub(in crate::app) making: HashSet<SpaceId>,
    /// The spaces whose directory is shown once it is made.
    show_directory: HashSet<SpaceId>,
    /// The spaces whose orchestrator starts once their directory is made (step 3).
    pub(in crate::app) start_after: HashSet<SpaceId>,
    /// The sessions being got ready as an orchestrator, by key: their space.
    pub(in crate::app) starting: HashMap<crate::sessions::worker::SessionKey, SpaceId>,
    /// What the question the host asked last offers, which `y` does (`SpaceRequest::Confirmed`):
    /// dropped when another is asked or it is answered otherwise.
    pub(in crate::app) offer: Option<orchestrator::Offer>,
    /// A replacement of an orchestrator asked about, which `y` does (step 5).
    pub(in crate::app) replace_asked: Option<handover::Asked>,
    /// The orchestrators being stopped for a successor, by key: what starts once each ended.
    pub(in crate::app) replacing: HashMap<crate::sessions::worker::SessionKey, handover::Successor>,
    /// The spaces whose replacement's probe is out: a second asks nothing.
    pub(in crate::app) probing: HashSet<SpaceId>,
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
            SpaceRequest::New(description) => self.new_space(description),
            SpaceRequest::AddRepo(text) => self.add_repository(&text),
            SpaceRequest::RemoveRepo(name) => self.remove_repository(&name),
            SpaceRequest::UseRepo(name) => self.use_repository(&name),
            SpaceRequest::Close => self.close_space(),
            SpaceRequest::Reopen(name) => self.reopen_space(&name),
            SpaceRequest::AskRename => self.ask_space_rename(),
            SpaceRequest::Rename(display) => self.rename_space(&display),
            SpaceRequest::Instructions => self.space_instructions(),
            SpaceRequest::Directory => self.space_directory(),
            SpaceRequest::StartOrchestrator { remake_directory } => {
                self.start_orchestrator(remake_directory);
            }
            SpaceRequest::Confirmed => self.space_confirmed(),
            SpaceRequest::ReplaceOrchestrator => self.replace_orchestrator(),
        }
        self.sessions_touched();
        self.request_redraw();
    }

    /// `space-new`: the selected buffer's repository is looked at, then the space is made and
    /// its orchestrator started; with none, `space-add-repo`'s prompt reads one (P1).
    fn new_space(&mut self, description: String) {
        if self.watch.spaces().full() {
            return self.editor.message("No more spaces can be kept".to_owned());
        }
        match self.selected_repository() {
            Some(repo) => {
                let token = self.hosting.spaces.check();
                self.hosting
                    .spaces
                    .pending
                    .insert(token, Pending::New { description });
                self.check_repository(token, repo);
            }
            None => {
                self.hosting.spaces.description = Some(description);
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
        let pending = match self.hosting.spaces.description.take() {
            Some(description) => Pending::New { description },
            None => match self.front_space() {
                Some(space) => Pending::Add { space },
                None => return self.editor.message("No space in front".to_owned()),
            },
        };
        let token = self.hosting.spaces.check();
        self.hosting.spaces.pending.insert(token, pending);
        self.check_repository(token, path);
    }

    /// Looks at `path` for an agent's `space_add_repo`, as for `space-add-repo`, `path` also
    /// naming it with no link on the way (ADR 0161).
    pub(in crate::app) fn check_repository_for_agent(&mut self, look: super::tools::AddLook) {
        let token = self.hosting.spaces.check();
        let path = look.path().to_owned();
        let pending = Pending::Agent(Box::new(look));
        self.hosting.spaces.pending.insert(token, pending);
        self.check_repository(token, path);
    }

    /// Drops the agents' looks that `gone` says are of a connection that went, before the
    /// person allowed them: nothing they asked for is added.
    pub(in crate::app) fn drop_agent_looks(
        &mut self,
        gone: impl Fn(&super::tools::AddLook) -> bool,
    ) {
        (self.hosting.spaces.pending)
            .retain(|_, pending| !matches!(pending, Pending::Agent(look) if gone(look)));
    }

    /// Asks the worker whether `path` is a repository's main checkout, for `token`.
    fn check_repository(&mut self, token: RepoCheck, path: PathBuf) {
        let as_named = matches!(
            self.hosting.spaces.pending.get(&token),
            Some(Pending::Agent(_))
        );
        match &self.sessions {
            Some(worker) => worker.send(Job::Space(SpaceJob::CheckRepository {
                check: token,
                path,
                as_named,
            })),
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
        let pending = match self.hosting.spaces.pending.remove(&token) {
            Some(Pending::Agent(look)) => return self.agent_repository_checked(*look, member),
            Some(pending) => pending,
            None => return,
        };
        let member = match member {
            Ok(member) => member,
            Err(why) => return self.editor.message(printable(&why)),
        };
        let now = now_ms();
        match pending {
            Pending::New { description } => self.make_new_space(&description, member, now),
            Pending::Agent(_) => {}
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
                        .message(format!("Added {shown} to space {}", self.space_label(at))),
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
    /// when `show`, and starting its orchestrator after when `start`. Its slug is first made
    /// unlike every session's name, as it is fixed once the directory is made (ADR 0159). One
    /// job a space at a time: a second would make another directory and rename the space.
    pub(in crate::app) fn make_space_directory(&mut self, at: usize, show: bool, start: bool) {
        let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) else {
            return;
        };
        if self.hosting.spaces.making.contains(&id) {
            return self
                .editor
                .message("The space's directory is being made".to_owned());
        }
        let home = oso_platform::privacy::home();
        let root = crate::config::expand(self.configuration.spaces_root(), home.as_deref());
        let Some(root) = root else {
            return self.editor.message(
                "[spaces] root starts with ~/, and oso found no home directory".to_owned(),
            );
        };
        if self.sessions.is_none() {
            return;
        }
        let taken = self.session_name_taken();
        (self.watch.spaces_mut()).free_slug(at, &taken, now_ms());
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let (name, members) = (stored.name.clone(), stored.members.clone());
        let taken: Vec<String> = (self.watch.spaces().iter())
            .filter(|space| space.id() != &id)
            .map(|space| space.name().to_owned())
            .collect();
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(SpaceJob::MakeDirectory {
                id: id.clone(),
                root,
                name,
                members,
                taken,
                exact: false,
            }));
        }
        let spaces = &mut self.hosting.spaces;
        if show {
            spaces.show_directory.insert(id.clone());
        }
        if start {
            spaces.start_after.insert(id.clone());
        }
        spaces.making.insert(id);
    }

    /// The worker made space `id`'s directory, or said why not, and looked at its
    /// repositories.
    pub(in crate::app) fn space_directory_made(
        &mut self,
        id: &SpaceId,
        made: Result<Made, String>,
        identities: Vec<(PathBuf, Option<(u64, u64)>)>,
    ) {
        self.hosting.spaces.making.remove(id);
        let show = self.hosting.spaces.show_directory.remove(id);
        let start = self.hosting.spaces.start_after.remove(id);
        let taken = self.session_name_taken();
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
                let names = (name.as_str(), asked.as_str());
                spaces.set_directory(at, path.clone(), identity, names, &taken, now_ms());
                if show {
                    self.show_listing(path);
                }
                if start {
                    self.launch_orchestrator(at, None);
                }
            }
            Err(why) => {
                // `o` later is the person's own key, shown where it is pressed.
                self.hosting.spaces.placeholders.remove(id);
                self.editor.message(printable(&why));
            }
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
        let name = self.space_label(at);
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
        // Nothing starts in it any more: no successor, and no orchestrator after its directory.
        self.forget_space_starts(at);
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
        let name = self.space_label(at);
        self.editor.message(format!("Space {name} open again"));
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
            None => self.make_space_directory(at, true, false),
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

    /// Space `at` as the person sees it called (ADR 0159).
    fn space_label(&self, at: usize) -> String {
        self.watch
            .spaces()
            .get(at)
            .map_or_else(String::new, |space| space.label().to_owned())
    }

    /// After the space requests of an event: a description `space-new` waited with is dropped
    /// once no prompt is open, as `space-add-repo`'s was closed without an answer, and so is a
    /// rename whose prompt closed.
    pub(in crate::app) fn forget_unanswered_space(&mut self) {
        if self.hosting.spaces.description.is_some() && self.editor.prompt().is_none() {
            self.hosting.spaces.description = None;
        }
        // A rename whose prompt closed with no answer: a later answer names no space.
        if let Some((prompt, _)) = self.hosting.spaces.renaming
            && !self.editor.prompts().iter().any(|p| p.id() == prompt)
        {
            self.hosting.spaces.renaming = None;
        }
    }
}

/// `path` as its components spell it, for comparing paths through a set.
pub(in crate::app) fn spelled(path: &Path) -> PathBuf {
    path.components().collect()
}

/// `text` as the echo area may show it, whole: no control or invisible character, as such a
/// one in a directory's name could make the line read otherwise than it is.
pub(in crate::app) fn printable(text: &str) -> String {
    text.chars().filter(|&c| is_plain(c)).collect()
}
