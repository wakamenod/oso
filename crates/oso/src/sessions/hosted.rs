//! The sessions oso hosts and remembers (ADR 0121): each one's identity, its terminal buffer,
//! its `claude`'s process while it runs, its baseline (ADR 0122) and the bridge connection that
//! holds it. The app keeps one table; nothing here reads a file or starts a process.
//!
//! Must not depend on: `app`, `dispatch` or threads; the app changes the table as its
//! sessions start, end and are remembered, and the channel's connections are only named.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oso_core::buffer::BufferId;

use super::bridge::{self, Candidate};
use super::identity::{SessionId, unique_name};
use super::record::Remembered;
use super::state::Exit;
use super::worker::SessionKey;
use crate::channel::Connection;

/// Where a hosted session's `claude` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Run {
    /// Being started on its thread.
    Starting,
    /// Running as this process, while it is not reaped.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the session's terminal sets it (ADR 0120 step 4)")
    )]
    Running { child: Option<u32> },
    /// Not running: remembered from an earlier oso, or ended with status 0 or by oso.
    Stopped,
    /// Ended otherwise.
    Failed,
}

/// One hosted session.
#[derive(Debug, Clone)]
pub struct Hosted {
    pub key: SessionKey,
    pub id: SessionId,
    pub name: String,
    pub directory: PathBuf,
    /// The project root it runs in, if any, for its space (ADR 0121 step 3).
    pub root: Option<PathBuf>,
    /// The session that started it, if any.
    pub parent: Option<SessionId>,
    /// Its terminal buffer, once it has one.
    pub buffer: Option<BufferId>,
    pub run: Run,
    /// oso stopped it (`C-x k`, `C-x C-c`, a hang-up), so an end by a signal is no failure.
    pub ended_by_oso: bool,
    /// Its baseline's tree id (ADR 0122), once taken.
    pub baseline: Option<String>,
    pub pinned: bool,
    /// Archived with `C-x k`: not brought back at start, until step 3's list restores it.
    pub archived: bool,
    /// When it last changed, in ms since the epoch.
    pub changed: u64,
    /// The bridge connection that holds it, at most one.
    pub bridge: Option<Connection>,
}

impl Hosted {
    /// What the record keeps of it: no screen and no transcript text.
    pub fn remembered(&self) -> Remembered {
        Remembered {
            id: self.id.clone(),
            name: self.name.clone(),
            directory: self.directory.clone(),
            root: self.root.clone(),
            parent: self.parent.clone(),
            pinned: self.pinned,
            archived: self.archived,
            changed: self.changed,
            baseline: self.baseline.clone(),
        }
    }

    /// Whether its `claude` runs, or is being started.
    pub fn runs(&self) -> bool {
        matches!(self.run, Run::Starting | Run::Running { .. })
    }
}

/// The table.
#[derive(Debug, Default)]
pub struct Table {
    sessions: Vec<Hosted>,
    next_key: SessionKey,
    /// Every id this instance has held, so that writing the record keeps only another
    /// instance's sessions besides these.
    known: HashSet<SessionId>,
    /// Names held for sessions whose start thread runs, with their directory, by key.
    pending: Vec<(SessionKey, PathBuf, String)>,
}

/// What a new session is made of, besides the name and directory it held.
#[derive(Debug, Clone)]
pub struct New {
    pub id: SessionId,
    pub root: Option<PathBuf>,
    pub parent: Option<SessionId>,
    pub changed: u64,
}

impl Table {
    /// Holds a name for a session about to be got ready in `directory`, from `base`, and
    /// gives the key it will have and the name.
    pub fn reserve(&mut self, directory: &Path, base: &str) -> (SessionKey, String) {
        let name = self.name_in(directory, base);
        self.next_key += 1;
        let key = self.next_key;
        self.pending.push((key, directory.to_owned(), name.clone()));
        (key, name)
    }

    /// The name `key` holds, and lets it go: its start was refused.
    pub fn release(&mut self, key: SessionKey) -> Option<(PathBuf, String)> {
        let index = self.pending.iter().position(|(k, ..)| *k == key)?;
        let (_, directory, name) = self.pending.remove(index);
        Some((directory, name))
    }

    /// Adds the session `key` held its name for, with its id, now that it is ready.
    pub fn add(&mut self, key: SessionKey, new: New) -> Option<&mut Hosted> {
        let (directory, name) = self.release(key)?;
        self.known.insert(new.id.clone());
        self.sessions.push(Hosted {
            key,
            id: new.id,
            name,
            directory,
            root: new.root,
            parent: new.parent,
            buffer: None,
            run: Run::Starting,
            ended_by_oso: false,
            baseline: None,
            pinned: false,
            archived: false,
            changed: new.changed,
            bridge: None,
        });
        self.sessions.last_mut()
    }

