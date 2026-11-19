//! What oso keeps of an issue space (ADR 0136, ADR 0159): its id, its slug, its display name,
//! its description, issue, member repositories, directory, orchestrators and worktrees, which
//! the `spaces` file (`record`) reads and writes; and how a slug, a display name and a
//! description are made from what the person typed.
//!
//! Must not depend on: threads, the UI or `app`; the caller reads the random bytes, off the UI
//! thread, and gives them here.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::sessions::identity::SessionId;

/// The longest name of a space, its slug, in bytes.
pub const MAX_NAME: usize = 48;
/// The longest description, in bytes (ADR 0159).
pub const MAX_DESCRIPTION: usize = 1024;
/// The longest description a build before ADR 0159 reads, as its title, in bytes.
pub const MAX_OLD_TITLE: usize = 200;
/// The longest display name, in bytes (ADR 0159).
pub const MAX_DISPLAY: usize = 100;
/// The most cells of a display name made from a description, when no summary gives one.
const FALLBACK_DISPLAY_CELLS: usize = 24;
/// The most ASCII words of a description a fallback slug is made of.
const FALLBACK_WORDS: usize = 4;
/// The longest issue reference, in bytes.
pub const MAX_ISSUE: usize = 200;
/// The most repositories a space holds.
pub const MAX_MEMBERS: usize = 16;
/// The most earlier orchestrators a space remembers.
pub const MAX_EARLIER: usize = 32;
/// The most worktrees a space records.
pub const MAX_WORKTREES: usize = 256;
/// A space's name when nothing is left of what it was made from.
const FALLBACK: &str = "space";

/// A space's id: 16 random bytes, as 32 lower-case hex digits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpaceId(String);

impl SpaceId {
    /// The id made from `bytes`.
    pub fn from_random(bytes: [u8; 16]) -> Self {
        use fmt::Write as _;
        let mut text = String::with_capacity(32);
        for byte in bytes {
            let _ = write!(text, "{byte:02x}");
        }
        Self(text)
    }

    /// `text` as an id, if it is 32 lower-case hex digits.
    pub fn parse(text: &str) -> Option<Self> {
        let hex = text.len() == 32
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        hex.then(|| Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SpaceId({})", self.0)
    }
}

/// A repository of a space: its main checkout's root, and the root's device and inode where the
/// system has them and they were looked at, as `trusted-projects` keys trust (ADR 0114).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub root: PathBuf,
    pub identity: Option<(u64, u64)>,
}

impl Member {
    /// `root`, with no identity looked at yet.
    pub fn at(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
            identity: None,
        }
    }

    /// Its name in a space: its root's last component.
    pub fn name(&self) -> String {
        last_component(&self.root)
    }
}

/// What the `spaces` file keeps of one space. No screen and no transcript text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub id: SpaceId,
    /// A slug, as [`slug`] leaves it, unique among the spaces oso keeps: the space's
    /// directory, its orchestrator's name and address. It never changes once the directory is
    /// made (ADR 0159).
    pub name: String,
    /// What the person sees the space called, as [`clean_display`] leaves it, unique among the
    /// spaces oso keeps; empty for the slug. `space-rename` changes it (ADR 0159).
    pub display: String,
    /// What the space is for, in the person's words, cleaned; at most [`MAX_DESCRIPTION`]
    /// bytes. Its orchestrator is given it.
    pub description: String,
    /// The issue's reference, such as `#123`, shown and never fetched.
    pub issue: String,
    pub members: Vec<Member>,
    /// Its own directory, once it has one: a space made from repository spaces gets it at its
    /// first use, from the root in force then.
    pub directory: Option<PathBuf>,
    /// The directory's device and inode when oso made it.
    pub directory_identity: Option<(u64, u64)>,
    pub archived: bool,
    /// The orchestrator's session, while it has one (step 3).
    pub orchestrator: Option<SessionId>,
    /// Earlier orchestrators, the latest last.
    pub earlier: Vec<SessionId>,
    /// The worktrees it made or holds, each in one space only.
    pub worktrees: Vec<PathBuf>,
    /// When it was made and when it last changed, in ms since the epoch.
    pub created: u64,
    pub changed: u64,
}

impl Stored {
    /// A new open space of one repository, `member`, called `name`, as of `now`.
    pub fn of_repository(id: SpaceId, name: String, member: Member, now: u64) -> Self {
        Self {
            id,
            name,
            display: String::new(),
            description: String::new(),
            issue: String::new(),
            members: vec![member],
            directory: None,
            directory_identity: None,
            archived: false,
            orchestrator: None,
            earlier: Vec::new(),
            worktrees: Vec::new(),
            created: now,
            changed: now,
        }
    }

