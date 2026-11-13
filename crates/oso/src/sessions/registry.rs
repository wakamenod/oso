//! Claude Code's session registry (ADR 0121, "Restore and the fork hazard"): the file each
//! running `claude` keeps in `sessions` under its configuration directory, named by its
//! process id, and whether the process it names still runs.
//!
//! Must not depend on: threads, the UI or `app`; the caller reads on a thread of its own, never
//! the UI thread, and gives the process table's answer as a function.
//!
//! - Only names of the form `<digits>.json` are opened. Other files sit beside them, and oso
//!   never reads them.
//! - At most [`MAX_FILES`] files, each a regular file of this user's, opened without following
//!   a link, and at most [`MAX_BYTES`] of each. A larger one is left out.
//! - Only the fields oso needs are kept, each bounded; the rest of an entry is not.
//! - An entry is live if its process started when the entry says, within [`PROC_START_SLACK`]
//!   of `procStart` or [`STARTED_AT_SLACK`] of `startedAt`: a process id the system gave
//!   again to another process is not the session's.

use std::ffi::OsString;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oso_platform::file_meta;
use oso_platform::privacy::is_this_users;
use serde_json::Value;

use super::identity::SessionId;

/// The most registry files read.
pub const MAX_FILES: usize = 256;

/// The most bytes read of one registry file; a larger one is left out.
pub const MAX_BYTES: u64 = 64 * 1024;

/// The longest name, kind or status kept, in bytes.
const MAX_FIELD: usize = 256;

/// The longest directory kept, in bytes.
const MAX_CWD: usize = 4096;

/// The most digits of a process id in a file's name.
const MAX_PID_DIGITS: usize = 10;

/// How far a process's start may be from `procStart`, which gives whole seconds.
pub const PROC_START_SLACK: Duration = Duration::from_secs(2);

/// How far a process's start may be from `startedAt`, which the CLI writes once it runs.
pub const STARTED_AT_SLACK: Duration = Duration::from_secs(10);

/// One registry entry: the fields oso reads, each `None` if missing or out of bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The process, as its file's name gives it too.
    pub pid: u32,
    pub session_id: Option<SessionId>,
    /// The directory it runs in.
    pub cwd: Option<PathBuf>,
    /// When the process started, as ctime text in UTC, such as `Thu Nov 12 19:46:55 2026`.
    pub proc_start: Option<String>,
    /// When the CLI started, in milliseconds since the epoch.
    pub started_at: Option<u64>,
    pub name: Option<String>,
    pub kind: Option<String>,
    /// The CLI's own status, such as `busy` or `idle`.
    pub status: Option<String>,
}

/// Whether an entry's process still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Yes,
    /// No such process, or one that started at another time: the id was given again.
    No,
    /// The entry says nothing of when its process started, or the system does not say when the
    /// process with its pid did. Callers count it as live: refusing a resume is the safe side.
    Unknown,
}

/// The variable that moves Claude Code's configuration, and with it the registry. A session
/// is always given oso's own, so that the registry it writes is the one oso reads.
pub const CONFIG_VARIABLE: &str = "CLAUDE_CONFIG_DIR";

/// The registry's directory: `sessions` under `$CLAUDE_CONFIG_DIR`, else under `~/.claude`.
/// `None` when it cannot be told: `$CLAUDE_CONFIG_DIR` is not an absolute path, which a
/// session would read from its own directory, or there is no absolute home
/// (`oso_platform::privacy::home`).
pub fn directory() -> Option<PathBuf> {
    directory_from(
        std::env::var_os(CONFIG_VARIABLE),
        oso_platform::privacy::home(),
    )
}

/// `home` is absolute, as `oso_platform::privacy::home` gives it.
fn directory_from(config_dir: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = config_dir {
        return Some(PathBuf::from(dir))
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("sessions"));
    }
    home.map(|home| home.join(".claude").join("sessions"))
}

/// The entries in `dir`, in no order. A directory that cannot be listed has none.
pub fn read(dir: &Path) -> Vec<Entry> {
    list(dir).entries
}

/// What a read of the registry found.
#[derive(Debug, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// It holds more than [`MAX_FILES`] entry files, so some were not read: whether a session
    /// runs cannot be told from it.
    pub cut: bool,
}

