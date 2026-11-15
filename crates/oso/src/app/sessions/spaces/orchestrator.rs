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
        let Some(stored) = self.watch.spaces().stored(at) else {
            return;
        };
        let id = stored.id.clone();
        match stored.directory.clone() {
            None => {
                self.hosting.spaces.start_after.insert(id);
                self.make_space_directory(at, false);
            }
            Some(directory) if remake => {
                self.hosting.spaces.start_after.insert(id.clone());
                self.remake_space_directory(at, &id, &directory);
            }
            Some(_) => self.launch_orchestrator(at),
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
        if let Some(worker) = &self.sessions {
            worker.send(Job::Space(job));
        }
    }

    /// Starts space `at`'s orchestrator in its directory, named after the space.
    pub(in crate::app) fn launch_orchestrator(&mut self, at: usize) {
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
        let begin = Begin {
            name: Some(name),
            directory,
            space: Some(space),
            ..Begin::default()
        };
        if let Some(key) = self.begin_session(begin) {
            self.hosting.spaces.starting.insert(key, id);
        }
    }

    /// Hosted session `key` started as an orchestrator: it belongs to its space, which
    /// records it, the one before kept as an earlier one.
    pub(in crate::app) fn orchestrator_started(&mut self, key: SessionKey) {
        let Some(space) = self.hosting.spaces.starting.remove(&key) else {
            return;
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
    /// space as it is now, as Claude Code keeps neither its `--add-dir`s nor its prompt.
    ///
    /// # Errors
    ///
    /// Why the orchestrator cannot start, for the echo area.
    pub(in crate::app) fn orchestrator_start_for(
        &self,
        key: SessionKey,
    ) -> Result<Option<SpaceStart>, String> {
        let Some(session) = self.hosting.table().get(key) else {
            return Ok(None);
        };
        let spaces = self.watch.spaces();
        let at = spaces
            .iter()
            .position(|space| space.stored.orchestrator.as_ref() == Some(&session.id));
        at.map(|at| self.orchestrator_start(at)).transpose()
    }

    /// What space `at`'s orchestrator starts with: the fixed text, the space's header, its
    /// trusted repositories and its instructions' file.
    fn orchestrator_start(&self, at: usize) -> Result<SpaceStart, String> {
        let stored = self.watch.spaces().stored(at).ok_or("No such space")?;
        if stored.directory.is_none() {
            return Err("The space has no directory yet: space-directory makes it".to_owned());
        }
        let named = self.watch.spaces().member_names(at);
        let trusted_paths: Vec<PathBuf> = self
            .editor
            .trust_list()
            .map(|list| list.paths().map(Path::to_path_buf).collect())
            .unwrap_or_default();
        let trusted = |root: &Path| trusted_paths.iter().any(|path| same(path, root));
        let repositories: Vec<Repository<'_>> = named
            .iter()
            .map(|(name, root)| Repository {
                name,
                root,
                trusted: trusted(root),
            })
            .collect();
        let trusted_members: Vec<Member> = (stored.members.iter())
            .filter(|member| trusted(&member.root))
            .cloned()
            .collect();
        let prompt = format!("{TEXT}{}", header(stored, &repositories));
        let instructions = self.hosting.state_directory().map(|state| {
            crate::sessions::spaces::probe::instructions_path(&state, stored.id.as_str())
        });
        Ok(SpaceStart {
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
    /// given `root`: a repository added, or trusted, since it started (Q5).
    pub(in crate::app) fn offer_add_dir(&mut self, at: usize, root: &Path) {
        let Some(key) = self.live_orchestrator(at) else {
            return;
        };
        // A repository oso does not trust is given no `--add-dir`, so it is offered none.
        let trusted = (self.editor.trust_list())
            .is_some_and(|list| list.paths().any(|path| same(path, root)));
        if !trusted {
            return;
        }
        let Some(text) = root.to_str().map(|root| format!("/add-dir {root}")) else {
            return;
        };
        let name = self
            .watch
            .spaces()
            .get(at)
            .map_or_else(String::new, |space| space.name().to_owned());
        let question = format!(
            "Paste {} into orchestrator {name}'s prompt, not sent? (y or n)",
            super::printable(&text)
        );
        // A question asked anew drops the one before, and what it offered.
        self.hosting.spaces.offer = None;
        match self.editor.ask(question, "space-confirm") {
            Ok(prompt) => {
                self.hosting.spaces.offer = Some(Offer {
                    prompt,
                    session: key,
                    text,
                });
            }
            Err(e) => self.editor.message(e.to_string()),
        }
    }

    /// `y` to the space question asked last: its offer. One whose question closed otherwise
    /// was dropped after that event, and one asked anew replaced it, so a `y` does only the
    /// question it answered; with nothing kept, it does nothing.
    pub(in crate::app) fn space_confirmed(&mut self) {
        let Some(offer) = self.hosting.spaces.offer.take() else {
            return;
        };
        if let Err(why) = self.send_to_session(offer.session, offer.text) {
            self.editor.message(why);
        }
    }

    /// After an event: an offer whose question is no longer open, and was not answered `y`
    /// in it, is dropped, so that no later `y` does it.
    pub(in crate::app) fn forget_unanswered_offer(&mut self) {
        let open = self.editor.prompt().map(|prompt| prompt.id());
        let asked = self.hosting.spaces.offer.as_ref().map(|offer| offer.prompt);
        if asked.is_some() && open != asked {
            self.hosting.spaces.offer = None;
        }
    }

    /// The trusted roots changed from `before`: each repository of a space that became
    /// trusted is offered to the space's running orchestrator (Q5).
    pub(in crate::app) fn offer_newly_trusted(&mut self, before: &[PathBuf]) {
        let now: Vec<PathBuf> = self
            .editor
            .trust_list()
            .map(|list| list.paths().map(Path::to_path_buf).collect())
            .unwrap_or_default();
        let new: Vec<PathBuf> = now
            .into_iter()
            .filter(|path| !before.iter().any(|was| same(was, path)))
            .collect();
        for root in new {
            let at = (0..self.watch.spaces().iter().count()).find(|&at| {
                self.watch
                    .spaces()
                    .stored(at)
                    .is_some_and(|s| !s.archived && s.has_member(&root))
            });
            if let Some(at) = at {
                self.offer_add_dir(at, &root);
                return;
            }
        }
    }
}