    /// What the person sees it called: its display name, else its slug (ADR 0159).
    pub fn label(&self) -> &str {
        if self.display.is_empty() {
            &self.name
        } else {
            &self.display
        }
    }

    /// Whether a session running in `runs_in` is this space's orchestrator in its own right,
    /// with `root` the `[spaces] root` now: the space is open, its directory is one oso makes
    /// under the root, kept with the identity oso made it with where the system has
    /// identities, and the session runs there. The record is one agents can write, so it
    /// alone makes no session elsewhere an orchestrator (ADR 0136, step 4a). The directory
    /// may be named otherwise than the space: `space-rename` leaves it where it is, and a name
    /// changed, or taken, while it was made is kept.
    pub fn orchestrated_from(&self, root: &Path, runs_in: &Path) -> bool {
        let made = self.directory_identity.is_some() || !cfg!(unix);
        let directory = self.directory.as_deref();
        !self.archived
            && made
            && directory.is_some_and(|dir| super::dir::made_under(root, dir) && same(dir, runs_in))
    }

    /// The root of its repository a session may also read `dir` of (ADR 0136, step 4b): `dir`
    /// is that repository's root, or a worktree this space holds inside it. A worktree git
    /// lists but the space does not hold is not taken: git's records are files agents can
    /// write.
    pub fn read_root(&self, dir: &Path) -> Option<&Path> {
        if let Some(member) = self.member(dir) {
            return Some(&member.root);
        }
        if !self.holds_worktree(dir) {
            return None;
        }
        (self.members.iter())
            .map(|member| member.root.as_path())
            .filter(|root| dir.starts_with(root))
            .max_by_key(|root| root.components().count())
    }

    /// Its repository at `root`, by ADR 0114's comparison.
    pub fn member(&self, root: &Path) -> Option<&Member> {
        self.members.iter().find(|member| same(&member.root, root))
    }

    /// Whether `root` is one of its repositories.
    pub fn has_member(&self, root: &Path) -> bool {
        self.members.iter().any(|member| same(&member.root, root))
    }

    /// Whether it records the worktree at `path`.
    pub fn holds_worktree(&self, path: &Path) -> bool {
        self.worktrees.iter().any(|worktree| same(worktree, path))
    }
}

/// Whether `a` and `b` name one place by their words (ADR 0114's comparison).
pub fn same(a: &Path, b: &Path) -> bool {
    oso_core::project::same_place(a, b)
}

/// `raw` as a space's name: lower-case ASCII letters and digits, each run of anything else one
/// `-`, none at either end, at most [`MAX_NAME`] bytes; `space` if nothing is left. So a name
/// is always a directory's name on its own: never `.`, `..`, a path or an option.
pub fn slug(raw: &str) -> String {
    let mut name = String::with_capacity(raw.len().min(MAX_NAME));
    for c in raw.chars() {
        if name.len() == MAX_NAME {
            break;
        }
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    if name.is_empty() {
        FALLBACK.to_owned()
    } else if is_device_name(&name) {
        // A directory of this name would be a device on Windows.
        format!("{name}-{FALLBACK}")
    } else {
        name
    }
}

/// Whether `name`, lower-case, names a device on Windows, as `con` and `com1` do.
fn is_device_name(name: &str) -> bool {
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    matches!(name, "con" | "prn" | "aux" | "nul") || numbered("com") || numbered("lpt")
}

/// `raw`, the person's words for what a space is for, as its description: control and
/// invisible characters dropped, as they could make a row read otherwise than it is, runs of
/// white space made one, trimmed, and cut to [`MAX_DESCRIPTION`] bytes (ADR 0136, step 1b; ADR
/// 0159).
pub fn clean_description(raw: &str) -> String {
    let words: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c.is_control())
        .map(|word| word.chars().filter(|&c| is_plain(c)).collect::<String>())
        .filter(|word| !word.is_empty())
        .collect();
    let description = words.join(" ");
    cut_bytes(&description, MAX_DESCRIPTION)
        .trim_end()
        .to_owned()
}

