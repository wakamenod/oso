//! The sessions oso remembers (ADR 0121, "Restore and the fork hazard"): `oso/sessions` in the
//! per-user state directory, beside `theme`, one JSON object a line. A start of oso brings each
//! back stopped; nothing in the file starts a process.
//!
//! Must not depend on: threads, the UI or `app`; the sessions worker reads and writes it, never
//! the UI thread.
//!
//! - It holds each session's id, name, directory, project root, parent, pin and archive marks,
//!   when it last changed and its baseline. No screen text and no transcript text.
//! - It is read only if the directory is private and the file a regular one of this user's
//!   with one name, at most [`MAX_BYTES`] and [`MAX_SESSIONS`] sessions. A line that does not
//!   hold a whole, valid session is left out, and only how many were is logged.
//! - A write replaces the file whole, through a `.part`, or leaves it as it was. It reads the
//!   file first and keeps the sessions this instance never knew, so that two instances do not
//!   drop each other's.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use oso_platform::file_meta;
use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};
use serde_json::{Map, Value, json};

use super::identity::{SessionId, clean_name};

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
    pub pinned: bool,
    /// Archived: not brought back at start.
    pub archived: bool,
    /// When it last changed state, in milliseconds since the epoch.
    pub changed: u64,
    /// Its baseline for a review (ADR 0122): a git tree's id, 40 or 64 lower-case hex digits.
    pub baseline: Option<String>,
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
    let Some(directory) = PrivateDir::existing(state_dir)? else {
        return Ok(Vec::new());
    };
    let path = directory.path().join(FILE);
    let opened = match file_meta::open_regular_here(&path) {
        Ok(opened) => opened,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let refused = || {
        io::Error::other(format!(
            "{} is not a small regular file of its own",
            path.display()
        ))
    };
    let (file, meta) = opened.ok_or_else(refused)?;
    if !is_this_users(&meta) || !has_one_name(&meta) || meta.len() > MAX_BYTES {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(refused());
    }
    Ok(parse(&bytes))
}

/// The sessions in the file's bytes, the last of an id kept, at most [`MAX_SESSIONS`].
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
    sessions.truncate(MAX_SESSIONS);
    sessions
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
        pinned: flag("pinned")?,
        archived: flag("archived")?,
        changed: object.get("changed")?.as_u64()?,
        baseline,
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
    object.insert("pinned".into(), json!(session.pinned));
    object.insert("archived".into(), json!(session.archived));
    object.insert("changed".into(), json!(session.changed));
    if let Some(baseline) = &session.baseline {
        object.insert("baseline".into(), json!(baseline));
    }
    Some(Value::Object(object).to_string())
}

/// Writes `sessions` in `state_dir`, which is made private if it is missing, with the sessions
/// already there whose ids are not in `known`: another instance's. Ours come first, and at most
/// [`MAX_SESSIONS`] in all. A file there that may not be read is not replaced.
pub fn write(
    state_dir: &Path,
    sessions: &[Remembered],
    known: &HashSet<SessionId>,
) -> io::Result<()> {
    let theirs = read_checked(state_dir)?;
    let directory = PrivateDir::open(state_dir)?;
    let mut text = String::new();
    let ours = sessions.iter();
    let kept = theirs
        .iter()
        .filter(|session| !known.contains(&session.id))
        .filter(|session| sessions.iter().all(|ours| ours.id != session.id));
    for line in ours.chain(kept).filter_map(line).take(MAX_SESSIONS) {
        text.push_str(&line);
        text.push('\n');
    }
    let part = directory.path().join(PART);
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
    fs::rename(&part, directory.path().join(FILE))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::{FILE, MAX_SESSIONS, Remembered, read, write};
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
            id: id(n),
            name: format!("oso-{n}"),
            directory: PathBuf::from("/work/oso"),
            root: Some(PathBuf::from("/work")),
            parent: None,
            pinned: false,
            archived: false,
            changed: 1_794_512_815_000,
            baseline: None,
        }
    }

    fn known(ids: &[u8]) -> HashSet<SessionId> {
        ids.iter().map(|n| id(*n)).collect()
    }

    #[test]
    fn what_is_written_is_read_back() {
        let scratch = Scratch::new("round");
        assert!(read(&scratch.oso()).is_empty(), "nothing yet");
        let mut full = session(1);
        full.parent = Some(id(9));
        full.pinned = true;
        full.archived = true;
        full.baseline = Some("a".repeat(40));
        let mut bare = session(2);
        bare.root = None;
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
    fn at_most_so_many_sessions_and_bytes() {
        let scratch = Scratch::new("caps");
        let many: Vec<Remembered> = (0..300u64)
            .map(|i| {
                let mut bytes = [0u8; 16];
                bytes[..8].copy_from_slice(&i.to_le_bytes());
                Remembered {
                    id: SessionId::from_random(bytes),
                    ..session(0)
                }
            })
            .collect();
        assert_eq!(many.len(), 300);
        write(&scratch.oso(), &many, &HashSet::new()).unwrap();
        let read_back = read(&scratch.oso());
        assert_eq!(read_back.len(), MAX_SESSIONS);
        assert_eq!(read_back[..], many[..MAX_SESSIONS]);
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
