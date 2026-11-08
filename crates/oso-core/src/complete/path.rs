//! Completing a path by its segments, as Emacs's `partial-completion` style does (ADR 0030,
//! ADR 0087): what a typed path means, which directories it needs listed, and the candidates
//! once they are.
//!
//! Must not depend on: `editor`, or the file system; the host lists directories.
//!
//! The segments before the last are prefixes of directory names, smart case each: `~/P/S/o`
//! reaches `~/Projects/SideProjects/oso`. A segment that is a whole name matches only that name.
//! One that matches several directories goes into each of them, at most [`MAX_EXPANDED`]. The
//! last segment is matched by the orderless matcher over the names in the directories reached.
//! `~/` or `/` typed right after a `/` starts the path over, as Emacs's shadowed file names do.

use std::path::{Path, PathBuf};

/// The most directories one segment expands into.
pub const MAX_EXPANDED: usize = 32;

/// What a name in a directory is, as its listing says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    File,
    Directory,
    /// A symbolic link, which may lead to a directory.
    Link,
    /// A pipe, a device or a socket.
    Other,
}

/// A name in a listed directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub name: String,
    pub kind: NameKind,
}

/// What a typed path means: where it starts, the segments before the last, and the last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Where the path starts: the root, the home directory, or the directory it is relative to.
    pub root: PathBuf,
    /// The segments between the root and the last, each a prefix of a directory's name.
    pub segments: Vec<String>,
    /// What is typed after the last `/`, which the matcher matches.
    pub last: String,
    /// Where in the typed text the path starts, after the last place it was started over.
    pub start: usize,
}

/// A candidate: what its row shows, and the path it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The name, with `/` after a directory, which the last segment is matched against.
    pub name: String,
    /// The directories from the first segment that matched several, which tell rows of one
    /// name apart, shown beside the name and not matched.
    pub context: String,
    pub path: PathBuf,
    pub directory: bool,
}

/// What expanding a plan came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expanded {
    /// These directories must be listed first.
    Need(Vec<PathBuf>),
    /// The candidates, and whether a segment matched more directories than are kept.
    Ready(Vec<Found>, bool),
}

/// Where `input` starts after it was started over: right after `//`, or at `~/` typed right
/// after a `/`.
pub fn start(input: &str) -> usize {
    let bytes = input.as_bytes();
    let mut start = 0;
    for at in 1..bytes.len() {
        if bytes[at - 1] != b'/' {
            continue;
        }
        if bytes[at] == b'/' || (bytes[at] == b'~' && bytes.get(at + 1).is_none_or(|b| *b == b'/'))
        {
            start = at;
        }
    }
    start
}

/// What `input` means, with `home` for a `~` that begins it and `base` for a relative path.
pub fn plan(input: &str, home: Option<&Path>, base: &Path) -> Plan {
    let start = start(input);
    let path = &input[start..];
    let (root, rest) = if let Some(rest) = path.strip_prefix('/') {
        (PathBuf::from("/"), rest)
    } else if let Some(home) = home.filter(|_| path == "~" || path.starts_with("~/")) {
        (home.to_path_buf(), path.get(2..).unwrap_or(""))
    } else {
        (base.to_path_buf(), path)
    };
    let mut parts: Vec<&str> = rest.split('/').collect();
    let last = parts.pop().unwrap_or("").to_owned();
    let segments = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect();
    Plan {
        root,
        segments,
        last,
        start,
    }
}

/// Whether `name` starts with `prefix`, ignoring case unless `prefix` has an upper-case letter.
fn has_prefix(name: &str, prefix: &str) -> bool {
    if prefix.chars().any(char::is_uppercase) {
        name.starts_with(prefix)
    } else {
        name.to_lowercase().starts_with(&prefix.to_lowercase())
    }
}

/// Whether a name of `kind` may be a directory to go into.
fn may_be_directory(kind: NameKind) -> bool {
    matches!(kind, NameKind::Directory | NameKind::Link)
}

/// A directory a plan has reached, with what its rows show before a name.
type Reached = (PathBuf, String);

