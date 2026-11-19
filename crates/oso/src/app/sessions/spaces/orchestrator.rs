//! A space's orchestrator in the app (ADR 0136, step 3): starting it in the space's own
//! directory, its directory made first, or made again where it is gone; recording it; making
//! its command line again for a resume; and offering to paste `/add-dir` into it when a
//! repository it was not given becomes one it may have.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! An offer is a question the host asks with `space-confirm` as its `y`: what it would paste is
//! kept beside the question's id, dropped when another question is asked, and done only when
//! `y` answers that same question, as the person's paste, never submitted.

use std::path::{Path, PathBuf};

use oso_core::minibuffer::PromptId;

use crate::app::App;
use crate::app::sessions::hosting::Begin;
use crate::sessions::spaces::orchestrator::{Repository, TEXT, header};
use crate::sessions::spaces::stored::{now_ms, same};
use crate::sessions::spaces::{Member, SpaceId};
use crate::sessions::start::SpaceStart;
use crate::sessions::worker::{Job, SessionKey, SpaceJob};

/// What a question the host asked would do on `y`: paste `/add-dir PATH` into an orchestrator.
#[derive(Debug)]
pub(in crate::app) struct Offer {
    /// The question it was asked with.
    prompt: PromptId,
    session: SessionKey,
    /// The space and the repository it offers.
    space: SpaceId,
    root: PathBuf,
    text: String,
}

impl App {
    /// `space-orchestrator-start`: the space in front's orchestrator, shown if it runs; else
    /// started in the space's directory, which is made first if it has none, and made again
    /// where it is gone with `remake`.
    pub(in crate::app) fn start_orchestrator(&mut self, remake: bool) {
        let Some(at) = self.watch.spaces().current() else {
            return self.editor.message("No space in front".to_owned());
        };
        if let Some(key) = self.live_orchestrator(at) {
            self.editor
                .message("The space's orchestrator runs: here it is".to_owned());
            return self.show_session(key);
        }
        if let Some(why) = self.space_starts_busy(at) {
            return self.editor.message(why);
        }
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let id = stored.id.clone();
        match stored.directory.clone() {
            None => self.make_space_directory(at, false, true),
            Some(directory) if remake => self.remake_space_directory(at, &id, &directory),
            Some(_) => self.launch_orchestrator(at, None),
        }
    }