/// The entries in `dir`, in no order, and whether files were left unread. A directory that
/// cannot be listed has none.
pub fn list(dir: &Path) -> Listing {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Listing::default();
    };
    let mut files: Vec<(u32, PathBuf)> = listing
        .filter_map(Result::ok)
        .filter_map(|item| pid_of(&item.file_name()).map(|pid| (pid, item.path())))
        .take(MAX_FILES + 1)
        .collect();
    let cut = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    let entries = files
        .into_iter()
        .filter_map(|(pid, path)| read_entry(&path, pid))
        .collect();
    Listing { entries, cut }
}

/// The process id a file called `name` is for, if it is `<digits>.json`.
fn pid_of(name: &std::ffi::OsStr) -> Option<u32> {
    let digits = name.to_str()?.strip_suffix(".json")?;
    let plain =
        (1..=MAX_PID_DIGITS).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit());
    plain.then(|| digits.parse().ok()).flatten()
}

/// The entry in the file at `path`, which must be for `pid`.
fn read_entry(path: &Path, pid: u32) -> Option<Entry> {
    let (file, meta) = file_meta::open_regular_here(path).ok()??;
    if !is_this_users(&meta) || meta.len() > MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let entry = parse(&bytes)?;
    (entry.pid == pid).then_some(entry)
}

/// An entry from a registry file's bytes, if they hold an object with a process id.
fn parse(bytes: &[u8]) -> Option<Entry> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let object = value.as_object()?;
    let pid = u32::try_from(object.get("pid")?.as_u64()?).ok()?;
    let text = |key: &str, most: usize| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| text.len() <= most)
            .map(str::to_owned)
    };
    Some(Entry {
        pid,
        session_id: object
            .get("sessionId")
            .and_then(Value::as_str)
            .and_then(SessionId::parse),
        cwd: text("cwd", MAX_CWD).map(PathBuf::from),
        proc_start: text("procStart", MAX_FIELD),
        started_at: object.get("startedAt").and_then(Value::as_u64),
        name: text("name", MAX_FIELD),
        kind: text("kind", MAX_FIELD),
        status: text("status", MAX_FIELD),
    })
}

/// Whether `entry`'s process still runs, by `started`, which gives when a process started,
/// `None` if there is none, or an error if the system does not say:
/// `oso_platform::process::start_time`.
pub fn liveness(entry: &Entry, started: impl Fn(u32) -> io::Result<Option<SystemTime>>) -> Live {
    let start = match started(entry.pid) {
        Ok(Some(start)) => start,
        Ok(None) => return Live::No,
        Err(_) => return Live::Unknown,
    };
    let near = |then: SystemTime, slack: Duration| {
        let apart = start
            .duration_since(then)
            .or_else(|_| then.duration_since(start))
            .unwrap_or(Duration::MAX);
        if apart <= slack { Live::Yes } else { Live::No }
    };
    if let Some(then) = entry.proc_start.as_deref().and_then(parse_ctime) {
        return near(then, PROC_START_SLACK);
    }
    match entry.started_at {
        Some(ms) => near(UNIX_EPOCH + Duration::from_millis(ms), STARTED_AT_SLACK),
        None => Live::Unknown,
    }
}

