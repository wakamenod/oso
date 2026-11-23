//! The sessions oso remembers (ADR 0121, "Restore and the fork hazard"): `oso/sessions` in the
//! per-user state directory, beside `theme`, one JSON object a line. A start of oso brings each
//! back stopped; nothing in the file starts a process.
//!
//! Must not depend on: threads, the UI or `app`; the sessions worker reads and writes it, never
//! the UI thread.
//!
//! - It holds each session's id, name, directory, project root, parent, how it started (ADR
//!   0135), pin and archive marks, when it last changed and its baseline. No screen text, no
//!   transcript text and no brief.
//! - It is read only if the directory is private and the file a regular one of this user's
//!   with one name, at most [`MAX_BYTES`] and [`MAX_SESSIONS`] sessions. A line that does not
//!   hold a whole, valid session is left out, and only how many were is logged.
//! - A write replaces the file whole, through a `.part`, or leaves it as it was. It reads the
//!   file first and keeps the sessions this instance never knew, so that two instances do not
//!   drop each other's.
//! - As oso starts, [`stranded`] finds the sessions a worktree removed left behind, in a
//!   directory gone under a repository's main checkout, for the app to archive (ADR 0121,
//!   "Removal archives its sessions"); the worker runs it on a thread of its own.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use oso_platform::file_meta;
use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};
use serde_json::{Map, Value, json};

use super::identity::{Origin, SessionId, clean_name};
use super::spaces::SpaceId;

/// The file's name in its directory.
const FILE: &str = "sessions";
/// Where a write goes first, so that the file is whole or not there.
const PART: &str = "sessions.part";
/// The most bytes read of the file; a larger one is not read.
pub const MAX_BYTES: u64 = 256 * 1024;
/// The most sessions read or written.
pub const MAX_SESSIONS: usize = 256;
/// The longest directory or root kept, in bytes.
const MAX_PATH: usize = 4096;

/// The most directories a session's record keeps that it also reads.
const MAX_READS: usize = 8;

/// One remembered session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    pub id: SessionId,
    /// Its name, as [`clean_name`] leaves it.
    pub name: String,
    /// The directory it runs in, absolute.
    pub directory: PathBuf,
    /// The root of its project, for its space.
    pub root: Option<PathBuf>,
    /// The session that started it.
    pub parent: Option<SessionId>,
    /// How it started.
    pub origin: Origin,
    pub pinned: bool,
    /// Archived: not brought back at start.
    pub archived: bool,
    /// A native session (ADR 0165): it comes back, and resumes, as one. Absent: a TUI's.
    pub native: bool,
    /// When it last changed state, in milliseconds since the epoch.
    pub changed: u64,
    /// Its baseline for a review (ADR 0122): a git tree's id, 40 or 64 lower-case hex digits.
    pub baseline: Option<String>,
    /// The issue space it belongs to (ADR 0136).
    pub space: Option<SpaceId>,
    /// The directories it also reads, with the identities they were checked with, each
    /// checked again before a resume passes it (ADR 0136, step 4b).
    pub reads: Vec<super::spaces::Member>,
}

/// What is remembered in `state_dir`. Anything the rules above refuse gives nothing.
pub fn read(state_dir: &Path) -> Vec<Remembered> {
    match read_checked(state_dir) {
        Ok(sessions) => sessions,
        Err(e) => {
            log::warn!("not reading the remembered sessions: {e}");
            Vec::new()
        }
    }
}

/// What is remembered in `state_dir`: nothing if there is no file, and an error if there is
/// one that may not be read, which a write must then not replace.
fn read_checked(state_dir: &Path) -> io::Result<Vec<Remembered>> {
    Ok(read_file(state_dir, FILE, MAX_BYTES)?.map_or_else(Vec::new, |bytes| parse(&bytes)))
}

