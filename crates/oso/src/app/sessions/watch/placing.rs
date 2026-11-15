//! Which issue space each hosted session is in (ADR 0136, step 1a): the one its record names,
//! else the one it joins by `Spaces::join`, kept in its record; and which space's tree the
//! frame shows first.
//!
//! Must not depend on: how a frame is drawn, key presses, or the worker's thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;
use oso_core::editor::Editor;

use super::place;
use crate::app::App;
use crate::sessions::worker::{Job, SessionKey};

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
        let mut joined = false;
        let unplaced: Vec<(SessionKey, PathBuf, Option<PathBuf>)> = self
            .hosting
            .table()
            .iter()
            .filter(|session| {
                (session.space.as_ref()).is_none_or(|id| self.watch.spaces.find(id).is_none())
            })
            .map(|session| {
                let place = place(session);
                let repo = self.listed_repository_of(place).map(Path::to_path_buf);
                (session.key, place.to_path_buf(), repo)
            })
            .collect();
        for (key, place, repo) in unplaced {
            let Some(at) = self.watch.spaces.join(&place, repo.as_deref(), now) else {
                continue;
            };
            let id = self.watch.spaces.get(at).map(|space| space.id().clone());
            if let Some(session) = self.hosting.table.get_mut(key) {
                session.space = id;
                joined = true;
            }
        }
        if joined {
            self.hosting.unsaved = true;
        }
        self.save_sessions();
        if self.watch.spaces.current().is_none() {
            let shown = tree_buffer_of(&self.editor);
            let first = shown
                .and_then(|buffer| self.hosting.table().by_buffer(buffer))
                .or_else(|| self.hosting.table().iter().next())
                .and_then(|session| session.space.as_ref())
                .and_then(|id| self.watch.spaces.find(id));
            if let Some(at) = first {
                self.watch.spaces.adopt(at);
            }
        }
        if self.watch.spaces.wants_ids()
            && let Some(worker) = &self.sessions
        {
            worker.send(Job::SpaceIds(IDS_ASKED));
        }
        // Each session's space's name, worked out once a change of the table.
        let placed: HashMap<SessionKey, String> = self
            .hosting
            .table()
            .iter()
            .map(|session| {
                let name = (session.space.as_ref())
                    .and_then(|id| self.watch.spaces.find(id))
                    .and_then(|at| self.watch.spaces.get(at))
                    .map_or_else(String::new, |space| space.name().to_owned());
                (session.key, name)
            })
            .collect();
        self.watch.placed = placed;
    }
}

/// The buffer of the frame's selected window in the tree, not the side window's.
fn tree_buffer_of(editor: &Editor) -> Option<BufferId> {
    let side = editor.side_window();
    editor
        .windows()
        .find(|window| Some(window.id()) != side)
        .map(oso_core::window::Window::buffer)
}