/// The entries of the session `id` whose process may still run: live, or unknown.
pub fn live_with<'a>(
    entries: &'a [Entry],
    id: &SessionId,
    started: impl Fn(u32) -> io::Result<Option<SystemTime>>,
) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|entry| entry.session_id.as_ref() == Some(id))
        .filter(|entry| liveness(entry, &started) != Live::No)
        .collect()
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// ctime text in UTC, such as `Thu Nov 12 19:46:55 2026` or `Mon Nov  2 01:02:03 2026`, as a
/// time. The weekday must be one, and is not checked against the date.
fn parse_ctime(text: &str) -> Option<SystemTime> {
    let mut parts = text.split_ascii_whitespace();
    let (weekday, month, day, clock, year) = (
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
        parts.next()?,
    );
    if parts.next().is_some() || !WEEKDAYS.contains(&weekday) {
        return None;
    }
    let month = MONTHS.iter().position(|name| *name == month)? + 1;
    let number = |text: &str, most: usize| {
        (!text.is_empty() && text.len() <= most && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<i64>().ok())
            .flatten()
    };
    let day = number(day, 2).filter(|day| (1..=31).contains(day))?;
    let year = number(year, 4).filter(|year| *year >= 1970)?;
    let mut clock = clock.split(':');
    let (hour, minute, second) = (
        number(clock.next()?, 2).filter(|n| *n < 24)?,
        number(clock.next()?, 2).filter(|n| *n < 60)?,
        number(clock.next()?, 2).filter(|n| *n <= 60)?,
    );
    if clock.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, i64::try_from(month).ok()?, day);
    let seconds = u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Days from 1970-01-01 to the date, in the proleptic Gregorian calendar (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{
        Entry, Live, MAX_FILES, directory_from, list, live_with, liveness, parse_ctime, pid_of,
        read,
    };
    use crate::sessions::identity::SessionId;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "oso-registry-{name}-{}-{:?}",
                std::process::id(),
                SystemTime::now()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(name), text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const ID: &str = "0123abcd-4567-4890-abcd-ef0123456789";

    fn id() -> SessionId {
        SessionId::parse(ID).unwrap()
    }

    fn entry_json(pid: u32, proc_start: &str) -> String {
        format!(
            r#"{{"pid":{pid},"sessionId":"{ID}","cwd":"/work/oso","procStart":"{proc_start}","startedAt":1794512815000,"name":"oso","kind":"interactive","status":"idle","messagingSocketPath":"/x","version":"2.1.322"}}"#
        )
    }

    fn entry(pid: u32) -> Entry {
        Entry {
            pid,
            session_id: Some(id()),
            cwd: None,
            proc_start: None,
            started_at: None,
            name: None,
            kind: None,
            status: None,
        }
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// `time` as ctime text in UTC, as the CLI writes `procStart`.
    fn ctime(time: SystemTime) -> String {
        let seconds = time.duration_since(UNIX_EPOCH).unwrap().as_secs();
        let (days, rest) = (seconds / 86_400, seconds % 86_400);
        let days = i64::try_from(days).unwrap();
        // Hinnant's civil_from_days.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize];
        let month = super::MONTHS[(month - 1) as usize];
        format!(
            "{weekday} {month} {day:>2} {:02}:{:02}:{:02} {year}",
            rest / 3600,
            rest / 60 % 60,
            rest % 60
        )
    }

    #[test]
    fn the_directory_is_under_the_configuration_or_home() {
        let some = |text: &str| Some(OsString::from(text));
        let home = Some(PathBuf::from("/h"));
        assert_eq!(
            directory_from(some("/c"), home.clone()),
            Some(PathBuf::from("/c/sessions"))
        );
        // A session would read a relative one from its own directory: oso cannot tell where.
        assert_eq!(directory_from(some("relative"), home.clone()), None);
        assert_eq!(
            directory_from(None, home),
            Some(PathBuf::from("/h/.claude/sessions"))
        );
        // `oso_platform::privacy::home` gives no relative home: its own tests check that.
        assert_eq!(directory_from(None, None), None);
    }

    #[test]
    fn only_digits_and_json_name_an_entry() {
        assert_eq!(pid_of("123.json".as_ref()), Some(123));
        for name in [
            "x.json",
            "5.json.tmp",
            "5.key",
            "5.abc.key",
            ".json",
            "12345678901.json",
            "+5.json",
            "5.JSON",
        ] {
            assert_eq!(pid_of(name.as_ref()), None, "{name}");
        }
    }

    #[test]
    fn ctime_text_parses_in_utc() {
        assert_eq!(parse_ctime("Thu Jan  1 00:00:00 1970"), Some(at(0)));
        assert_eq!(
            parse_ctime("Thu Nov 12 19:46:55 2026"),
            Some(at(1_794_512_815))
        );
        assert_eq!(
            parse_ctime("Mon Feb  2 03:04:05 2026"),
            Some(at(1_770_001_445))
        );
        assert_eq!(
            parse_ctime("Tue Feb 29 23:59:59 2000"),
            Some(at(951_868_799))
        );
        for bad in [
            "",
            "Xyz Nov 12 19:46:55 2026",
            "Thu Foo 12 19:46:55 2026",
            "Thu Nov 32 19:46:55 2026",
            "Thu Nov 12 24:00:00 2026",
            "Thu Nov 12 19:46 2026",
            "Thu Nov 12 19:46:55:00 2026",
            "Thu Nov 12 19:46:55 2026 extra",
            "Thu Nov 12 19:46:55 1969",
            "Thu Nov +1 19:46:55 2026",
        ] {
            assert_eq!(parse_ctime(bad), None, "{bad:?}");
        }
        for seconds in [0, 86_399, 951_868_799, 1_794_512_815, 4_102_444_800] {
            assert_eq!(parse_ctime(&ctime(at(seconds))), Some(at(seconds)));
        }
    }

    #[test]
    fn liveness_holds_within_its_slack() {
        let started = |pid| Ok((pid == 7).then(|| at(1_000)));
        let mut entry = entry(7);
        assert_eq!(liveness(&entry, started), Live::Unknown);
        entry.started_at = Some(1_010_000);
        assert_eq!(liveness(&entry, started), Live::Yes, "10 s after");
        entry.started_at = Some(1_010_001);
        assert_eq!(liveness(&entry, started), Live::No, "past 10 s");
        entry.started_at = Some(990_000);
        assert_eq!(liveness(&entry, started), Live::Yes, "10 s before");
        // procStart wins over startedAt, with 2 s.
        entry.proc_start = Some(ctime(at(1_002)));
        assert_eq!(liveness(&entry, started), Live::Yes);
        entry.proc_start = Some(ctime(at(1_003)));
        assert_eq!(liveness(&entry, started), Live::No);
        entry.proc_start = Some(ctime(at(998)));
        assert_eq!(liveness(&entry, started), Live::Yes);
        // procStart that does not parse falls back to startedAt.
        entry.proc_start = Some("garbage".into());
        assert_eq!(liveness(&entry, started), Live::Yes);
        // No such process.
        assert_eq!(
            liveness(
                &super::Entry {
                    pid: 8,
                    ..entry.clone()
                },
                started
            ),
            Live::No
        );
        // A process whose start the system does not say may run.
        let untold = |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(liveness(&entry, untold), Live::Unknown);
    }

    #[test]
    fn live_with_keeps_live_and_unknown_entries_of_the_id() {
        let started = |pid| match pid {
            3 => Ok(None),
            6 => Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            _ => Ok(Some(at(1_000))),
        };
        let mut live = entry(1);
        live.proc_start = Some(ctime(at(1_000)));
        let unknown = entry(2);
        let dead = entry(3);
        let mut reused = entry(4);
        reused.proc_start = Some(ctime(at(90_000)));
        let mut other = entry(5);
        other.session_id = SessionId::parse("ffffffff-ffff-4fff-bfff-ffffffffffff");
        let mut untold = entry(6);
        untold.proc_start = Some(ctime(at(1_000)));
        let entries = [live, unknown, dead, reused, other, untold];
        let found: Vec<u32> = live_with(&entries, &id(), started)
            .iter()
            .map(|entry| entry.pid)
            .collect();
        assert_eq!(found, [1, 2, 6]);
    }

    #[test]
    fn an_entry_keeps_only_its_fields() {
        let scratch = Scratch::new("fields");
        scratch.write("42.json", &entry_json(42, "Thu Nov 12 19:46:55 2026"));
        let entries = read(&scratch.0);
        assert_eq!(
            entries,
            [Entry {
                pid: 42,
                session_id: Some(id()),
                cwd: Some(PathBuf::from("/work/oso")),
                proc_start: Some("Thu Nov 12 19:46:55 2026".into()),
                started_at: Some(1_794_512_815_000),
                name: Some("oso".into()),
                kind: Some("interactive".into()),
                status: Some("idle".into()),
            }]
        );
    }

    #[test]
    fn bad_files_are_left_out() {
        let scratch = Scratch::new("bad");
        scratch.write("1.json", "not json");
        scratch.write("2.json", "[1, 2]");
        scratch.write("3.json", r#"{"sessionId": "x"}"#);
        // The pid must be the file's.
        scratch.write("4.json", &entry_json(5, ""));
        // Too large.
        let mut large = entry_json(6, "");
        large.push_str(&" ".repeat(64 * 1024));
        scratch.write("6.json", &large);
        // Names that are no entry's are never opened.
        for name in ["7.json.tmp", "x.json", "7.key", "7.abc.key"] {
            scratch.write(name, &entry_json(7, ""));
        }
        // A bad field is dropped, not the entry.
        scratch.write(
            "8.json",
            &format!(
                r#"{{"pid": 8, "sessionId": "bad", "name": "{}"}}"#,
                "n".repeat(300)
            ),
        );
        let entries = read(&scratch.0);
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].pid, 8);
        assert_eq!(entries[0].session_id, None);
        assert_eq!(entries[0].name, None);
        assert!(read(&scratch.0.join("missing")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_is_not_followed() {
        let scratch = Scratch::new("link");
        scratch.write("real", &entry_json(5, ""));
        std::os::unix::fs::symlink(scratch.0.join("real"), scratch.0.join("5.json")).unwrap();
        assert!(read(&scratch.0).is_empty());
    }

    /// A FIFO named as the files beside the entries are: opening it would wait for a writer.
    #[cfg(unix)]
    #[test]
    fn the_files_beside_the_entries_are_never_opened() {
        use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
        let scratch = Scratch::new("fifo");
        scratch.write("9.json", &entry_json(9, ""));
        let fifo = scratch.0.join("9.0123abcd.key");
        let Ok(mkfifo) = Executable::from_path("mkfifo", None) else {
            return;
        };
        let args = [fifo.clone().into_os_string()];
        let mut child = spawn(&Spawn {
            executable: &mkfifo,
            args: &args,
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .unwrap();
        assert!(child.wait().unwrap().success());
        assert!(std::fs::symlink_metadata(&fifo).is_ok());
        // Would block forever if the FIFO were opened.
        let entries = read(&scratch.0);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn at_most_so_many_files_are_read() {
        let scratch = Scratch::new("many");
        for pid in 1..=300 {
            scratch.write(&format!("{pid}.json"), &entry_json(pid, ""));
        }
        assert_eq!(read(&scratch.0).len(), MAX_FILES);
        let listing = list(&scratch.0);
        assert!(listing.cut, "more were there than were read");
        assert_eq!(listing.entries.len(), MAX_FILES);
        std::fs::remove_file(scratch.0.join("300.json")).unwrap();
        for pid in 257..300 {
            std::fs::remove_file(scratch.0.join(format!("{pid}.json"))).unwrap();
        }
        assert!(!list(&scratch.0).cut, "exactly as many as are read");
    }

    /// The pid of a child that has ended and been reaped.
    #[cfg(unix)]
    fn reaped_pid() -> u32 {
        use oso_platform::process::{Executable, Sink, Source, Spawn, spawn};
        let sh = Executable::absolute("/bin/sh").unwrap();
        let args = ["-c".into(), "exit 0".into()];
        let mut child = spawn(&Spawn {
            executable: &sh,
            args: &args,
            cwd: None,
            project: None,
            inherit: &[],
            stdin: Source::Closed,
            stdout: Sink::Null,
            stderr: Sink::Null,
            own_process_group: false,
            set: &[],
            terminal: None,
        })
        .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    /// Against the real process table: a stale file, a pid given again, and a live one.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn liveness_reads_the_process_table() {
        use oso_platform::process::start_time;
        let scratch = Scratch::new("table");
        let me = std::process::id();
        let mine = start_time(me).unwrap().unwrap();
        scratch.write(&format!("{me}.json"), &entry_json(me, &ctime(mine)));
        let stale = reaped_pid();
        scratch.write(&format!("{stale}.json"), &entry_json(stale, &ctime(mine)));
        let entries = read(&scratch.0);
        let of = |pid: u32| entries.iter().find(|entry| entry.pid == pid).unwrap();
        assert_eq!(liveness(of(me), start_time), Live::Yes);
        assert_eq!(liveness(of(stale), start_time), Live::No);
        // This process's id, written as if another process had it a day before.
        let reused = Entry {
            proc_start: Some(ctime(mine - Duration::from_secs(86_400))),
            ..of(me).clone()
        };
        assert_eq!(liveness(&reused, start_time), Live::No);
        let live: Vec<u32> = live_with(&entries, &id(), start_time)
            .iter()
            .map(|entry| entry.pid)
            .collect();
        assert_eq!(live, [me]);
    }
}
