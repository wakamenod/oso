//! Which issue space each hosted session is in (ADR 0136, step 1a): the one its record names,
//! else the one it joins by `Spaces::join`, kept in its record; and which space's tree the
//! frame shows first.
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread.

use std::collections::HashMap;
use std::path::PathBuf;

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

use super::place;
use crate::app::App;
use crate::sessions::hosted::Hosted;
use crate::sessions::worker::{Job, SessionKey, SpaceJob};

/// How many ids for new spaces one ask brings.
const IDS_ASKED: usize = 8;

impl App {
    /// Each hosted session's space: the one its record names, else the one it joins (ADR
    /// 0136), kept in its record. The frame's tree is the first space's, or after a restart
    /// that kept it, the space of the session it shows.
    pub(super) fn place_sessions(&mut self) {
        if !self.watch.spaces.recalled() {
            return;
        }
        let now = crate::sessions::spaces::stored::now_ms();
        let keeps = self.watch.spaces.keeps_joins();
        let mut joined = false;
        // The repository git listed each one's place under is looked up only when no space
        // holds the place already.
        let unplaced: Vec<(SessionKey, PathBuf, Option<PathBuf>)> = self
            .hosting
            .table()
            .iter()
            .filter(|session| self.space_of_session(session).is_none())
            .map(|session| {
                let place = place(session);
                let repo = match self.watch.spaces.holding(place) {
                    Some(_) => None,
                    None => self.listed_repository_of(place),
                };
                (session.key, place.to_path_buf(), repo)
            })
            .collect();
        for (key, place, repo) in unplaced {
            let Some(at) = self.watch.spaces.join(&place, repo.as_deref(), now) else {
                continue;
            };
            let Some(id) = self.watch.spaces.get(at).map(|space| space.id().clone()) else {
                continue;
            };
            if !keeps {
                self.watch.joined.insert(key, id);
            } else if let Some(session) = self.hosting.table.get_mut(key) {
                session.space = Some(id);
                joined = true;
            }
        }
        if joined {
            self.hosting.unsaved = true;
        }
        self.save_sessions();
        if self.watch.spaces.current().is_none() {
            let first = tree_buffer_of(&self.editor)
                .and_then(|buffer| self.hosting.table().by_buffer(buffer))
                .or_else(|| self.hosting.table().iter().next())
                .and_then(|session| self.space_of_session(session));
            if let Some(at) = first {
                self.watch.spaces.adopt(at);
            }
        }
        if self.watch.spaces.wants_ids()
            && let Some(worker) = &self.sessions
        {
            worker.send(Job::Space(SpaceJob::Ids(IDS_ASKED)));
        }
        // Each session's space's name, worked out once a change of the table.
        let placed: HashMap<SessionKey, String> = self
            .hosting
            .table()
            .iter()
            .map(|session| {
                let name = (self.space_of_session(session))
                    .and_then(|at| self.watch.spaces.get(at))
                    .map_or_else(String::new, |space| space.name().to_owned());
                (session.key, name)
            })
            .collect();
        self.watch.placed = placed;
    }

    /// After a restart's windows are in: the space of the session the frame's tree shows is in
    /// front, in place of the one the tree was given as the record was read, before them. Not once
    /// a space was entered, nor where the tree shows no session (ADR 0136).
    pub(in crate::app) fn front_space_is_shown(&mut self) {
        let shown = tree_buffer_of(&self.editor)
            .and_then(|buffer| self.hosting.table().by_buffer(buffer))
            .and_then(|session| self.space_of_session(session));
        if let Some(at) = shown
            && self.watch.spaces.adopt_instead(at)
        {
            self.sessions_touched();
        }
    }

    /// The space hosted session `session` is in: the one its record names, else the one it
    /// joined in this instance only.
    pub(in crate::app) fn space_of_session(&self, session: &Hosted) -> Option<usize> {
        let spaces = &self.watch.spaces;
        (session.space.as_ref())
            .and_then(|id| spaces.find(id))
            .or_else(|| {
                self.watch
                    .joined
                    .get(&session.key)
                    .and_then(|id| spaces.find(id))
            })
    }
}

/// The buffer of the frame's tree's selected window, else of the one selected last, as the side
/// window is; of a selected docked window's host.
fn tree_buffer_of(editor: &Editor) -> Option<BufferId> {
    (editor.window_by_id(editor.tree_window())).map(oso_core::window::Window::buffer)
}