/// The bytes of the file called `name` in `state_dir`: `None` if there is none, and an error
/// if there is one that may not be read, which a write must then not replace. It is read only
/// if the directory is private and the file a regular one of this user's with one name, of at
/// most `max` bytes. The `spaces` file is read the same way (ADR 0136), the recent files
/// (ADR 0148), saved reviews (ADR 0122, step 6) and dictation's learning record, its log and the
/// list of what it added to TypeWhisper (ADR 0175, steps 3 and 4).
pub(crate) fn read_file(state_dir: &Path, name: &str, max: u64) -> io::Result<Option<Vec<u8>>> {
    let Some(directory) = PrivateDir::existing(state_dir)? else {
        return Ok(None);
    };
    let path = directory.path().join(name);
    let opened = match file_meta::open_regular_here(&path) {
        Ok(opened) => opened,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let refused = || {
        io::Error::other(format!(
            "{} is not a small regular file of its own",
            path.display()
        ))
    };
    let (file, meta) = opened.ok_or_else(refused)?;
    if !is_this_users(&meta) || !has_one_name(&meta) || meta.len() > max {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(refused());
    }
    Ok(Some(bytes))
}

/// The sessions in the file's bytes, the last of an id kept, at most [`MAX_SESSIONS`]: those
/// that changed last.
fn parse(bytes: &[u8]) -> Vec<Remembered> {
    let mut sessions: Vec<Remembered> = Vec::new();
    let mut bad = 0;
    for line in bytes.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Some(session) = parse_line(line) else {
            bad += 1;
            continue;
        };
        sessions.retain(|kept| kept.id != session.id);
        sessions.push(session);
    }
    if bad > 0 {
        log::warn!("left out {bad} lines of the remembered sessions that hold no session");
    }
    newest_first(&mut sessions);
    sessions.truncate(MAX_SESSIONS);
    sessions
}

/// Sorts `sessions` by when they last changed, the latest first; ties keep their order.
fn newest_first(sessions: &mut [Remembered]) {
    sessions.sort_by_key(|session| std::cmp::Reverse(session.changed));
}

/// One line's session, if it is whole and valid.
fn parse_line(line: &[u8]) -> Option<Remembered> {
    let value: Value = serde_json::from_slice(line).ok()?;
    let object = value.as_object()?;
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    let path = |text: &str| {
        let path = PathBuf::from(text);
        (text.len() <= MAX_PATH && path.is_absolute()).then_some(path)
    };
    let optional = |key: &str| match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.as_str())),
        Some(_) => None,
    };
    let flag = |key: &str| match object.get(key) {
        None => Some(false),
        Some(value) => value.as_bool(),
    };
    let name = text("name")?;
    if clean_name(name) != name {
        return None;
    }
    let root = match optional("root")? {
        Some(root) => Some(path(root)?),
        None => None,
    };
    let parent = match optional("parent")? {
        Some(parent) => Some(SessionId::parse(parent)?),
        None => None,
    };
    let origin = Origin::read(optional("origin")?, parent.is_some());
    let space = match optional("space")? {
        Some(space) => Some(SpaceId::parse(space)?),
        None => None,
    };
    let reads = match object.get("reads") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) if items.len() <= MAX_READS => (items.iter())
            .map(super::spaces::record::member)
            .collect::<Option<Vec<_>>>()?,
        Some(_) => return None,
    };
    let baseline = match optional("baseline")? {
        Some(tree) if is_tree_id(tree) => Some(tree.to_owned()),
        Some(_) => return None,
        None => None,
    };
    Some(Remembered {
        id: SessionId::parse(text("id")?)?,
        name: name.to_owned(),
        directory: path(text("directory")?)?,
        root,
        parent,
        origin,
        pinned: flag("pinned")?,
        archived: flag("archived")?,
        native: flag("native")?,
        changed: object.get("changed")?.as_u64()?,
        baseline,
        space,
        reads,
    })
}

/// Whether `text` is a git object's id: 40 or 64 lower-case hex digits.
fn is_tree_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One session as its line. A path that is not UTF-8 cannot be written, and its session is
/// not remembered.
fn line(session: &Remembered) -> Option<String> {
    let mut object = Map::new();
    object.insert("id".into(), json!(session.id.as_str()));
    object.insert("name".into(), json!(session.name));
    object.insert("directory".into(), json!(session.directory.to_str()?));
    if let Some(root) = &session.root {
        object.insert("root".into(), json!(root.to_str()?));
    }
    if let Some(parent) = &session.parent {
        object.insert("parent".into(), json!(parent.as_str()));
    }
    object.insert("origin".into(), json!(session.origin.name()));
    object.insert("pinned".into(), json!(session.pinned));
    object.insert("archived".into(), json!(session.archived));
    if session.native {
        object.insert("native".into(), json!(true));
    }
    object.insert("changed".into(), json!(session.changed));
    if let Some(baseline) = &session.baseline {
        object.insert("baseline".into(), json!(baseline));
    }
    if let Some(space) = &session.space {
        object.insert("space".into(), json!(space.as_str()));
    }
    if !session.reads.is_empty() {
        let reads = (session.reads.iter())
            .map(super::spaces::record::member_value)
            .collect::<Option<Vec<_>>>()?;
        object.insert("reads".into(), Value::Array(reads));
    }
    Some(Value::Object(object).to_string())
}

