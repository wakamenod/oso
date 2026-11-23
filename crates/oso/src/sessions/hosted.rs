//! The sessions oso hosts and remembers (ADR 0121): each one's identity, its terminal buffer,
//! its `claude`'s process while it runs, its baseline (ADR 0122), the bridge connection that
//! holds it, and a native session's bridge's connection to the sessions' socket, which asks the
//! person's permission (ADR 0165, step 2). The app keeps one table; nothing here reads a file or starts a process.
//!
//! Must not depend on: `app`, `dispatch` or threads; the app changes the table as its
//! sessions start, end and are remembered, and the channel's connections are only named.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::BufferId;

mod claims;
use super::identity::Origin;
use super::identity::{SessionId, unique_name};
use super::record::Remembered;
use super::spaces::SpaceId;
use super::state::Exit;
use super::worker::SessionKey;
use crate::channel::Connection;
pub use claims::PermissionClaim;

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
    /// How it started (ADR 0135).
    pub origin: Origin,
    /// The brief of a session an agent started without asking, for `session-explain`; kept
    /// in memory only, never in the record (ADR 0135, as built).
    pub unasked_brief: Option<Arc<str>>,
    /// Its terminal buffer, once it has one.
    pub buffer: Option<BufferId>,
    pub run: Run,
    /// Its baseline's tree id (ADR 0122), once taken.
    pub baseline: Option<String>,
    /// The issue space it belongs to, once it joined one (ADR 0136).
    pub space: Option<SpaceId>,
    /// The directories it also reads, as they were checked (ADR 0136, step 4b).
    pub reads: Vec<crate::sessions::spaces::Member>,
    pub pinned: bool,
    /// Archived with `C-x k`: not brought back at start, until step 3's list restores it.
    pub archived: bool,
    /// A native session (ADR 0165): `claude -p` shown in oso's rows, not a terminal.
    pub native: bool,
    /// When it last changed, in ms since the epoch.
    pub changed: u64,
    /// The bridge connection that holds it, at most one.
    pub bridge: Option<Connection>,
    /// A native session's bridge's connection to the sessions' socket, which asks the person's
    /// permission for its calls (ADR 0165, step 2), at most one. A connection of that socket,
    /// never of the agent channel's, whose numbers it may share.
    pub permission_bridge: Option<Connection>,
    /// Whether its running `claude` came over a restart, `oso dev`'s or `oso-restart`'s: it is
    /// no longer oso's child, so its id could be another's once it ends (ADR 0120, step 11).
    /// Until [`Hosted::started_here`]: a `claude` started in this instance is oso's own.
    pub carried: bool,
    /// The name its running `claude` was given with `--name`, once a rename changed the
    /// record's since it started (ADR 0155): what Claude Code knows it by until its next start
    /// or resume, where the registry does not say. `None`: the record's name.
    pub launched_as: Option<String>,
    /// While `carried`, when that `claude` started, as the instance before read it: a bridge's
    /// parent must have started then too, and with no start known, no bridge is believed
    /// unasked.
    pub started: Option<std::time::SystemTime>,
    /// Its TUI's `claude` (ADR 0165, part 4d-2), from its start until its end is seen reaped,
    /// or a native resume saw it gone: a native resume of the session waits for it first.
    /// Kept in memory only.
    pub tui_process: Option<TuiProcess>,
}