    /// Makes space `at`'s directory again at its recorded place, `directory`, which must be
    /// gone: Claude Code keys the orchestrator's memory by that path.
    fn remake_space_directory(&mut self, at: usize, id: &SpaceId, directory: &Path) {
        let (Some(root), Some(name)) = (directory.parent(), directory.file_name()) else {
            return;
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let job = SpaceJob::MakeDirectory {
            id: id.clone(),
            root: root.to_path_buf(),
            name: name.to_string_lossy().into_owned(),
            members: stored.members.clone(),
            taken: Vec::new(),
            exact: true,
        };
        if self.hosting.spaces.making.contains(id) {
            return self
                .editor
                .message("The space's directory is being made".to_owned());
        }
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(job));
            self.hosting.spaces.start_after.insert(id.clone());
            self.hosting.spaces.making.insert(id.clone());
        }
    }

    /// Starts space `at`'s orchestrator in its directory, named after the space.
    pub(in crate::app) fn launch_orchestrator(&mut self, at: usize, brief: Option<String>) {
        let space = match self.orchestrator_start(at) {
            Ok(space) => space,
            Err(why) => return self.editor.message(why),
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let (id, name, directory) = (
            stored.id.clone(),
            stored.name.clone(),
            stored.directory.clone(),
        );
        // Shown in its space: one whose start comes after the person left it starts behind, as
        // a successor's may (ADR 0159), and so does `space-new`'s once the person put something
        // else where its `*scratch*` was, as it would take the keys they type there.
        let placeholder = self.hosting.spaces.placeholders.remove(&id);
        let behind = self.watch.spaces().current() != Some(at)
            || placeholder.is_some_and(|scratch| self.editor.window().buffer() != scratch);
        let begin = Begin {
            name: Some(name),
            directory,
            brief,
            space: Some(space),
            behind,
            ..Begin::default()
        };
        if let Some(key) = self.begin_session(begin) {
            self.hosting.spaces.starting.insert(key, id);
        }
    }

    /// Hosted session `key` just started: as an orchestrator, it belongs to its space, which
    /// records it, the one before kept as an earlier one; started by one, it is a worker of
    /// that space (ADR 0136, step 4).
    pub(in crate::app) fn space_session_started(&mut self, key: SessionKey) {
        // A session that moved into a space at its request is in that space, if still open
        // (ADR 0151).
        if let Some(space) = self.moving_into(key).cloned() {
            let spaces = self.watch.spaces();
            let open =
                (spaces.find(&space).and_then(|at| spaces.get(at))).is_some_and(|s| s.is_open());
            if let Some(session) = self.hosting.table.get_mut(key)
                && open
            {
                session.space = Some(space);
                self.hosting.unsaved = true;
                self.sessions_touched();
            }
            return;
        }
        let Some(space) = self.hosting.spaces.starting.remove(&key) else {
            return self.worker_joined(key);
        };
        let Some(session) = self.hosting.table.get_mut(key) else {
            return;
        };
        session.space = Some(space.clone());
        let session_id = session.id.clone();
        self.hosting.unsaved = true;
        let spaces = self.watch.spaces_mut();
        if let Some(at) = spaces.find(&space) {
            spaces.set_orchestrator(at, session_id, now_ms());
        }
        self.sessions_touched();
    }

    /// What hosted session `key` starts with again, when it is a space's orchestrator: its
    /// space as it is now, as Claude Code keeps neither its `--add-dir`s nor its prompt. Another
    /// session in a space's directory gets the directory's checks alone.
    ///
    /// # Errors
    ///
    /// Why the orchestrator cannot start, for the echo area.
    pub(in crate::app) fn orchestrator_start_for(
        &mut self,
        key: SessionKey,
    ) -> Result<Option<SpaceStart>, String> {
        let Some(session) = self.hosting.table().get(key) else {
            return Ok(None);
        };
        let (id, directory) = (session.id.clone(), session.directory.clone());
        let spaces = self.watch.spaces();
        let at = spaces
            .iter()
            .position(|space| space.stored.orchestrator.as_ref() == Some(&id));
        match at {
            Some(at) => self.orchestrator_start(at).map(Some),
            None => Ok(self
                .space_checks(&directory)
                .or_else(|| self.reads_again(key))),
        }
    }

    /// The checks a session starting in `directory` passes when it is a space's directory,
    /// with no `--add-dir` and no prompt (ADR 0136: before every start there).
    pub(in crate::app) fn space_checks(&self, directory: &Path) -> Option<SpaceStart> {
        let spaces = self.watch.spaces();
        let space = spaces.iter().find(|space| {
            (space.stored.directory.as_deref()).is_some_and(|its| same(its, directory))
        })?;
        Some(SpaceStart {
            in_space_directory: true,
            reads: Vec::new(),
            made: space.stored.directory_identity,
            members: space.stored.members.clone(),
            trusted: Vec::new(),
            prompt: String::new(),
            instructions: None,
        })
    }

    /// What space `at`'s orchestrator starts with, the repositories it is given kept so that
    /// none is offered to it again.
    fn orchestrator_start(&mut self, at: usize) -> Result<SpaceStart, String> {
        let start = self.orchestrator_start_of(at)?;
        if let Some(stored) = self.watch.spaces().stored(at) {
            let given = start.trusted.iter().map(|member| member.root.clone());
            (self.hosting.spaces.given).insert(stored.id.clone(), given.collect());
        }
        Ok(start)
    }

    /// What space `at`'s orchestrator starts with: the fixed text, the space's header, its
    /// repositories trusted by path and identity, as oso trusts a project, and its
    /// instructions' file.
    fn orchestrator_start_of(&self, at: usize) -> Result<SpaceStart, String> {
        let stored = self.watch.spaces().stored(at).ok_or("No such space")?;
        if stored.archived {
            return Err("The space is closed: space-reopen opens it again".to_owned());
        }
        if stored.directory.is_none() {
            return Err("The space has no directory yet: space-directory makes it".to_owned());
        }
        let named = self.watch.spaces().member_names(at);
        let trusted = |root: &Path| {
            (stored.members.iter()).any(|member| same(&member.root, root) && self.trusts(member))
        };
        let repositories: Vec<Repository<'_>> = named
            .iter()
            .map(|(name, root)| Repository {
                name,
                root,
                trusted: trusted(root),
            })
            .collect();
        let trusted_members: Vec<Member> = (stored.members.iter())
            .filter(|member| self.trusts(member))
            .cloned()
            .collect();
        let prompt = format!("{TEXT}{}", header(stored, &repositories));
        let instructions = self.hosting.state_directory().map(|state| {
            crate::sessions::spaces::probe::instructions_path(&state, stored.id.as_str())
        });
        Ok(SpaceStart {
            in_space_directory: true,
            reads: Vec::new(),
            made: stored.directory_identity,
            members: stored.members.clone(),
            trusted: trusted_members,
            prompt,
            instructions,
        })
    }

    /// The key of space `at`'s orchestrator while it runs.
    fn live_orchestrator(&self, at: usize) -> Option<SessionKey> {
        let id = self.watch.spaces().stored(at)?.orchestrator.clone()?;
        let table = self.hosting.table();
        table
            .iter()
            .find(|session| session.id == id && session.runs())
            .map(|session| session.key)
    }

    /// The key of space `at`'s orchestrator, running or stopped, while it has a terminal.
    pub(in crate::app) fn orchestrator_of(&self, at: usize) -> Option<SessionKey> {
        let id = self.watch.spaces().stored(at)?.orchestrator.clone()?;
        let table = self.hosting.table();
        table
            .iter()
            .find(|session| session.id == id && !session.archived && session.buffer.is_some())
            .map(|session| session.key)
    }

    /// Offers to paste `/add-dir root` into space `at`'s running orchestrator, which was not
    /// given `root`: a trusted repository added, or trusted, since it started (Q5). Whether
    /// it asked.
    fn offer_add_dir(&mut self, at: usize, root: &Path) -> bool {
        let Some(key) = self.live_orchestrator(at) else {
            return false;
        };
        let Some(stored) = self.watch.spaces().stored(at) else {
            return false;
        };
        let space = stored.id.clone();
        let given = self.hosting.spaces.given.get(&space);
        if given.is_some_and(|given| given.iter().any(|was| same(was, root))) {
            return false;
        }
        let Some(text) = root.to_str().map(|root| format!("/add-dir {root}")) else {
            return false;
        };
        // The orchestrator answers to the space's slug.
        let name = stored.name.clone();
        let question = format!(
            "Paste {} into orchestrator {name}'s prompt, not sent? (y or n)",
            super::printable(&text)
        );
        // A question asked anew drops the one before, and what it offered.
        self.hosting.spaces.offer = None;
        self.hosting.spaces.replace_asked = None;
        match self.editor.ask(question, "space-confirm") {
            Ok(prompt) => {
                self.hosting.spaces.offer = Some(Offer {
                    prompt,
                    session: key,
                    space,
                    root: root.to_owned(),
                    text,
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
        true
    }

    /// `y` to the space question asked last: its offer, if it still holds: the session is
    /// still the space's running orchestrator, and the repository still one of the space's
    /// that oso trusts. One whose question closed otherwise was dropped after that event, and
    /// one asked anew replaced it, so a `y` does only the question it answered; with nothing
    /// kept, it does nothing.
    pub(in crate::app) fn space_confirmed(&mut self) {
        if self.hosting.spaces.replace_asked.is_some() {
            return self.replace_confirmed();
        }
        let Some(offer) = self.hosting.spaces.offer.take() else {
            return;
        };
        let at = self.watch.spaces().find(&offer.space);
        let holds = at.is_some_and(|at| {
            self.live_orchestrator(at) == Some(offer.session)
                && self.trusted_members().is_some_and(|members| {
                    (members.iter()).any(|(id, root)| *id == offer.space && same(root, &offer.root))
                })
        });
        if !holds {
            return self
                .editor
                .message("Not pasted: the space or its orchestrator changed meanwhile".to_owned());
        }
        match self.send_to_session(offer.session, offer.text) {
            Ok(()) => (self.hosting.spaces.given.entry(offer.space))
                .or_default()
                .push(offer.root),
            Err(why) => self.editor.message(why),
        }
    }

    /// After an event: an offer whose question is no longer open, and was not answered `y`
    /// in it, is dropped, so that no later `y` does it.
    pub(in crate::app) fn forget_unanswered_offer(&mut self) {
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let replace = self
            .hosting
            .spaces
            .replace_asked
            .as_ref()
            .map(|asked| asked.prompt);
        if replace.is_some() && open != replace {
            self.hosting.spaces.replace_asked = None;
        }
        let asked = self.hosting.spaces.offer.as_ref().map(|offer| offer.prompt);
        if asked.is_some() && open != asked {
            self.hosting.spaces.offer = None;
        }
    }

    /// After an event: the repositories of open spaces that oso trusts, compared with those
    /// seen last; one trusted or added since is offered to its space's running orchestrator
    /// (Q5). Nothing is offered when the trust list was not read before, as when oso starts:
    /// every root would look new.
    pub(in crate::app) fn notice_trust(&mut self) {
        let now = self.trusted_members();
        let before = std::mem::replace(&mut self.hosting.spaces.trusted_seen, now.clone());
        let (Some(before), Some(now)) = (before, now) else {
            return;
        };
        for (space, root) in now {
            let seen = (before.iter()).any(|(id, was)| *id == space && same(was, &root));
            if seen {
                continue;
            }
            let at = self.watch.spaces().find(&space);
            if at.is_some_and(|at| self.offer_add_dir(at, &root)) {
                return;
            }
        }
    }

    /// Each open space's repositories that oso trusts; none while the trust list is unread.
    fn trusted_members(&self) -> Option<Vec<(SpaceId, PathBuf)>> {
        self.editor.trust_list()?;
        let spaces = self.watch.spaces();
        let open = spaces.iter().filter(|space| !space.stored.archived);
        let members = open.flat_map(|space| {
            (space.stored.members.iter())
                .filter(|member| self.trusts(member))
                .map(|member| (space.stored.id.clone(), member.root.clone()))
        });
        Some(members.collect())
    }
}