/// `text` cut to at most `most` bytes, on a character's boundary.
pub fn cut_bytes(text: &str, most: usize) -> &str {
    let mut end = text.len().min(most);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `raw` as a display name (ADR 0159): the clusters oso draws as text, as oso-core's cell grid
/// splits them, so that an emoji sequence keeps its joiners and selectors; every control,
/// bidi control and other invisible character dropped, each run of white space one blank,
/// none at either end; cut to [`MAX_DISPLAY`] bytes between clusters. Cleaning what it gives
/// changes nothing, so that the record's reader can refuse a name that was not cleaned.
pub fn clean_display(raw: &str) -> String {
    let mut text = clean_display_once(raw);
    // Dropping a character can join its neighbours into one cluster, which a second pass may
    // read otherwise: a few passes reach the text that stays.
    for _ in 0..4 {
        let again = clean_display_once(&text);
        if again == text {
            break;
        }
        text = again;
    }
    text
}

/// One pass of [`clean_display`].
fn clean_display_once(raw: &str) -> String {
    use oso_core::display_map::cell::{CellKind, Clusters};
    let mut out = String::with_capacity(raw.len().min(MAX_DISPLAY));
    let mut gap = false;
    for (_, cluster, kind, _) in Clusters::new(raw) {
        let blank = cluster.chars().all(char::is_whitespace);
        match kind {
            CellKind::Text if !blank => {
                let space = gap && !out.is_empty();
                if out.len() + usize::from(space) + cluster.len() > MAX_DISPLAY {
                    break;
                }
                if space {
                    out.push(' ');
                }
                out.push_str(cluster);
                gap = false;
            }
            CellKind::Text | CellKind::Tab | CellKind::LineBreak | CellKind::Control(_) => {
                gap = true;
            }
            CellKind::Invisible(_) | CellKind::PastLimit(_) | CellKind::InvalidByte(_) => {}
        }
    }
    out
}

/// How many cells `text` takes as oso draws it, cluster by cluster.
pub fn display_cells(text: &str) -> usize {
    oso_core::display_map::cell::Clusters::new(text)
        .map(|(.., cells)| cells)
        .sum()
}

/// `text` cut to `cells` cells between clusters, ending in `…` if cut.
pub fn cut_cells(text: &str, cells: usize) -> String {
    if display_cells(text) <= cells {
        return text.to_owned();
    }
    let mut used = 0;
    let mut end = 0;
    for (at, cluster, _, n) in oso_core::display_map::cell::Clusters::new(text) {
        if used + n + 1 > cells {
            break;
        }
        used += n;
        end = at + cluster.len();
    }
    format!("{}…", text[..end].trim_end())
}

/// The base of a slug and the display name of a space made for `description`, with
/// `repository` its first repository's name, when no summary names it (ADR 0159): the slug
/// from the description's first ASCII words, else from the repository's name; the display
/// name the description cut to a few cells. The caller makes both unique.
pub fn fallback_names(description: &str, repository: &str) -> (String, String) {
    let words: Vec<&str> = description
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(FALLBACK_WORDS)
        .collect();
    let base = if words.is_empty() {
        slug(repository)
    } else {
        slug(&words.join("-"))
    };
    let display = cut_cells(&clean_display(description), FALLBACK_DISPLAY_CELLS);
    (base, clean_display(&display))
}

/// `base`, a display name, or `base 2`, `base 3` and on, the first that `taken` does not hold,
/// within [`MAX_DISPLAY`] bytes, cut between clusters.
pub fn unique_display(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| {
            let suffix = format!(" {n}");
            let room = MAX_DISPLAY.saturating_sub(suffix.len());
            let kept = if base.len() <= room {
                base
            } else {
                let mut end = 0;
                for (at, cluster, ..) in oso_core::display_map::cell::Clusters::new(base) {
                    if at + cluster.len() > room {
                        break;
                    }
                    end = at + cluster.len();
                }
                &base[..end]
            };
            format!("{}{suffix}", kept.trim_end())
        })
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// Whether `c` may stand in a description or an issue reference: no control character, and nothing
/// drawn as a boxed code point (oso-core's `is_invisible`).
pub fn is_plain(c: char) -> bool {
    !c.is_control() && !oso_core::display_map::cell::is_invisible(c)
}

/// `base`, a slug, or `base-2`, `base-3` and on, the first that `taken` does not hold, within
/// [`MAX_NAME`] bytes.
pub fn unique(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2u32..)
        .map(|n| {
            let suffix = format!("-{n}");
            let mut keep = MAX_NAME.saturating_sub(suffix.len()).min(base.len());
            while !base.is_char_boundary(keep) {
                keep -= 1;
            }
            format!("{}{suffix}", base[..keep].trim_end_matches('-'))
        })
        .find(|name| !taken(name))
        .unwrap_or_else(|| base.to_owned())
}

/// `path`'s last component, or the whole path when it has none.
pub fn last_component(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Now, in ms since the epoch, as `changed` times are kept.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests;