/// The candidates of `plan`, given what is listed of each directory, or the directories that
/// must be listed first. A directory `known` to exist, such as one the prompt started in, is
/// gone into by a segment that names it whole without listing the one it is in. Names starting
/// with `.` are left out unless the last segment starts with `.`.
pub fn expand<'a>(
    plan: &Plan,
    listing: impl Fn(&Path) -> Option<&'a [Name]>,
    known: impl Fn(&Path) -> bool,
) -> Expanded {
    let mut reached: Vec<Reached> = vec![(plan.root.clone(), String::new())];
    let mut cut = false;
    for segment in &plan.segments {
        match step(&reached, segment, &listing, &known) {
            Ok((next, more)) => {
                reached = next;
                cut |= more;
            }
            Err(need) => return Expanded::Need(need),
        }
    }
    let need: Vec<PathBuf> = reached
        .iter()
        .filter(|(directory, _)| listing(directory).is_none())
        .map(|(directory, _)| directory.clone())
        .collect();
    if !need.is_empty() {
        return Expanded::Need(need);
    }
    let hidden = plan.last.starts_with('.');
    let mut found = Vec::new();
    for (directory, shown) in &reached {
        let names = listing(directory).unwrap_or(&[]);
        for name in names {
            if name.name.starts_with('.') && !hidden {
                continue;
            }
            let is_directory = name.kind == NameKind::Directory;
            let slash = if is_directory { "/" } else { "" };
            found.push(Found {
                name: format!("{}{slash}", name.name),
                context: shown.clone(),
                path: directory.join(&name.name),
                directory: is_directory,
            });
        }
    }
    Expanded::Ready(found, cut)
}

/// The directories one `segment` reaches from those `reached`, and whether some were left out
/// past [`MAX_EXPANDED`], or the directories that must be listed first.
fn step<'a>(
    reached: &[Reached],
    segment: &str,
    listing: &impl Fn(&Path) -> Option<&'a [Name]>,
    known: &impl Fn(&Path) -> bool,
) -> Result<(Vec<Reached>, bool), Vec<PathBuf>> {
    if segment == "." {
        return Ok((reached.to_vec(), false));
    }
    if segment == ".." {
        let up = reached
            .iter()
            .map(|(directory, shown)| {
                let shown = if shown.is_empty() {
                    String::new()
                } else {
                    format!("{shown}../")
                };
                (directory.join(".."), shown)
            })
            .collect();
        return Ok((up, false));
    }
    let mut next: Vec<(PathBuf, String, String)> = Vec::new();
    let mut need = Vec::new();
    for (directory, shown) in reached {
        let whole = directory.join(segment);
        if known(&whole) {
            next.push((whole, shown.clone(), segment.to_owned()));
            continue;
        }
        let Some(names) = listing(directory) else {
            need.push(directory.clone());
            continue;
        };
        let into = |name: &&Name| may_be_directory(name.kind);
        let whole: Vec<&Name> = names
            .iter()
            .filter(into)
            .filter(|name| name.name == segment)
            .collect();
        let matched = if whole.is_empty() {
            names
                .iter()
                .filter(into)
                .filter(|name| has_prefix(&name.name, segment))
                .collect()
        } else {
            whole
        };
        for name in matched {
            next.push((directory.join(&name.name), shown.clone(), name.name.clone()));
        }
    }
    if !need.is_empty() {
        return Err(need);
    }
    next.sort_by(|a, b| a.0.cmp(&b.0));
    let cut = next.len() > MAX_EXPANDED;
    next.truncate(MAX_EXPANDED);
    let several = next.len() > 1;
    let next = next
        .into_iter()
        .map(|(directory, mut shown, name)| {
            if several || !shown.is_empty() {
                shown.push_str(&name);
                shown.push('/');
            }
            (directory, shown)
        })
        .collect();
    Ok((next, cut))
}

/// The typed text that stands for `path` as a candidate: from the home directory as `~/` when it
/// is under it, with `/` after a directory.
pub fn typed_form(path: &Path, directory: bool, home: Option<&Path>) -> String {
    // The root is its own `/`.
    let slash = if directory && path.parent().is_some() {
        "/"
    } else {
        ""
    };
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~/".to_owned(),
        Some(rest) => format!("~/{}{slash}", rest.display()),
        None => format!("{}{slash}", path.display()),
    }
}

/// The path a typed text names as it is, made absolute from `plan`'s root, for an answer that
/// is no candidate, such as a new file.
pub fn as_typed(plan: &Plan) -> PathBuf {
    let mut path = plan.root.clone();
    for segment in &plan.segments {
        path.push(segment);
    }
    if !plan.last.is_empty() {
        path.push(&plan.last);
    }
    path
}

#[cfg(test)]
mod tests;