    /// Takes the sessions an earlier oso remembered, stopped, and gives the keys of those to
    /// bring back: not archived, and not held already. Archived ones are kept, unseen, so that
    /// the record still has them.
    pub fn recall(&mut self, remembered: Vec<Remembered>) -> Vec<SessionKey> {
        let mut back = Vec::new();
        for session in remembered {
            if self.known.contains(&session.id) {
                continue;
            }
            self.next_key += 1;
            let key = self.next_key;
            self.known.insert(session.id.clone());
            if !session.archived {
                back.push(key);
            }
            self.sessions.push(Hosted {
                key,
                id: session.id,
                name: session.name,
                directory: session.directory,
                root: session.root,
                parent: session.parent,
                buffer: None,
                run: Run::Stopped,
                ended_by_oso: false,
                baseline: session.baseline,
                pinned: session.pinned,
                archived: session.archived,
                changed: session.changed,
                bridge: None,
            });
        }
        back
    }

    /// Drops session `key`, which never started: nothing remembers it.
    pub fn forget(&mut self, key: SessionKey) {
        if let Some(index) = self.sessions.iter().position(|session| session.key == key) {
            let session = self.sessions.remove(index);
            self.known.remove(&session.id);
        }
    }

    /// Every session, in the order they came.
    pub fn iter(&self) -> impl Iterator<Item = &Hosted> {
        self.sessions.iter()
    }

    pub fn get(&self, key: SessionKey) -> Option<&Hosted> {
        self.sessions.iter().find(|session| session.key == key)
    }

    pub fn get_mut(&mut self, key: SessionKey) -> Option<&mut Hosted> {
        self.sessions.iter_mut().find(|session| session.key == key)
    }

    /// The session whose terminal is `buffer`.
    pub fn by_buffer(&self, buffer: BufferId) -> Option<&Hosted> {
        self.sessions
            .iter()
            .find(|session| session.buffer == Some(buffer))
    }

    /// The session the bridge connection `connection` holds.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the session's terminal and step 6's tools use it (ADR 0120 step 4)"
        )
    )]
    pub fn by_connection(&self, connection: Connection) -> Option<&Hosted> {
        self.sessions
            .iter()
            .find(|session| session.bridge == Some(connection))
    }

    /// A name for a new session in `directory`, from `base`, unlike every other session's
    /// there, hosted, remembered or held.
    fn name_in(&self, directory: &Path, base: &str) -> String {
        let held = self
            .pending
            .iter()
            .filter(|(_, held_in, _)| held_in == directory)
            .map(|(.., name)| name.as_str());
        let taken = self
            .sessions
            .iter()
            .filter(|session| session.directory == directory)
            .map(|session| session.name.as_str())
            .chain(held);
        unique_name(base, taken)
    }

    /// Whether the connection of `peer`, claiming `claim`, is a hosted session's bridge, as
    /// `bridge::admit` judges it. If so, that session holds it from now on.
    pub fn admit(
        &mut self,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: Option<&SessionId>,
    ) -> Option<&Hosted> {
        let candidates = self.sessions.iter().map(|session| Candidate {
            id: &session.id,
            child: match session.run {
                Run::Running { child } => child,
                _ => None,
            },
            bridged: session.bridge.is_some(),
        });
        let index = bridge::admit(peer, claim, candidates)?;
        let session = &mut self.sessions[index];
        session.bridge = Some(connection);
        Some(session)
    }

    /// The bridge connection `connection` went: its session holds none.
    pub fn bridge_gone(&mut self, connection: Connection) {
        for session in &mut self.sessions {
            if session.bridge == Some(connection) {
                session.bridge = None;
            }
        }
    }

    /// The channel stopped: no session holds a connection.
    pub fn bridges_gone(&mut self) {
        for session in &mut self.sessions {
            session.bridge = None;
        }
    }

    /// The session `key`'s `claude` ended with `status`, or a signal: stopped or failed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the session's terminal and step 6's tools use it (ADR 0120 step 4)"
        )
    )]
    pub fn ended(&mut self, key: SessionKey, status: Option<i32>, changed: u64) -> Option<Exit> {
        let session = self.get_mut(key)?;
        let exit = exit_of(status, session.ended_by_oso);
        session.run = match exit {
            Exit::Stopped => Run::Stopped,
            Exit::Failed => Run::Failed,
        };
        session.changed = changed;
        session.bridge = None;
        Some(exit)
    }

    /// Every session, archived ones too, for the record.
    pub fn remembered(&self) -> Vec<Remembered> {
        self.sessions.iter().map(Hosted::remembered).collect()
    }

    /// Every id this instance has held.
    pub fn known(&self) -> &HashSet<SessionId> {
        &self.known
    }
}