/// A TUI session's `claude` process, as its start thread found it (ADR 0165, part 4d-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiProcess {
    pub pid: u32,
    /// When it started, as the system said just after the spawn; `None` if it did not say,
    /// and then any process with the id counts as it.
    pub started: Option<std::time::SystemTime>,
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
            origin: self.origin,
            pinned: self.pinned,
            archived: self.archived,
            native: self.native,
            changed: self.changed,
            baseline: self.baseline.clone(),
            space: self.space.clone(),
            reads: self.reads.clone(),
        }
    }

    /// Whether its `claude` runs, or is being started.
    pub fn runs(&self) -> bool {
        matches!(self.run, Run::Starting | Run::Running { .. })
    }

    /// Its start thread started its `claude` in this instance, as `child`: oso's own child,
    /// so whatever came over a restart (`carried`, `started`) no longer says anything of it,
    /// and its bridges are judged as any started here are.
    pub fn started_here(&mut self, child: Option<u32>) {
        self.run = Run::Running { child };
        self.carried = false;
        self.started = None;
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
    pub origin: Origin,
    /// The brief, kept only for a session started without asking.
    pub unasked_brief: Option<Arc<str>>,
    pub changed: u64,
    /// A native session (ADR 0165).
    pub native: bool,
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

    /// The directory session `key` runs in, or will once it is ready.
    pub fn directory(&self, key: SessionKey) -> Option<&Path> {
        self.get(key)
            .map(|session| session.directory.as_path())
            .or_else(|| {
                self.pending
                    .iter()
                    .find(|(k, ..)| *k == key)
                    .map(|(_, directory, _)| directory.as_path())
            })
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
            origin: new.origin,
            unasked_brief: new.unasked_brief,
            buffer: None,
            run: Run::Starting,
            baseline: None,
            space: None,
            reads: Vec::new(),
            pinned: false,
            archived: false,
            native: new.native,
            changed: new.changed,
            bridge: None,
            permission_bridge: None,
            carried: false,
            launched_as: None,
            started: None,
            tui_process: None,
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
                // One carried over a restart before the record was read: its space is the
                // record's.
                let held = self.sessions.iter_mut().find(|held| held.id == session.id);
                if let Some(held) = held {
                    if held.space.is_none() {
                        held.space = session.space;
                    }
                    if held.reads.is_empty() {
                        held.reads = session.reads;
                    }
                }
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
                origin: session.origin,
                unasked_brief: None,
                buffer: None,
                run: Run::Stopped,
                baseline: session.baseline,
                space: session.space,
                reads: session.reads,
                pinned: session.pinned,
                native: session.native,
                archived: session.archived,
                changed: session.changed,
                bridge: None,
                permission_bridge: None,
                carried: false,
                launched_as: None,
                started: None,
                tui_process: None,
            });
        }
        back
    }

    /// Drops session `key`, which started and was written in the record, but leaves nothing to
    /// keep: a native session given no message (ADR 0165, step 2). Its id stays known, so the
    /// next record written drops its line, as [`Table::prune`]'s.
    pub fn drop_started(&mut self, key: SessionKey) {
        if let Some(index) = self.sessions.iter().position(|session| session.key == key) {
            self.sessions.remove(index);
        }
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

    /// Archives the sessions not archived yet whose directory is `path` or under it, as a
    /// removal of the worktree at `path` does once git removed it (ADR 0121, "Removal archives
    /// its sessions"): those that do not run, and those that run still only as oso hung them
    /// up, as `hung_up` says of each key. Gives their ids.
    pub fn archive_under(
        &mut self,
        path: &Path,
        hung_up: impl Fn(SessionKey) -> bool,
    ) -> Vec<SessionId> {
        self.sessions
            .iter_mut()
            .filter(|session| !session.archived && session.directory.starts_with(path))
            .filter(|session| !session.runs() || hung_up(session.key))
            .map(|session| {
                session.archived = true;
                session.id.clone()
            })
            .collect()
    }

    /// The sessions, in the order they came.
    pub fn all(&self) -> &[Hosted] {
        &self.sessions
    }

    /// Keeps at most [`MAX_SESSIONS`](super::record::MAX_SESSIONS) sessions, so that the table
    /// and each record written stay bounded in a long-running oso: past that, the ones that do
    /// not run go, archived ones first, then unpinned ones, each the one that changed longest
    /// ago. Their ids stay known, so the record drops them too. Returns the ids of those that
    /// went, each with its buffer if it had one, which holds no session any more.
    pub fn prune(&mut self) -> Vec<(SessionId, Option<BufferId>)> {
        let mut gone = Vec::new();
        while self.sessions.len() > super::record::MAX_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| !session.runs())
                .min_by_key(|(_, session)| (!session.archived, session.pinned, session.changed))
                .map(|(at, _)| at);
            let Some(at) = oldest else { break };
            let session = self.sessions.remove(at);
            gone.push((session.id, session.buffer));
        }
        gone
    }

    /// The names held for sessions whose start thread runs, which a rename may not take
    /// (ADR 0155).
    pub fn held_names(&self) -> impl Iterator<Item = &str> {
        self.pending.iter().map(|(.., name)| name.as_str())
    }

    /// Every hosted session.
    pub fn iter(&self) -> impl Iterator<Item = &Hosted> {
        self.sessions.iter()
    }

    /// Whose words a start by `origin` sends first, as the session's rows say it (ADR 0165, "The
    /// rows' colours"): the session `parent`'s, by its name now; `None` for the person's.
    pub fn brief_from(&self, origin: Origin, parent: Option<&SessionId>) -> Option<String> {
        if origin == Origin::Person {
            return None;
        }
        let parent = parent.and_then(|id| self.iter().find(|session| &session.id == id));
        Some(match parent {
            Some(parent) => format!("session {}", parent.name),
            None => "another session".to_owned(),
        })
    }

    /// The same of the start of session `id`, `None` too if it is not here.
    pub fn started_by(&self, id: &SessionId) -> Option<String> {
        let session = self.iter().find(|session| &session.id == id)?;
        self.brief_from(session.origin, session.parent.as_ref())
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
        session.permission_bridge = None;
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
    use crate::sessions::identity::Origin;
    use crate::sessions::identity::SessionId;
    use crate::sessions::record::Remembered;
    use crate::sessions::state::Exit;

    fn new(n: u8) -> New {
        New {
            native: false,
            id: SessionId::from_random([n; 16]),
            root: None,
            parent: None,
            origin: Origin::Person,
            unasked_brief: None,
            changed: 1,
        }
    }

    fn add(table: &mut Table, n: u8, directory: &str, base: &str) -> u64 {
        let (key, _) = table.reserve(Path::new(directory), base);
        table.add(key, new(n)).unwrap().key
    }

    #[test]
    fn an_agents_start_says_whose_its_first_message_is() {
        let mut table = Table::default();
        let parent = add(&mut table, 1, "/p", "oso-2");
        let parent = table.get(parent).unwrap().id.clone();
        let gone = SessionId::from_random([9; 16]);
        let name = Some("session oso-2".to_owned());
        let other = Some("another session".to_owned());
        for origin in [Origin::Answered, Origin::Unasked, Origin::Unknown] {
            assert_eq!(table.brief_from(origin, Some(&parent)), name);
            assert_eq!(table.brief_from(origin, Some(&gone)), other);
            assert_eq!(table.brief_from(origin, None), other);
            let (key, _) = table.reserve(Path::new("/q"), "child");
            let child = New {
                origin,
                parent: Some(parent.clone()),
                ..new(2)
            };
            let child = table.add(key, child).unwrap().id.clone();
            assert_eq!(table.started_by(&child), name);
            table.forget(key);
        }
        // The person's hand-off, which has a parent too.
        assert_eq!(table.brief_from(Origin::Person, Some(&parent)), None);
        assert_eq!(table.started_by(&parent), None);
        assert_eq!(table.started_by(&gone), None);
    }

    fn remembered(n: u8, archived: bool) -> Remembered {
        Remembered {
            native: false,
            id: SessionId::from_random([n; 16]),
            name: format!("r{n}"),
            directory: PathBuf::from("/p"),
            root: None,
            parent: None,
            origin: Origin::Person,
            pinned: false,
            archived,
            changed: 5,
            baseline: None,
            space: None,
            reads: Vec::new(),
        }
    }

    /// A started session dropped stays known, so that the record drops its line rather than
    /// keep it as another instance's.
    #[test]
    fn a_started_session_dropped_stays_known_and_is_not_remembered() {
        let mut table = Table::default();
        let key = add(&mut table, 1, "/p", "a");
        let id = SessionId::from_random([1; 16]);
        table.drop_started(key);
        assert!(table.get(key).is_none());
        assert!(table.known().contains(&id));
        assert!(table.remembered().iter().all(|kept| kept.id != id));
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
        let gone = table.prune();
        assert_eq!(gone.len(), 1, "one past the cap");
        assert_eq!(gone[0].1, None, "it had no buffer");
        assert!(
            table.iter().all(|s| !s.archived),
            "the archived one went first"
        );
        table.recall(many(MAX_SESSIONS + 1, 2));
        let gone = table.prune();
        assert_eq!(gone.len(), 2);
        assert_eq!(
            gone.iter()
                .filter_map(|(_, buffer)| *buffer)
                .collect::<Vec<_>>(),
            [buffer]
        );
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
        // Its directory is known while it is only reserved, and once it is added.
        assert_eq!(table.directory(second), Some(Path::new("/p/oso")));
        table.add(first, new(1)).unwrap();
        assert_eq!(table.directory(first), Some(Path::new("/p/oso")));
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
        assert!(
            table.admit(first, &peer, Some(&id), None).is_none(),
            "starting"
        );
        table.get_mut(key).unwrap().run = Run::Running { child: Some(77) };
        assert!(table.admit(first, &peer, Some(&id), None).is_some());
        assert!(
            table.admit(second, &peer, Some(&id), None).is_none(),
            "held"
        );
        assert_eq!(table.by_connection(first).unwrap().key, key);
        table.bridge_gone(first);
        assert!(table.admit(second, &peer, Some(&id), None).is_some());
        assert_eq!(table.ended(key, Some(0), 2), Some(Exit::Stopped));
        assert!(table.by_connection(second).is_none());
    }

    /// A session carried over a restart, whose `claude` oso starts again in this instance (a
    /// TUI switched to native, or resumed), takes its new child's bridge unasked: what the
    /// carried `claude` started at says nothing of it (ADR 0165, part 4d-2).
    #[test]
    fn a_carried_sessions_new_claude_is_judged_as_one_started_here() {
        use super::PermissionClaim;
        let mut table = Table::default();
        let key = add(&mut table, 1, "/p", "p");
        let id = table.get(key).unwrap().id.clone();
        let carried_at = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(100);
        let session = table.get_mut(key).unwrap();
        session.run = Run::Running { child: Some(77) };
        session.carried = true;
        session.started = Some(carried_at);
        let peer = |parent| Peer {
            process: Some(PeerProcess::new(9, Some("oso"))),
            parent: Some(PeerProcess::new(parent, Some("claude"))),
            runs_this_program: true,
        };
        let connection = |number| Connection { run: 1, number };
        // The carried `claude`'s bridge: only with its start.
        let later = carried_at + std::time::Duration::from_secs(5);
        assert!(
            (table.admit(connection(1), &peer(77), Some(&id), Some(later))).is_none(),
            "another start"
        );
        assert!((table.admit(connection(1), &peer(77), Some(&id), Some(carried_at))).is_some());
        table.bridge_gone(connection(1));
        // It ended; the switch starts `claude -p` here, which asks on the sessions' socket.
        assert_eq!(table.ended(key, Some(0), 2), Some(Exit::Stopped));
        let session = table.get_mut(key).unwrap();
        session.native = true;
        session.started_here(Some(88));
        let claim =
            table.answer_permission_claim(connection(2), &peer(88), Some(&id), Some(later), |_| {
                true
            });
        assert_eq!(claim, PermissionClaim::Holds(key));
        let session = table.get(key).unwrap();
        assert!(!session.carried && session.started.is_none());
        // The carried `claude`'s id is no longer believed, with its start or with none; the
        // new child's is, with no start known.
        table.permission_bridge_gone(connection(2));
        for started in [Some(carried_at), None] {
            let claim =
                table.answer_permission_claim(connection(3), &peer(77), Some(&id), started, |_| {
                    true
                });
            assert_eq!(claim, PermissionClaim::No, "{started:?}");
            assert!(
                table
                    .admit(connection(3), &peer(77), Some(&id), started)
                    .is_none()
            );
        }
        let claim =
            table.answer_permission_claim(connection(4), &peer(88), Some(&id), None, |_| true);
        assert_eq!(claim, PermissionClaim::Holds(key));
    }

    /// A running native session's bridge asks on the sessions' socket beside its connection to
    /// the channel; a terminal session's never does (ADR 0165, step 2).
    #[test]
    fn only_a_native_sessions_bridge_asks_and_one_at_a_time() {
        use super::PermissionClaim;
        let mut table = Table::default();
        let key = add(&mut table, 1, "/p", "p");
        let id = table.get(key).unwrap().id.clone();
        table.get_mut(key).unwrap().run = Run::Running { child: Some(77) };
        let peer = Peer {
            process: Some(PeerProcess::new(9, Some("oso"))),
            parent: Some(PeerProcess::new(77, Some("claude"))),
            runs_this_program: true,
        };
        let (bridge, first, second) = (
            Connection { run: 1, number: 1 },
            Connection { run: 1, number: 2 },
            Connection { run: 1, number: 3 },
        );
        let asks = |table: &mut Table, connection| {
            table.answer_permission_claim(connection, &peer, Some(&id), None, |_| true)
        };
        assert_eq!(asks(&mut table, first), PermissionClaim::No, "a terminal's");
        table.get_mut(key).unwrap().native = true;
        assert!(table.admit(bridge, &peer, Some(&id), None).is_some());
        assert_eq!(
            asks(&mut table, first),
            PermissionClaim::Holds(key),
            "beside its channel's"
        );
        // A second claimant is told apart: the person hears of it.
        assert_eq!(asks(&mut table, second), PermissionClaim::Held(key));
        let holder = table.by_permission_bridge(first).map(|session| session.key);
        assert_eq!(holder, Some(key));
        table.permission_bridge_gone(first);
        assert!(table.by_permission_bridge(first).is_none());
        // One whose answer did not reach holds nothing.
        let late = table.answer_permission_claim(second, &peer, Some(&id), None, |_| false);
        assert_eq!(late, PermissionClaim::No);
        assert!(table.get(key).unwrap().permission_bridge.is_none());
        assert_eq!(asks(&mut table, second), PermissionClaim::Holds(key));
        assert_eq!(table.ended(key, Some(0), 2), Some(Exit::Stopped));
        assert!(table.by_permission_bridge(second).is_none());
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
        let kept = table.answer_claim(late, &peer, Some(&id), None, |admitted| {
            reply.send(admitted).is_ok()
        });
        assert_eq!(kept, None);
        assert!(table.by_connection(late).is_none());
        assert!(table.get(key).unwrap().bridge.is_none());
        // The session's next bridge is let in.
        let (reply, answered) = std::sync::mpsc::channel::<bool>();
        let kept = table.answer_claim(next, &peer, Some(&id), None, |admitted| {
            reply.send(admitted).is_ok()
        });
        assert_eq!(kept.as_deref(), Some(table.get(key).unwrap().name.as_str()));
        assert_eq!(answered.try_recv(), Ok(true));
    }

    #[test]
    fn a_removal_archives_the_sessions_in_and_under_its_worktree_that_do_not_run() {
        let mut table = Table::default();
        let at = add(&mut table, 1, "/r/wt/a", "at");
        let under = add(&mut table, 2, "/r/wt/a/crates/x", "under");
        let sibling = add(&mut table, 3, "/r/wt/a-2", "sibling");
        let primary = add(&mut table, 4, "/r", "primary");
        let running = add(&mut table, 5, "/r/wt/a", "running");
        let hung_up = add(&mut table, 6, "/r/wt/a", "hung");
        let before = add(&mut table, 7, "/r/wt/a", "before");
        for key in [at, under, sibling, primary, before] {
            table.get_mut(key).unwrap().run = Run::Stopped;
        }
        for key in [running, hung_up] {
            table.get_mut(key).unwrap().run = Run::Running { child: Some(9) };
        }
        table.get_mut(before).unwrap().archived = true;
        let ids = table.archive_under(Path::new("/r/wt/a"), |key| key == hung_up);
        let id = |key| table.get(key).unwrap().id.clone();
        assert_eq!(ids, vec![id(at), id(under), id(hung_up)]);
        let archived = |key| table.get(key).unwrap().archived;
        assert!(archived(at) && archived(under) && archived(hung_up));
        assert!(!archived(sibling), "a sibling with the same prefix stays");
        assert!(!archived(primary), "the main checkout above stays");
        assert!(!archived(running), "one that runs, not hung up, stays");
        assert!(
            archived(before),
            "one archived before stays archived, not counted"
        );
    }

    #[test]
    fn an_end_is_stopped_or_failed() {
        assert_eq!(exit_of(Some(0)), Exit::Stopped);
        assert_eq!(exit_of(Some(3)), Exit::Failed);
        assert_eq!(exit_of(None), Exit::Failed);
    }
}
