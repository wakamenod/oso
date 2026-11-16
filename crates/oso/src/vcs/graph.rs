//! A repository's commits as a graph (ADR 0127, step 4): what `git log` gives for each commit,
//! read into rows, the refs it names as labels, its date, and the arguments that read a page;
//! and what a graph's keys run (`GraphRun`, step 4b): a tag, a cherry-pick and a revert.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; these are pure functions of a
//! child's output, which is untrusted: a record that does not parse is skipped, and nothing
//! here panics or grows past its input.
//!
//! - Pages of [`PAGE`] commits, at most [`MAX_COMMITS`] in all (Q4); each page reads one more
//!   to tell that there are more.
//! - The lanes are oso's own ([`lanes`]), not git's `--graph` text (Q9).
//! - A subject and an author are shown on one line, escaped and cut as a commit section's are.

pub(crate) mod lanes;
pub(crate) mod work;
pub(crate) mod write;

use super::history;

/// The commits one page reads.
pub(crate) const PAGE: usize = 2000;
/// The most commits a graph shows (Q4).
pub(crate) const MAX_COMMITS: usize = 50_000;
/// The most bytes of one page's log read: past it, the commits after are not shown.
pub(crate) const MAX_PAGE: usize = 16 * 1024 * 1024;
/// The most characters of an author's name shown.
const MAX_AUTHOR: usize = 40;
/// The `git log` format each record is read with: the full id, the short one, the parents, the
/// refs, the author, the time and the subject.
const FORMAT: &str = "--format=%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%s";

/// What a graph's key runs, with the repository's own hooks (step 4b).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GraphRun {
    /// Make the tag `name` at `at`, annotated with `message` if there is one.
    Tag {
        name: String,
        at: String,
        message: Option<String>,
    },
    /// Cherry-pick `ids`, oldest first.
    Pick { ids: Vec<String> },
    /// Revert `id`, with git's message.
    Revert { id: String },
}

impl GraphRun {
    /// git's arguments after oso's own, and what goes on standard input.
    pub(crate) fn args(&self) -> (Vec<String>, Option<Vec<u8>>) {
        let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        match self {
            Self::Tag {
                name,
                at,
                message: None,
            } => (words(&["tag", name, at]), None),
            Self::Tag {
                name,
                at,
                message: Some(message),
            } => (
                words(&["tag", "--annotate", "--file=-", name, at]),
                Some(message.clone().into_bytes()),
            ),
            Self::Pick { ids } => {
                let mut out: Vec<String> = words(&["cherry-pick"]);
                out.extend(ids.iter().cloned());
                (out, None)
            }
            Self::Revert { id } => (words(&["revert", "--no-edit", id]), None),
        }
    }

    /// What the log and the echo area call it.
    pub(crate) fn title(&self) -> String {
        match self {
            Self::Tag { name, .. } => format!("git tag {name}"),
            Self::Pick { ids } if ids.len() == 1 => "git cherry-pick".into(),
            Self::Pick { ids } => format!("git cherry-pick of {} commits", ids.len()),
            Self::Revert { .. } => "git revert".into(),
        }
    }

    /// What the echo area says once it went well.
    pub(crate) fn done(&self) -> String {
        match self {
            Self::Tag { name, at, .. } => format!("Tagged {} as {name}", at.get(..7).unwrap_or(at)),
            Self::Pick { ids } if ids.len() == 1 => "Picked the commit".into(),
            Self::Pick { ids } => format!("Picked {} commits", ids.len()),
            Self::Revert { .. } => "Reverted the commit".into(),
        }
    }

    /// The ids it names, which must be whole.
    pub(crate) fn ids(&self) -> Vec<&str> {
        match self {
            Self::Tag { at, .. } => vec![at.as_str()],
            Self::Pick { ids } => ids.iter().map(String::as_str).collect(),
            Self::Revert { id } => vec![id.as_str()],
        }
    }
}

/// What a ref's label is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Head,
    Local,
    Remote,
    Tag,
}

/// A ref named on a commit's row: its kind and its short name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Label {
    pub(crate) kind: Kind,
    pub(crate) name: String,
}

/// A commit as a graph's row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Commit {
    /// Its full id, as hex.
    pub(crate) id: String,
    pub(crate) short: String,
    pub(crate) parents: Vec<String>,
    pub(crate) labels: Vec<Label>,
    /// Its author's name as git wrote it, which may be any bytes, cut.
    pub(crate) author: Vec<u8>,
    /// The author's time, in seconds since 1970.
    pub(crate) time: i64,
    /// Its subject as git wrote it, cut as a commit section's.
    pub(crate) subject: Vec<u8>,
}