/// How an end reads: status 0, or a signal when oso stopped it, is stopped; else failed.
pub fn exit_of(status: Option<i32>, ended_by_oso: bool) -> Exit {
    match status {
        Some(0) => Exit::Stopped,
        None if ended_by_oso => Exit::Stopped,
        _ => Exit::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use oso_platform::control::{Peer, PeerProcess};

    use super::{New, Run, Table, exit_of};
    use crate::channel::Connection;
    use crate::sessions::identity::SessionId;
    use crate::sessions::record::Remembered;
    use crate::sessions::state::Exit;

    fn new(n: u8) -> New {
        New {
            id: SessionId::from_random([n; 16]),
            root: None,
            parent: None,
            changed: 1,
        }
    }

    fn add(table: &mut Table, n: u8, directory: &str, base: &str) -> u64 {
        let (key, _) = table.reserve(Path::new(directory), base);
        table.add(key, new(n)).unwrap().key
    }

    fn remembered(n: u8, archived: bool) -> Remembered {
        Remembered {
            id: SessionId::from_random([n; 16]),
            name: format!("r{n}"),
            directory: PathBuf::from("/p"),
            root: None,
            parent: None,
            pinned: false,
            archived,
            changed: 5,
            baseline: None,
        }
    }

    #[test]
    fn names_differ_within_a_directory() {
        let mut table = Table::default();
        let (first, name) = table.reserve(Path::new("/p/oso"), "oso");
        assert_eq!(name, "oso");
        let (second, name) = table.reserve(Path::new("/p/oso"), "oso");
        assert_eq!(name, "oso-2", "held while the first gets ready");
        table.add(first, new(1)).unwrap();
        assert_eq!(table.reserve(Path::new("/q/oso"), "oso").1, "oso");
        table.release(second);
        assert_eq!(table.reserve(Path::new("/p/oso"), "oso").1, "oso-2");
        assert!(table.add(second, new(2)).is_none(), "released");
    }

    #[test]
    fn recalled_sessions_come_back_stopped_and_archived_ones_stay_out() {
        let mut table = Table::default();
        let back = table.recall(vec![remembered(1, false), remembered(2, true)]);
        assert_eq!(back.len(), 1);
        let session = table.get(back[0]).unwrap();
        assert_eq!(session.run, Run::Stopped);
        assert_eq!(
            table.remembered().len(),
            2,
            "the archived one is still written"
        );
        // Recalled twice, nothing is added.
        assert!(table.recall(vec![remembered(1, false)]).is_empty());
    }

    #[test]
    fn a_bridge_holds_its_running_session_once() {
        let mut table = Table::default();
        let key = add(&mut table, 1, "/p", "p");
        let id = table.get(key).unwrap().id.clone();
        let peer = Peer {
            process: Some(PeerProcess::new(9, Some("oso"))),
            parent: Some(PeerProcess::new(77, Some("claude"))),
            runs_this_program: true,
        };
        let (first, second) = (
            Connection { run: 1, number: 1 },
            Connection { run: 1, number: 2 },
        );
        assert!(table.admit(first, &peer, Some(&id)).is_none(), "starting");
        table.get_mut(key).unwrap().run = Run::Running { child: Some(77) };
        assert!(table.admit(first, &peer, Some(&id)).is_some());
        assert!(table.admit(second, &peer, Some(&id)).is_none(), "held");
        assert_eq!(table.by_connection(first).unwrap().key, key);
        table.bridge_gone(first);
        assert!(table.admit(second, &peer, Some(&id)).is_some());
        assert_eq!(table.ended(key, Some(0), 2), Some(Exit::Stopped));
        assert!(table.by_connection(second).is_none());
    }

    #[test]
    fn an_end_is_stopped_or_failed() {
        assert_eq!(exit_of(Some(0), false), Exit::Stopped);
        assert_eq!(exit_of(Some(3), false), Exit::Failed);
        assert_eq!(exit_of(Some(3), true), Exit::Failed);
        assert_eq!(exit_of(None, true), Exit::Stopped);
        assert_eq!(exit_of(None, false), Exit::Failed);
    }
}