/// Writes `sessions` in `state_dir`, which is made private if it is missing, with the sessions
/// already there whose ids are not in `known`: another instance's. Those that changed last are
/// kept, at most [`MAX_SESSIONS`] and [`MAX_BYTES`] in all, so that the file can always be
/// read back. A file there that may not be read is not replaced.
pub fn write(
    state_dir: &Path,
    sessions: &[Remembered],
    known: &HashSet<SessionId>,
) -> io::Result<()> {
    let theirs = read_checked(state_dir)?;
    let directory = PrivateDir::open(state_dir)?;
    let mut text = String::new();
    let kept = theirs
        .into_iter()
        .filter(|session| !known.contains(&session.id))
        .filter(|session| sessions.iter().all(|ours| ours.id != session.id));
    let mut all: Vec<Remembered> = sessions.iter().cloned().chain(kept).collect();
    newest_first(&mut all);
    let most = usize::try_from(MAX_BYTES).unwrap_or(usize::MAX);
    for line in all.iter().filter_map(line).take(MAX_SESSIONS) {
        if text.len() + line.len() + 1 > most {
            break;
        }
        text.push_str(&line);
        text.push('\n');
    }
    replace_file(&directory, FILE, PART, &text)
}

/// Replaces the file called `name` in `directory` with `text` whole, through `part`, or
/// leaves it as it was. The `spaces` file is written the same way (ADR 0136), the recent
/// files (ADR 0148) and dictation's learning record (ADR 0175, step 3).
pub(crate) fn replace_file(
    directory: &PrivateDir,
    name: &str,
    part: &str,
    text: &str,
) -> io::Result<()> {
    let part = directory.path().join(part);
    match fs::remove_file(&part) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&part)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&part, directory.path().join(name))
}

/// How many directories above a session's the repositories are looked for in.
const STRANDED_DEPTH: usize = 64;

/// A session that may have been left behind by a worktree removed: its id, its directory and
/// the repository above it.
pub type Candidate = (SessionId, PathBuf, PathBuf);

/// The `sessions` that a worktree removed before oso archived on removal, or outside oso, may
/// have left behind (ADR 0121, "Removal archives its sessions"): not archived, in a directory
/// under one of the repositories `roots`, absolute, but not at it. Touches no disk; only the
/// candidates' repositories are then looked at.
pub fn stranded_candidates<'a>(
    sessions: &[Remembered],
    roots: impl IntoIterator<Item = &'a Path>,
) -> Vec<Candidate> {
    let roots: HashSet<&Path> = roots
        .into_iter()
        .filter(|root| root.is_absolute())
        .collect();
    if roots.is_empty() {
        return Vec::new();
    }
    let under = |dir: &Path| -> Option<PathBuf> {
        (dir.ancestors().skip(1).take(STRANDED_DEPTH))
            .find(|above| roots.contains(above))
            .map(Path::to_path_buf)
    };
    sessions
        .iter()
        .filter(|session| !session.archived)
        .filter_map(|session| {
            let root = under(&session.directory)?;
            Some((
                session.id.clone(),
                session.directory.clone(),
                root.to_path_buf(),
            ))
        })
        .collect()
}

/// The ids of the `candidates` that are stranded: their directory is not found, and their
/// repository's main checkout is there ([`holds_repository`]). A disk that is not mounted has
/// no `.git` there, so nothing on it is taken; nor is a directory that cannot be looked at, or
/// whose nearest directory above that is there is a link, as one to a disk not mounted is. Each
/// repository is looked at once. Looks at the disk.
///
/// [`holds_repository`]: super::spaces::probe::holds_repository
pub fn stranded(candidates: &[Candidate]) -> Vec<SessionId> {
    let mut checkouts: std::collections::HashMap<&Path, bool> = std::collections::HashMap::new();
    let mut found = Vec::new();
    for (id, directory, root) in candidates {
        let checkout = *checkouts
            .entry(root.as_path())
            .or_insert_with(|| super::spaces::probe::holds_repository(root));
        if checkout && gone(directory) {
            found.push(id.clone());
        }
    }
    found
}