/// `git log`'s arguments for a page of `refs`: its commits after the first `skip`, at most
/// [`PAGE`] and one more.
pub(crate) fn args(refs: &[String], skip: usize) -> Vec<String> {
    let mut words: Vec<String> = [
        "log",
        "-z",
        "--topo-order",
        "--no-show-signature",
        "--no-color",
        "--decorate=full",
        FORMAT,
    ]
    .map(str::to_owned)
    .to_vec();
    words.push(format!("--skip={skip}"));
    words.push(format!("--max-count={}", PAGE + 1));
    words.extend(refs.iter().cloned());
    words.push("--".into());
    words
}

/// Reads `git log -z` output with [`FORMAT`], keeping at most `keep` commits, and whether there
/// was one more. A record whose ids are not hex is skipped.
pub(crate) fn parse(bytes: &[u8], keep: usize) -> (Vec<Commit>, bool) {
    let mut commits = Vec::new();
    for record in bytes.split(|&byte| byte == 0) {
        let record = record.strip_prefix(b"\n").unwrap_or(record);
        if record.is_empty() {
            continue;
        }
        let Some(commit) = commit(record) else {
            continue;
        };
        if commits.len() == keep {
            return (commits, true);
        }
        commits.push(commit);
    }
    (commits, false)
}

fn commit(record: &[u8]) -> Option<Commit> {
    let mut parts = record.splitn(7, |&byte| byte == 0x1f);
    let mut next = || parts.next();
    let (id, short, parents, refs) = (next()?, next()?, next()?, next()?);
    let (author, time, subject) = (next()?, next()?, next()?);
    let hex = |part: &[u8]| {
        !part.is_empty() && part.len() <= 64 && part.iter().all(u8::is_ascii_hexdigit)
    };
    if !hex(id) || !hex(short) || short.len() > id.len() {
        return None;
    }
    let parents: Vec<String> = (parents.split(|&byte| byte == b' '))
        .filter(|parent| !parent.is_empty())
        .map(|parent| hex(parent).then(|| String::from_utf8_lossy(parent).into_owned()))
        .collect::<Option<_>>()?;
    let time = std::str::from_utf8(time).ok()?.trim().parse().unwrap_or(0);
    let author: String = String::from_utf8_lossy(author)
        .chars()
        .take(MAX_AUTHOR)
        .collect();
    Some(Commit {
        id: String::from_utf8_lossy(id).into_owned(),
        short: String::from_utf8_lossy(short).into_owned(),
        parents,
        labels: labels(refs),
        author: author.into_bytes(),
        time,
        subject: history::cut(subject.strip_suffix(b"\n").unwrap_or(subject)),
    })
}

/// The labels `%D` with `--decorate=full` gives, as `HEAD -> refs/heads/main, refs/remotes/
/// origin/main, tag: refs/tags/v1`: `HEAD` first, then each ref by its kind; a ref of another
/// kind is left out.
pub(crate) fn labels(refs: &[u8]) -> Vec<Label> {
    let refs = String::from_utf8_lossy(refs);
    let mut out = Vec::new();
    for part in refs
        .split(", ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let part = match part.strip_prefix("HEAD -> ") {
            Some(rest) => {
                out.push(Label {
                    kind: Kind::Head,
                    name: "HEAD".into(),
                });
                rest
            }
            None => part,
        };
        let label = if part == "HEAD" {
            Some((Kind::Head, "HEAD"))
        } else if let Some(tag) = part.strip_prefix("tag: refs/tags/") {
            Some((Kind::Tag, tag))
        } else if let Some(local) = part.strip_prefix("refs/heads/") {
            Some((Kind::Local, local))
        } else {
            part.strip_prefix("refs/remotes/")
                .filter(|remote| !remote.ends_with("/HEAD"))
                .map(|remote| (Kind::Remote, remote))
        };
        if let Some((kind, name)) = label {
            out.push(Label {
                kind,
                name: name.to_owned(),
            });
        }
    }
    out
}

/// `time`, seconds since 1970, as `YYYY-MM-DD` at `offset` seconds east of UTC (Q3).
pub(crate) fn date(time: i64, offset: i32) -> String {
    let local = time.saturating_add(i64::from(offset));
    let (year, month, day) = crate::dired::civil_from_days(local.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests;
