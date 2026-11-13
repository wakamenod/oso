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
    Running { child: Option<u32> },
    /// Not running: remembered from an earlier oso, ended with status 0, or archived.
    Stopped,
    /// Ended otherwise.
    Failed,
}

impl Run {
    /// How lists show it: `*Worktrees*` and the agents' session tools alike.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running { .. } => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
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

    pub fn get(&self, key: SessionKey) -> Option<&Hosted> {
        self.sessions.iter().find(|session| session.key == key)
    }

    pub fn get_mut(&mut self, key: SessionKey) -> Option<&mut Hosted> {
        self.sessions.iter_mut().find(|session| session.key == key)
    }

    /// Whether a session oso hosts runs, or is being started, in `directory`.
    pub fn runs_in(&self, directory: &Path) -> bool {
        self.sessions.iter().any(|session| {
            session.runs() && oso_core::project::same_place(&session.directory, directory)
        }) || self
            .pending
            .iter()
            .any(|(_, pending, _)| oso_core::project::same_place(pending, directory))
    }

    /// The sessions, in the order they came.
    pub fn all(&self) -> &[Hosted] {
        &self.sessions
    }

    /// Keeps at most [`MAX_SESSIONS`](super::record::MAX_SESSIONS) sessions, so that the table
    /// and each record written stay bounded in a long-running oso: past that, the ones that do
    /// not run go, archived ones first, then unpinned ones, each the one that changed longest
    /// ago. Their ids stay known, so the record drops them too. Returns the buffers of those
    /// that had one, which hold no session any more.
    pub fn prune(&mut self) -> Vec<BufferId> {
        let mut buffers = Vec::new();
        while self.sessions.len() > super::record::MAX_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| !session.runs())
                .min_by_key(|(_, session)| (!session.archived, session.pinned, session.changed))
                .map(|(at, _)| at);
            let Some(at) = oldest else { break };
            buffers.extend(self.sessions.remove(at).buffer);
        }
        buffers
    }

    /// Every hosted session.
    pub fn iter(&self) -> impl Iterator<Item = &Hosted> {
        self.sessions.iter()
    }

    /// The session whose terminal is `buffer`.
    pub fn by_buffer(&self, buffer: BufferId) -> Option<&Hosted> {
        self.sessions
            .iter()
            .find(|session| session.buffer == Some(buffer))
    }

    /// The session the bridge connection `connection` holds.
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

    /// Answers the claim of `connection`, from `peer`, to be `claim`'s bridge: [`Table::admit`],
    /// then `answer` with whether it was admitted, which says whether the answer reached the
    /// one who asked. An admission whose answer did not reach is undone: the asker took the
    /// connection as no bridge. The session's name if it holds the connection now.
    pub fn answer_claim(
        &mut self,
        connection: Connection,
        peer: &oso_platform::control::Peer,
        claim: Option<&SessionId>,
        answer: impl FnOnce(bool) -> bool,
    ) -> Option<String> {
        let name = self
            .admit(connection, peer, claim)
            .map(|session| session.name.clone());
        let reached = answer(name.is_some());
        if name.is_some() && !reached {
            self.bridge_gone(connection);
            return None;
        }
        name
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
    pub fn ended(&mut self, key: SessionKey, status: Option<i32>, changed: u64) -> Option<Exit> {
        let session = self.get_mut(key)?;
        let exit = exit_of(status);
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

/// How an end reads: status 0 is stopped; another status or a signal is failed. A session oso
/// ends itself, with `C-x k`, is archived with its buffer and never reported as ended.
pub fn exit_of(status: Option<i32>) -> Exit {
    if status == Some(0) {
        Exit::Stopped
    } else {
        Exit::Failed
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
    fn past_the_cap_archived_then_the_oldest_stopped_sessions_go_and_running_ones_stay() {
        use crate::sessions::record::MAX_SESSIONS;
        let many = |from: usize, count: usize| -> Vec<Remembered> {
            (from..from + count)
                .map(|i| {
                    let mut bytes = [0u8; 16];
                    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
                    Remembered {
                        id: SessionId::from_random(bytes),
                        changed: 100 + i as u64,
                        ..remembered(0, false)
                    }
                })
                .collect()
        };
        let mut table = Table::default();
        let mut first = many(0, MAX_SESSIONS);
        first[5].archived = true;
        first[0].pinned = true;
        let keys = table.recall(first);
        let oldest_unpinned = keys[1];
        let running = table.iter().find(|s| s.changed == 102).unwrap().key;
        table.get_mut(running).unwrap().run = Run::Running { child: None };
        let buffer = oso_core::editor::Editor::default().window().buffer();
        table.get_mut(oldest_unpinned).unwrap().buffer = Some(buffer);
        assert!(table.prune().is_empty(), "at the cap, nothing goes");
        table.recall(many(MAX_SESSIONS, 1));
        assert!(table.prune().is_empty());
        assert!(
            table.iter().all(|s| !s.archived),
            "the archived one went first"
        );
        table.recall(many(MAX_SESSIONS + 1, 2));
        assert_eq!(table.prune(), [buffer]);
        assert_eq!(table.iter().count(), MAX_SESSIONS);
        assert!(
            table.get(oldest_unpinned).is_none(),
            "the oldest unpinned stopped one"
        );
        assert!(table.get(running).is_some(), "a running one stays");
        assert!(
            table.iter().any(|s| s.pinned),
            "a pinned one stays while others can go"
        );
        let newest = table.iter().map(|s| s.changed).max().unwrap();
        assert_eq!(newest, 100 + MAX_SESSIONS as u64 + 2);
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
    fn a_claim_whose_answer_came_too_late_holds_no_session() {
        let mut table = Table::default();
        let key = add(&mut table, 1, "/p", "p");
        let id = table.get(key).unwrap().id.clone();
        table.get_mut(key).unwrap().run = Run::Running { child: Some(77) };
        let peer = Peer {
            process: Some(PeerProcess::new(9, Some("oso"))),
            parent: Some(PeerProcess::new(77, Some("claude"))),
            runs_this_program: true,
        };
        let (late, next) = (
            Connection { run: 1, number: 1 },
            Connection { run: 1, number: 2 },
        );
        // The server stopped waiting: its receiver is gone, and the send fails.
        let (reply, gone) = std::sync::mpsc::channel::<bool>();
        drop(gone);
        let kept = table.answer_claim(late, &peer, Some(&id), |admitted| {
            reply.send(admitted).is_ok()
        });
        assert_eq!(kept, None);
        assert!(table.by_connection(late).is_none());
        assert!(table.get(key).unwrap().bridge.is_none());
        // The session's next bridge is let in.
        let (reply, answered) = std::sync::mpsc::channel::<bool>();
        let kept = table.answer_claim(next, &peer, Some(&id), |admitted| {
            reply.send(admitted).is_ok()
        });
        assert_eq!(kept.as_deref(), Some(table.get(key).unwrap().name.as_str()));
        assert_eq!(answered.try_recv(), Ok(true));
    }

    #[test]
    fn an_end_is_stopped_or_failed() {
        assert_eq!(exit_of(Some(0)), Exit::Stopped);
        assert_eq!(exit_of(Some(3)), Exit::Failed);
        assert_eq!(exit_of(None), Exit::Failed);
    }
}