/// Whether `directory` is not found, and the nearest directory above it that is there is a
/// directory, not a link: below a main checkout that is there, one is.
fn gone(directory: &Path) -> bool {
    let not_found = |e: &io::Error| e.kind() == io::ErrorKind::NotFound;
    let mut places = directory.ancestors();
    if !places
        .next()
        .is_some_and(|dir| fs::symlink_metadata(dir).is_err_and(|e| not_found(&e)))
    {
        return false;
    }
    for above in places.take(STRANDED_DEPTH) {
        match fs::symlink_metadata(above) {
            Ok(meta) => return meta.is_dir(),
            Err(e) if not_found(&e) => {}
            Err(_) => return false,
        }
    }
    false
}

// Hosted sessions run on Unix only (ADR 0120): the scratch paths and the private state
// directory these tests use are Unix ones.
#[cfg(all(test, unix))]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{FILE, MAX_SESSIONS, Remembered, parse_line, read, write};
    use crate::sessions::identity::Origin;
    use crate::sessions::identity::SessionId;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "oso-record-{name}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// oso's own directory in it, as the state directory gives it.
        fn oso(&self) -> PathBuf {
            self.0.join("oso")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn id(n: u8) -> SessionId {
        SessionId::from_random([n; 16])
    }

    fn session(n: u8) -> Remembered {
        Remembered {
            native: false,
            id: id(n),
            name: format!("oso-{n}"),
            directory: PathBuf::from("/work/oso"),
            root: Some(PathBuf::from("/work")),
            parent: None,
            origin: Origin::Person,
            pinned: false,
            archived: false,
            changed: 1_794_512_815_000,
            baseline: None,
            space: None,
            reads: Vec::new(),
        }
    }

    fn known(ids: &[u8]) -> HashSet<SessionId> {
        ids.iter().map(|n| id(*n)).collect()
    }

    #[test]
    fn only_sessions_whose_directory_is_gone_under_a_main_checkout_that_is_there_are_stranded() {
        use super::{stranded, stranded_candidates};
        let scratch = Scratch::new("stranded");
        let repo = scratch.0.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("crates/x")).unwrap();
        // A linked worktree's `.git` is a file: it is no main checkout.
        let linked = scratch.0.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(linked.join(".git"), "gitdir: elsewhere\n").unwrap();
        // Not mounted, say: nothing is there.
        let unmounted = scratch.0.join("volume/repo");
        // A link in the checkout to a disk not mounted.
        std::os::unix::fs::symlink(scratch.0.join("unplugged"), repo.join("vendor")).unwrap();
        // A main checkout whose `.git` is a link to a directory.
        let linked_git = scratch.0.join("linked-git");
        std::fs::create_dir_all(&linked_git).unwrap();
        std::os::unix::fs::symlink(repo.join(".git"), linked_git.join(".git")).unwrap();
        let at = |n: u8, dir: PathBuf, archived: bool| Remembered {
            directory: dir,
            archived,
            ..session(n)
        };
        let sessions = [
            at(1, repo.join(".claude/worktrees/gone"), false),
            at(2, repo.join(".claude/worktrees/gone/sub"), false),
            at(3, repo.join("crates/x"), false),
            at(4, repo.clone(), false),
            at(5, repo.join(".claude/worktrees/archived"), true),
            at(6, unmounted.join(".claude/worktrees/gone"), false),
            at(7, linked.join("gone"), false),
            at(8, scratch.0.join("elsewhere/gone"), false),
            at(9, repo.join("vendor/x"), false),
            at(10, linked_git.join("gone"), false),
        ];
        let roots = [
            repo.as_path(),
            repo.as_path(),
            &unmounted,
            &linked,
            &linked_git,
        ];
        let candidates = stranded_candidates(&sessions, roots);
        let ids: Vec<SessionId> = candidates.iter().map(|(id, ..)| id.clone()).collect();
        let under = vec![id(1), id(2), id(3), id(6), id(7), id(9), id(10)];
        assert_eq!(ids, under, "only those under a root");
        assert_eq!(stranded(&candidates), vec![id(1), id(2)]);
        assert!(
            stranded_candidates(&sessions, []).is_empty(),
            "no repository, none"
        );
    }

    #[test]
    fn the_workers_flush_answers_once_the_record_is_written() {
        use crate::sessions::worker::{Job, Worker};
        let scratch = Scratch::new("flush");
        let worker = Worker::start(|_| {}).expect("the worker starts");
        // Many writes queued, the last one wanted: the flush comes after them all.
        for n in 1..=20 {
            worker.send(Job::Remember {
                directory: scratch.oso(),
                sessions: (1..=n).map(session).collect(),
                known: known(&[]),
            });
        }
        assert!(worker.flush(std::time::Duration::from_secs(10)));
        assert_eq!(read(&scratch.oso()).len(), 20);
    }

    #[test]
    fn what_is_written_is_read_back() {
        let scratch = Scratch::new("round");
        assert!(read(&scratch.oso()).is_empty(), "nothing yet");
        let mut full = session(1);
        full.parent = Some(id(9));
        full.origin = Origin::Unasked;
        full.pinned = true;
        full.archived = true;
        // A native session (ADR 0165); a record without the key is a TUI's.
        full.native = true;
        full.baseline = Some("a".repeat(40));
        full.space = Some(crate::sessions::spaces::SpaceId::from_random([7; 16]));
        full.reads = vec![
            crate::sessions::spaces::Member {
                root: PathBuf::from("/work/web"),
                identity: Some((1, 2)),
            },
            crate::sessions::spaces::Member::at(std::path::Path::new("/work/db")),
        ];
        let mut bare = session(2);
        bare.root = None;
        bare.origin = Origin::Answered;
        bare.baseline = Some("0".repeat(64));
        let sessions = [full, bare];
        write(&scratch.oso(), &sessions, &known(&[1, 2])).unwrap();
        assert_eq!(read(&scratch.oso()), sessions);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(scratch.oso().join(FILE)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn an_old_line_is_the_person_s_unless_a_session_started_it_and_an_unknown_one_is_neither() {
        let line = |more: &str| {
            format!(
                r#"{{"id":"{}","name":"oso-1","directory":"/work/oso","pinned":false,"archived":false,"changed":1{more}}}"#,
                id(1).as_str()
            )
        };
        let origin = |text: &str| parse_line(text.as_bytes()).map(|session| session.origin);
        let parent = format!(r#","parent":"{}""#, id(2).as_str());
        assert_eq!(origin(&line("")), Some(Origin::Person));
        assert_eq!(origin(&line(&parent)), Some(Origin::Unknown));
        assert_eq!(origin(&line(r#","origin":"person""#)), Some(Origin::Person));
        assert_eq!(
            origin(&line(r#","origin":"answered""#)),
            Some(Origin::Answered)
        );
        assert_eq!(
            origin(&line(r#","origin":"unasked""#)),
            Some(Origin::Unasked)
        );
        assert_eq!(origin(&line(r#","origin":"later""#)), Some(Origin::Unknown));
        assert_eq!(origin(&line(r#","origin":3"#)), None, "not a string");
        assert!(
            super::line(&session(1))
                .unwrap()
                .contains(r#""origin":"person""#),
            "written for every session"
        );
    }

    #[test]
    fn at_most_so_many_sessions_and_bytes() {
        let scratch = Scratch::new("caps");
        let many: Vec<Remembered> = (0..300u64)
            .map(|i| {
                let mut bytes = [0u8; 16];
                bytes[..8].copy_from_slice(&i.to_le_bytes());
                Remembered {
                    id: SessionId::from_random(bytes),
                    changed: i,
                    ..session(0)
                }
            })
            .collect();
        assert_eq!(many.len(), 300);
        write(&scratch.oso(), &many, &HashSet::new()).unwrap();
        let read_back = read(&scratch.oso());
        assert_eq!(read_back.len(), MAX_SESSIONS);
        // Those that changed last are kept, the latest first.
        let newest: Vec<Remembered> = many.iter().rev().take(MAX_SESSIONS).cloned().collect();
        assert_eq!(read_back, newest);
        // Long lines are kept only as far as the file can be read back.
        let long: Vec<Remembered> = many
            .iter()
            .map(|session| Remembered {
                directory: PathBuf::from(format!("/{}", "d".repeat(4000))),
                ..session.clone()
            })
            .collect();
        write(&scratch.oso(), &long, &HashSet::new()).unwrap();
        let size = std::fs::metadata(scratch.oso().join(FILE)).unwrap().len();
        assert!(size <= super::MAX_BYTES, "{size}");
        let read_back = read(&scratch.oso());
        assert!(!read_back.is_empty() && read_back.len() < MAX_SESSIONS);
        assert_eq!(read_back[0].changed, 299);
        // A file past the byte limit is not read at all.
        let text = std::fs::read_to_string(scratch.oso().join(FILE)).unwrap();
        let times = usize::try_from(super::MAX_BYTES).unwrap() / text.len() + 1;
        std::fs::write(scratch.oso().join(FILE), text.repeat(times)).unwrap();
        assert!(read(&scratch.oso()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_neither_read_nor_replaced() {
        let scratch = Scratch::new("link");
        let elsewhere = Scratch::new("link-elsewhere");
        write(&elsewhere.oso(), &[session(1)], &known(&[1])).unwrap();
        write(&scratch.oso(), &[], &HashSet::new()).unwrap();
        let file = scratch.oso().join(FILE);
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(elsewhere.oso().join(FILE), &file).unwrap();
        assert!(read(&scratch.oso()).is_empty());
        assert!(write(&scratch.oso(), &[session(2)], &known(&[2])).is_err());
        assert_eq!(read(&elsewhere.oso()), [session(1)], "left as it was");
        assert!(
            std::fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn another_instances_sessions_are_kept() {
        let scratch = Scratch::new("merge");
        // The other instance wrote 1 and 2.
        write(&scratch.oso(), &[session(1), session(2)], &known(&[1, 2])).unwrap();
        // This one knew 3 and 4, and no longer has 4.
        write(&scratch.oso(), &[session(3), session(4)], &known(&[3, 4])).unwrap();
        write(&scratch.oso(), &[session(3)], &known(&[3, 4])).unwrap();
        assert_eq!(read(&scratch.oso()), [session(3), session(1), session(2)]);
        // A session of ours replaces the other instance's line of it.
        let mut changed = session(1);
        changed.pinned = true;
        write(&scratch.oso(), &[changed.clone()], &known(&[1])).unwrap();
        assert_eq!(read(&scratch.oso()), [changed, session(3), session(2)]);
    }

    #[test]
    fn unknown_fields_and_bad_lines_are_not_kept() {
        let scratch = Scratch::new("fields");
        write(&scratch.oso(), &[], &HashSet::new()).unwrap();
        let one = session(1);
        let lines = [
            format!(
                r#"{{"id":"{}","name":"oso-1","directory":"/work/oso","root":"/work","pinned":false,"archived":false,"changed":1794512815000,"screen":"secret text"}}"#,
                one.id
            ),
            "not json".to_owned(),
            // A name that does not clean to itself.
            format!(
                r#"{{"id":"{}","name":"--rm","directory":"/w","changed":1}}"#,
                id(2)
            ),
            // A relative directory.
            format!(
                r#"{{"id":"{}","name":"a","directory":"w","changed":1}}"#,
                id(3)
            ),
            // A baseline that is no tree id.
            format!(
                r#"{{"id":"{}","name":"a","directory":"/w","changed":1,"baseline":"HEAD"}}"#,
                id(4)
            ),
            // An id that is no UUID.
            r#"{"id":"x","name":"a","directory":"/w","changed":1}"#.to_owned(),
        ];
        std::fs::write(scratch.oso().join(FILE), lines.join("\n")).unwrap();
        assert_eq!(read(&scratch.oso()), std::slice::from_ref(&one));
        write(&scratch.oso(), &[], &HashSet::new()).unwrap();
        let text = std::fs::read_to_string(scratch.oso().join(FILE)).unwrap();
        assert!(
            !text.contains("screen") && !text.contains("secret"),
            "{text}"
        );
        assert_eq!(read(&scratch.oso()), [one]);
    }

    #[test]
    fn the_last_line_of_an_id_wins() {
        let scratch = Scratch::new("duplicate");
        write(&scratch.oso(), &[session(1)], &known(&[1])).unwrap();
        let file = scratch.oso().join(FILE);
        let text = std::fs::read_to_string(&file).unwrap();
        let pinned = text.replace(r#""pinned":false"#, r#""pinned":true"#);
        std::fs::write(&file, format!("{text}{pinned}")).unwrap();
        let read_back = read(&scratch.oso());
        assert_eq!(read_back.len(), 1);
        assert!(read_back[0].pinned);
    }
}
